//! HTTP routes and MCP tool handlers for cross-platform remote node control.
//!
//! Two router layers:
//! - [`node_channel_router`]: authenticated strictly with `DeviceKind::Node` tokens (`cad_...`).
//!   Hosts `GET /v1/node/stream` (SSE push of `NodeRpcCall`) and `POST /v1/node/results/{call_id}`.
//! - [`control_router`]: merged into the hub's normal `authed` router. Hosts `GET /v1/nodes`
//!   (`read`) and `POST /v1/nodes/{node}/...` (`admin`).
//!
//! In addition, [`mcp_tools`] and [`call_mcp_tool`] expose the same remote node operations to
//! agent sessions via `/v1/me/tools`, so a model running on this machine can inspect, edit, and
//! run commands on any connected Windows / macOS / Linux node.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use crossagents_api_types::{
    ApiError, ErrorCode, ListNodesResponse, NodeEditFileRequest, NodeEditFileResponse,
    NodeExecRequest, NodeExecResponse, NodeListFilesRequest, NodeListFilesResponse,
    NodeReadFileRequest, NodeReadFileResponse, NodeRpcOp, NodeRpcReply, NodeSearchFilesRequest,
    NodeSearchFilesResponse, NodeWriteFileRequest, NodeWriteFileResponse, RemoteNodeInfo,
    ToolInfo,
};
use crossagents_core::{CoreError, Device, DeviceKind, DeviceLevel, DeviceOs};
use futures::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::api::{ApiResult, AppError, AppState, VERSION};
use crate::remote_nodes::NodeStreamMsg;

const FILE_OP_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 60;
const MAX_EXEC_TIMEOUT_SECS: u64 = 120;
const NODE_FRAME_BODY_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone)]
struct AuthedNode {
    device: Device,
    cancel: CancellationToken,
}

/// Router for connected remote nodes (`DeviceKind::Node` tokens only).
pub fn node_channel_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/node/stream", get(node_stream))
        .route("/v1/node/results/{call_id}", post(node_result))
        .route(
            "/v1/node/frames",
            post(node_frames).layer(DefaultBodyLimit::max(NODE_FRAME_BODY_LIMIT)),
        )
        .route_layer(middleware::from_fn_with_state(state, require_node_token))
}

/// Router for listing and controlling remote nodes (merged into the hub's `authed` router).
pub fn control_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/nodes", get(list_nodes))
        .route("/v1/nodes/{node}/exec", post(node_exec))
        .route("/v1/nodes/{node}/files/read", post(node_read_file))
        .route("/v1/nodes/{node}/files/write", post(node_write_file))
        .route("/v1/nodes/{node}/files/edit", post(node_edit_file))
        .route("/v1/nodes/{node}/files/list", post(node_list_files))
        .route("/v1/nodes/{node}/files/search", post(node_search_files))
}

async fn require_node_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = presented else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new(ErrorCode::Unauthorized, "missing node token")),
        )
            .into_response();
    };
    let os = headers
        .get("x-crossagents-client-os")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_device_os);
    let authed = state
        .devices
        .authenticate_reporting(state.bus.store().as_ref(), token, os)
        .await;
    match authed {
        Ok(Some((device, cancel)))
            if device.kind == DeviceKind::Node
                || (device.kind == DeviceKind::Client && device.level >= DeviceLevel::Write) =>
        {
            request
                .extensions_mut()
                .insert(AuthedNode { device, cancel });
            next.run(request).await
        }
        Ok(Some(_)) => (
            StatusCode::FORBIDDEN,
            Json(ApiError::new(
                ErrorCode::Forbidden,
                "this endpoint requires a node token or a write/admin client device token",
            )),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new(
                ErrorCode::Unauthorized,
                "invalid or revoked node token",
            )),
        )
            .into_response(),
        Err(err) => AppError::from(err).into_response(),
    }
}

fn parse_device_os(raw: &str) -> Option<DeviceOs> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "windows" => Some(DeviceOs::Windows),
        "macos" | "darwin" => Some(DeviceOs::Macos),
        "linux" => Some(DeviceOs::Linux),
        _ => None,
    }
}

#[derive(Debug, Deserialize, Default)]
struct NodeStreamQuery {
    #[serde(default)]
    os: String,
    #[serde(default)]
    arch: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    shell: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    chats_dir: String,
    #[serde(default)]
    version: String,
}

