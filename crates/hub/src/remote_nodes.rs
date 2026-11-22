//! Registry of connected remote nodes and RPC dispatch over the hub's HTTP/SSE node channel.
//!
//! Each remote machine runs `crossagents node run --hub <url> --token <cad_...>` and holds an
//! outbound SSE stream (`GET /v1/node/stream`). When a person, CLI, or agent invokes an operation
//! on that node (`exec`, `read`, `write`, `edit`, `list`, `search`), the hub pushes a
//! [`NodeRpcCall`] down the stream and awaits the node's `POST /v1/node/results/{call_id}` reply.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossagents_api_types::{NodeRpcCall, NodeRpcOp, NodeRpcReply, RemoteNodeInfo};
use crossagents_core::CoreError;
use serde::de::DeserializeOwned;
use tokio::sync::{RwLock, mpsc, oneshot};

const CHANNEL_CAPACITY: usize = 64;

struct ConnectedNode {
    info: RemoteNodeInfo,
    tx: mpsc::Sender<NodeRpcCall>,
    connection_id: u64,
}

/// Tracks online remote nodes and matches in-flight RPC replies back to callers.
#[derive(Default)]
pub struct RemoteNodes {
    nodes: RwLock<HashMap<String, ConnectedNode>>,
    pending: RwLock<HashMap<String, oneshot::Sender<NodeRpcReply>>>,
    next_conn: std::sync::atomic::AtomicU64,
}

/// Guard returned by [`RemoteNodes::register`]; dropping it removes the node when the stream
/// closes (unless a newer connection for the same device already replaced it).
pub struct NodeRegistration {
    registry: Arc<RemoteNodes>,
    device_id: String,
    connection_id: u64,
}

impl Drop for NodeRegistration {
    fn drop(&mut self) {
        let registry = self.registry.clone();
        let device_id = self.device_id.clone();
        let connection_id = self.connection_id;
        tokio::spawn(async move {
            let mut nodes = registry.nodes.write().await;
            if nodes
                .get(&device_id)
                .is_some_and(|n| n.connection_id == connection_id)
            {
                nodes.remove(&device_id);
                tracing::info!(node = %device_id, "remote node disconnected");
            }
        });
    }
}

impl RemoteNodes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a newly connected remote node and returns its outbound call receiver plus a
    /// drop guard that unregisters it on disconnect.
    pub async fn register(
        self: &Arc<Self>,
        info: RemoteNodeInfo,
    ) -> (mpsc::Receiver<NodeRpcCall>, NodeRegistration) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let connection_id = self
            .next_conn
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let device_id = info.id.clone();
        tracing::info!(
            node = %info.name,
            id = %device_id,
            os = %info.os,
            hostname = %info.hostname,
            "remote node connected"
        );
        self.nodes.write().await.insert(
            device_id.clone(),
            ConnectedNode {
                info,
                tx,
                connection_id,
            },
        );
        (
            rx,
            NodeRegistration {
                registry: self.clone(),
                device_id,
                connection_id,
            },
        )
    }

    /// Lists all online nodes: the hub's own `"local"` machine first, then connected remote nodes
    /// sorted by name.
    pub async fn list(&self, started_at: DateTime<Utc>, version: &str) -> Vec<RemoteNodeInfo> {
        let mut out = vec![local_node_info(started_at, version)];
        let mut remote: Vec<RemoteNodeInfo> = self
            .nodes
            .read()
            .await
            .values()
            .map(|n| n.info.clone())
            .collect();
        remote.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out.extend(remote);
        out
    }

    /// Delivers a reply posted by a remote node for `call_id`. Returns `true` if a caller was
    /// waiting for it.
    pub async fn complete(&self, call_id: &str, reply: NodeRpcReply) -> bool {
        let tx = self.pending.write().await.remove(call_id);
        match tx {
            Some(tx) => tx.send(reply).is_ok(),
            None => false,
        }
    }

    /// Dispatches `op` to the connected remote node named `node_key` (matched by id, name, or
    /// hostname, case-insensitively) and decodes the typed response.
    pub async fn call<T: DeserializeOwned>(
        &self,
        node_key: &str,
        op: NodeRpcOp,
        timeout: Duration,
    ) -> Result<T, CoreError> {
        let key = node_key.trim();
        if key.is_empty() {
            return Err(CoreError::Agent("missing node name".into()));
        }
        if key.eq_ignore_ascii_case("local") {
            return Err(CoreError::Agent(
                "node \"local\" is the hub's own machine; use local tools or specify a remote node name".into(),
            ));
        }

        let (node_name, tx) = {
            let nodes = self.nodes.read().await;
            let found = nodes.get(key).or_else(|| {
                nodes.values().find(|n| {
                    n.info.name.eq_ignore_ascii_case(key)
                        || n.info.hostname.eq_ignore_ascii_case(key)
                })
            });
            match found {
                Some(n) => (n.info.name.clone(), n.tx.clone()),
                None => {
                    let online: Vec<&str> =
                        nodes.values().map(|n| n.info.name.as_str()).collect();
                    let hint = if online.is_empty() {
                        "no remote nodes are currently connected (start one with `crossagents node run`)".into()
                    } else {
                        format!("online remote nodes: {}", online.join(", "))
                    };
                    return Err(CoreError::NotFound {
                        kind: "remote node",
                        id: format!("{key} ({hint})"),
                    });
                }
            }
        };

        let call_id = uuid::Uuid::now_v7().simple().to_string();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.pending.write().await.insert(call_id.clone(), reply_tx);

        let send_res = tx
            .send(NodeRpcCall {
                call_id: call_id.clone(),
                op,
            })
            .await;
        if send_res.is_err() {
            self.pending.write().await.remove(&call_id);
            return Err(CoreError::Agent(format!(
                "remote node {node_name:?} disconnected before receiving the command"
            )));
        }

        let reply = match tokio::time::timeout(timeout, reply_rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => {
                self.pending.write().await.remove(&call_id);
                return Err(CoreError::Agent(format!(
                    "remote node {node_name:?} dropped the call"
                )));
            }
            Err(_) => {
                self.pending.write().await.remove(&call_id);
                return Err(CoreError::Agent(format!(
                    "remote node {node_name:?} did not answer within {}s",
                    timeout.as_secs()
                )));
            }
        };

        if !reply.ok {
            return Err(CoreError::Agent(
                reply
                    .error
                    .unwrap_or_else(|| format!("remote node {node_name:?} returned an error")),
            ));
        }

        let payload = reply.payload.unwrap_or(serde_json::Value::Null);
        serde_json::from_value(payload).map_err(|e| {
            CoreError::Agent(format!(
                "invalid response payload from remote node {node_name:?}: {e}"
            ))
        })
    }
}

fn local_node_info(started_at: DateTime<Utc>, version: &str) -> RemoteNodeInfo {
    let hostname = std::env::var(if cfg!(windows) {
        "COMPUTERNAME"
    } else {
        "HOSTNAME"
    })
    .ok()
    .map(|v| v.trim().to_owned())
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| "local".into());
    let default_cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let shell = if cfg!(windows) {
        "powershell".to_owned()
    } else {
        std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                std::path::Path::new(&s)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "sh".to_owned())
    };
    RemoteNodeInfo {
        id: "local".into(),
        name: "local".into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        hostname,
        shell,
        default_cwd,
        version: version.into(),
        connected_at: started_at,
        local: true,
    }
}