async fn node_stream(
    State(state): State<Arc<AppState>>,
    Extension(authed): Extension<AuthedNode>,
    Query(q): Query<NodeStreamQuery>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let chats_dir = q.chats_dir.trim();
    let info = RemoteNodeInfo {
        id: authed.device.id.clone(),
        name: authed.device.name.clone(),
        os: if q.os.trim().is_empty() {
            "unknown".into()
        } else {
            q.os.trim().to_owned()
        },
        arch: if q.arch.trim().is_empty() {
            "unknown".into()
        } else {
            q.arch.trim().to_owned()
        },
        hostname: if q.hostname.trim().is_empty() {
            authed.device.name.clone()
        } else {
            q.hostname.trim().to_owned()
        },
        shell: if q.shell.trim().is_empty() {
            "sh".into()
        } else {
            q.shell.trim().to_owned()
        },
        default_cwd: q.cwd.trim().to_owned(),
        chats_dir: (!chats_dir.is_empty()).then(|| chats_dir.to_owned()),
        version: if q.version.trim().is_empty() {
            "unknown".into()
        } else {
            q.version.trim().to_owned()
        },
        connected_at: Utc::now(),
        local: false,
        online: true,
        plugins: Vec::new(),
    };

    let (mut rx, registration) = state.remote_nodes.register(info.clone()).await;
    let (out_tx, out_rx) = mpsc::channel::<Result<SseEvent, Infallible>>(256);
    let cancel = authed.cancel;
    let shutdown = state.shutdown.clone();

    tokio::spawn(async move {
        let _registration = registration;
        if let Ok(hello) = serde_json::to_string(&json!({ "connected": info })) {
            let _ = out_tx
                .send(Ok(SseEvent::default().event("hello").data(hello)))
                .await;
        }
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = shutdown.cancelled() => break,
                _ = out_tx.closed() => break,
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    let sse = match msg {
                        NodeStreamMsg::Call(call) => {
                            let Ok(data) = serde_json::to_string(&call) else { continue };
                            SseEvent::default().event("call").data(data)
                        }
                        NodeStreamMsg::Frame(line) => {
                            SseEvent::default()
                                .event("frame")
                                .data(line.trim_end_matches(['\r', '\n']))
                        }
                    };
                    if out_tx.send(Ok(sse)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    Sse::new(ReceiverStream::new(out_rx)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

async fn node_frames(
    State(state): State<Arc<AppState>>,
    Extension(authed): Extension<AuthedNode>,
    body: String,
) -> StatusCode {
    if state.remote_nodes.feed_frames(&authed.device.id, &body).await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CONFLICT
    }
}

async fn node_result(
    State(state): State<Arc<AppState>>,
    Path(call_id): Path<String>,
    Json(reply): Json<NodeRpcReply>,
) -> StatusCode {
    if state.remote_nodes.complete(&call_id, reply).await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn list_nodes(State(state): State<Arc<AppState>>) -> Json<ListNodesResponse> {
    Json(ListNodesResponse {
        nodes: state
            .remote_nodes
            .list(
                state.store().as_ref(),
                state.runtime.plugins(),
                state.started_at,
                VERSION,
            )
            .await,
    })
}

async fn node_exec(
    State(state): State<Arc<AppState>>,
    Path(node): Path<String>,
    Json(mut req): Json<NodeExecRequest>,
) -> ApiResult<Json<NodeExecResponse>> {
    let secs = req
        .timeout_secs
        .unwrap_or(DEFAULT_EXEC_TIMEOUT_SECS)
        .clamp(1, MAX_EXEC_TIMEOUT_SECS);
    req.timeout_secs = Some(secs);
    let resp: NodeExecResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::Exec(req), Duration::from_secs(secs + 10))
        .await?;
    Ok(Json(resp))
}

async fn node_read_file(
    State(state): State<Arc<AppState>>,
    Path(node): Path<String>,
    Json(req): Json<NodeReadFileRequest>,
) -> ApiResult<Json<NodeReadFileResponse>> {
    let resp: NodeReadFileResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::ReadFile(req), FILE_OP_TIMEOUT)
        .await?;
    Ok(Json(resp))
}

async fn node_write_file(
    State(state): State<Arc<AppState>>,
    Path(node): Path<String>,
    Json(req): Json<NodeWriteFileRequest>,
) -> ApiResult<Json<NodeWriteFileResponse>> {
    let resp: NodeWriteFileResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::WriteFile(req), FILE_OP_TIMEOUT)
        .await?;
    Ok(Json(resp))
}

async fn node_edit_file(
    State(state): State<Arc<AppState>>,
    Path(node): Path<String>,
    Json(req): Json<NodeEditFileRequest>,
) -> ApiResult<Json<NodeEditFileResponse>> {
    let resp: NodeEditFileResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::EditFile(req), FILE_OP_TIMEOUT)
        .await?;
    Ok(Json(resp))
}

async fn node_list_files(
    State(state): State<Arc<AppState>>,
    Path(node): Path<String>,
    Json(req): Json<NodeListFilesRequest>,
) -> ApiResult<Json<NodeListFilesResponse>> {
    let resp: NodeListFilesResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::ListFiles(req), FILE_OP_TIMEOUT)
        .await?;
    Ok(Json(resp))
}

async fn node_search_files(
    State(state): State<Arc<AppState>>,
    Path(node): Path<String>,
    Json(req): Json<NodeSearchFilesRequest>,
) -> ApiResult<Json<NodeSearchFilesResponse>> {
    let resp: NodeSearchFilesResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::SearchFiles(req), FILE_OP_TIMEOUT)
        .await?;
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------------------
// Agent MCP tools (brain here, hands on another Windows / macOS / Linux node)

pub fn mcp_tools() -> Vec<ToolInfo> {
    vec![
        ToolInfo {
            name: "node_list".into(),
            description: "List all machines (nodes) connected to the CrossAgents hub, including their OS, arch, default shell, and working directory.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolInfo {
            name: "node_exec".into(),
            description: "Run a shell command on a connected remote node (Windows/macOS/Linux). Check `node_list` first to know the target node's OS and shell.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node": { "type": "string", "description": "Target remote node name, id, or hostname." },
                    "command": { "type": "string", "description": "Shell command to execute on the target node." },
                    "cwd": { "type": "string", "description": "Optional working directory on the target node." },
                    "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds (1-120, default 60)." }
                },
                "required": ["node", "command"]
            }),
        },
        ToolInfo {
            name: "node_read".into(),
            description: "Read a UTF-8 text file on a connected remote node.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node": { "type": "string", "description": "Target remote node name, id, or hostname." },
                    "path": { "type": "string", "description": "Absolute or node-cwd-relative path on the target node." },
                    "start_line": { "type": "integer", "description": "Optional 1-based start line." },
                    "end_line": { "type": "integer", "description": "Optional 1-based inclusive end line." }
                },
                "required": ["node", "path"]
            }),
        },
        ToolInfo {
            name: "node_write".into(),
            description: "Write or create a UTF-8 file on a connected remote node.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node": { "type": "string", "description": "Target remote node name, id, or hostname." },
                    "path": { "type": "string", "description": "Absolute or node-cwd-relative file path." },
                    "content": { "type": "string", "description": "Full file contents to write." },
                    "create_dirs": { "type": "boolean", "description": "Create missing parent directories (default true)." }
                },
                "required": ["node", "path", "content"]
            }),
        },
        ToolInfo {
            name: "node_edit".into(),
            description: "Replace exact text inside a file on a connected remote node.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node": { "type": "string", "description": "Target remote node name, id, or hostname." },
                    "path": { "type": "string", "description": "File path on the target node." },
                    "target": { "type": "string", "description": "Exact existing text to replace." },
                    "replacement": { "type": "string", "description": "Replacement text." },
                    "replace_all": { "type": "boolean", "description": "Replace all occurrences (default false: first match only)." }
                },
                "required": ["node", "path", "target", "replacement"]
            }),
        },
        ToolInfo {
            name: "node_list_files".into(),
            description: "List files and directories inside a folder on a connected remote node.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node": { "type": "string", "description": "Target remote node name, id, or hostname." },
                    "path": { "type": "string", "description": "Directory path (empty uses the node's default cwd)." },
                    "max_entries": { "type": "integer", "description": "Maximum entries to return (default 200)." }
                },
                "required": ["node"]
            }),
        },
        ToolInfo {
            name: "node_search".into(),
            description: "Search for a text substring recursively in files on a connected remote node.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node": { "type": "string", "description": "Target remote node name, id, or hostname." },
                    "pattern": { "type": "string", "description": "Substring to search for." },
                    "path": { "type": "string", "description": "Directory or file to search (empty uses the node's default cwd)." },
                    "max_matches": { "type": "integer", "description": "Maximum matching lines to return (default 100)." }
                },
                "required": ["node", "pattern"]
            }),
        },
    ]
}

/// Handles a remote-node MCP tool call if `name` is one of ours; returns `None` for other tools.
pub async fn call_mcp_tool(
    state: &AppState,
    name: &str,
    input: Value,
) -> Option<Result<Value, CoreError>> {
    match name {
        "node_list" => {
            let nodes = state
                .remote_nodes
                .list(
                    state.store().as_ref(),
                    state.runtime.plugins(),
                    state.started_at,
                    VERSION,
                )
                .await;
            Some(Ok(json!({ "nodes": nodes })))
        }
        "node_exec" => Some(run_mcp_exec(state, input).await),
        "node_read" => Some(run_mcp_read(state, input).await),
        "node_write" => Some(run_mcp_write(state, input).await),
        "node_edit" => Some(run_mcp_edit(state, input).await),
        "node_list_files" => Some(run_mcp_list(state, input).await),
        "node_search" => Some(run_mcp_search(state, input).await),
        _ => None,
    }
}

fn require_node_arg(input: &Value) -> Result<String, CoreError> {
    input
        .get("node")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| CoreError::Agent("missing required string field `node`".into()))
}

async fn run_mcp_exec(state: &AppState, input: Value) -> Result<Value, CoreError> {
    let node = require_node_arg(&input)?;
    let mut req: NodeExecRequest = serde_json::from_value(input)
        .map_err(|e| CoreError::Agent(format!("invalid node_exec arguments: {e}")))?;
    let secs = req
        .timeout_secs
        .unwrap_or(DEFAULT_EXEC_TIMEOUT_SECS)
        .clamp(1, MAX_EXEC_TIMEOUT_SECS);
    req.timeout_secs = Some(secs);
    let resp: NodeExecResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::Exec(req), Duration::from_secs(secs + 10))
        .await?;
    serde_json::to_value(resp).map_err(|e| CoreError::Agent(e.to_string()))
}

async fn run_mcp_read(state: &AppState, input: Value) -> Result<Value, CoreError> {
    let node = require_node_arg(&input)?;
    let req: NodeReadFileRequest = serde_json::from_value(input)
        .map_err(|e| CoreError::Agent(format!("invalid node_read arguments: {e}")))?;
    let resp: NodeReadFileResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::ReadFile(req), FILE_OP_TIMEOUT)
        .await?;
    serde_json::to_value(resp).map_err(|e| CoreError::Agent(e.to_string()))
}

async fn run_mcp_write(state: &AppState, input: Value) -> Result<Value, CoreError> {
    let node = require_node_arg(&input)?;
    let req: NodeWriteFileRequest = serde_json::from_value(input)
        .map_err(|e| CoreError::Agent(format!("invalid node_write arguments: {e}")))?;
    let resp: NodeWriteFileResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::WriteFile(req), FILE_OP_TIMEOUT)
        .await?;
    serde_json::to_value(resp).map_err(|e| CoreError::Agent(e.to_string()))
}

async fn run_mcp_edit(state: &AppState, input: Value) -> Result<Value, CoreError> {
    let node = require_node_arg(&input)?;
    let req: NodeEditFileRequest = serde_json::from_value(input)
        .map_err(|e| CoreError::Agent(format!("invalid node_edit arguments: {e}")))?;
    let resp: NodeEditFileResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::EditFile(req), FILE_OP_TIMEOUT)
        .await?;
    serde_json::to_value(resp).map_err(|e| CoreError::Agent(e.to_string()))
}

async fn run_mcp_list(state: &AppState, input: Value) -> Result<Value, CoreError> {
    let node = require_node_arg(&input)?;
    let req: NodeListFilesRequest = serde_json::from_value(input)
        .map_err(|e| CoreError::Agent(format!("invalid node_list_files arguments: {e}")))?;
    let resp: NodeListFilesResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::ListFiles(req), FILE_OP_TIMEOUT)
        .await?;
    serde_json::to_value(resp).map_err(|e| CoreError::Agent(e.to_string()))
}

async fn run_mcp_search(state: &AppState, input: Value) -> Result<Value, CoreError> {
    let node = require_node_arg(&input)?;
    let req: NodeSearchFilesRequest = serde_json::from_value(input)
        .map_err(|e| CoreError::Agent(format!("invalid node_search arguments: {e}")))?;
    let resp: NodeSearchFilesResponse = state
        .remote_nodes
        .call(&node, NodeRpcOp::SearchFiles(req), FILE_OP_TIMEOUT)
        .await?;
    serde_json::to_value(resp).map_err(|e| CoreError::Agent(e.to_string()))
}
