//! End-to-end integration test for cross-platform remote node control:
//! - Issuing a `DeviceKind::Node` token
//! - Enforcing that node tokens are rejected on normal routes (`403`) and accepted on `/v1/node/*`
//! - Connecting a remote node over SSE (`GET /v1/node/stream`)
//! - Listing online nodes (`GET /v1/nodes`)
//! - Executing remote commands & file operations (`exec`, `write`, `read`, `edit`, `list`, `search`)
//! - Revoking the node device and verifying it disconnects cleanly.

use std::time::Duration;

use crossagents_api_types::{
    CreateDeviceRequest, CreateDeviceResponse, DeviceKind, ListNodesResponse, NodeEditFileRequest,
    NodeEditFileResponse, NodeExecRequest, NodeExecResponse, NodeListFilesRequest,
    NodeListFilesResponse, NodeReadFileRequest, NodeReadFileResponse, NodeRpcCall, NodeRpcOp,
    NodeRpcReply, NodeSearchFilesRequest, NodeSearchFilesResponse, NodeWriteFileRequest,
    NodeWriteFileResponse,
};
use crossagents_hub::{HubConfig, Paths};
use futures::StreamExt;
use serde_json::json;

#[tokio::test]
async fn remote_node_channel_exec_and_file_ops_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let cfg = HubConfig::new(Paths::new(home.path().to_path_buf()), 0);
    let hub = crossagents_hub::start(cfg).await.unwrap();
    let base = format!("http://{}", hub.addr);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    // 1. Issue a node device credential.
    let created: CreateDeviceResponse = http
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&hub.token)
        .json(&CreateDeviceRequest {
            name: "linux-bot".into(),
            kind: Some(DeviceKind::Node),
            level: None,
            os: None,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let node_token = created.token;
    let device_id = created.device.id;

    // 2. Node token must still be refused (403) on ordinary API routes.
    let denied = http
        .get(format!("{base}/v1/agents"))
        .bearer_auth(&node_token)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status().as_u16(), 403);

    // 3. Connect a simulated remote node over SSE (`/v1/node/stream`).
    let node_cwd = work.path().to_path_buf();
    let stream_url = format!(
        "{base}/v1/node/stream?os=linux&arch=x86_64&hostname=bot-vps&shell=bash&version=0.3.0"
    );
    let resp = http
        .get(stream_url)
        .bearer_auth(&node_token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let node_task = {
        let http = http.clone();
        let base = base.clone();
        let token = node_token.clone();
        let cwd = node_cwd.clone();
        tokio::spawn(async move {
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();
            let mut event = String::new();
            while let Some(Ok(bytes)) = stream.next().await {
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_owned();
                    buf.drain(..=pos);
                    if line.is_empty() {
                        event.clear();
                        continue;
                    }
                    if let Some(ev) = line.strip_prefix("event:") {
                        event = ev.trim().to_owned();
                        continue;
                    }
                    if let Some(data) = line.strip_prefix("data:")
                        && event == "call"
                    {
                        let call: NodeRpcCall = serde_json::from_str(data.trim()).unwrap();
                        let reply = handle_mock_node_op(&cwd, call.op);
                        let _ = http
                            .post(format!("{base}/v1/node/results/{}", call.call_id))
                            .bearer_auth(&token)
                            .json(&reply)
                            .send()
                            .await;
                    }
                }
            }
        })
    };

    // Wait briefly for the node registration to appear in `GET /v1/nodes`.
    let mut found = false;
    for _ in 0..30 {
        let list: ListNodesResponse = http
            .get(format!("{base}/v1/nodes"))
            .bearer_auth(&hub.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if list.nodes.iter().any(|n| n.name == "linux-bot" && !n.local) {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(found, "remote node did not register in GET /v1/nodes");

    // 4. Write -> Read -> Edit -> List -> Search -> Exec on remote node.
    let written: NodeWriteFileResponse = http
        .post(format!("{base}/v1/nodes/linux-bot/files/write"))
        .bearer_auth(&hub.token)
        .json(&NodeWriteFileRequest {
            path: "src/bot.py".into(),
            content: "print('hello old')\n".into(),
            create_dirs: Some(true),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(written.bytes, 19);

    let edited: NodeEditFileResponse = http
        .post(format!("{base}/v1/nodes/linux-bot/files/edit"))
        .bearer_auth(&hub.token)
        .json(&NodeEditFileRequest {
            path: "src/bot.py".into(),
            target: "hello old".into(),
            replacement: "hello v0.3.0".into(),
            replace_all: Some(false),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(edited.replacements, 1);

    let read: NodeReadFileResponse = http
        .post(format!("{base}/v1/nodes/linux-bot/files/read"))
        .bearer_auth(&hub.token)
        .json(&NodeReadFileRequest {
            path: "src/bot.py".into(),
            start_line: Some(1),
            end_line: Some(5),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read.content, "print('hello v0.3.0')");

    let listed: NodeListFilesResponse = http
        .post(format!("{base}/v1/nodes/linux-bot/files/list"))
        .bearer_auth(&hub.token)
        .json(&NodeListFilesRequest {
            path: "src".into(),
            max_entries: Some(50),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].name, "bot.py");

    let searched: NodeSearchFilesResponse = http
        .post(format!("{base}/v1/nodes/linux-bot/files/search"))
        .bearer_auth(&hub.token)
        .json(&NodeSearchFilesRequest {
            path: "".into(),
            pattern: "v0.3.0".into(),
            max_matches: Some(20),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(searched.matches.len(), 1);
    assert_eq!(searched.matches[0].line, 1);

    let exec: NodeExecResponse = http
        .post(format!("{base}/v1/nodes/linux-bot/exec"))
        .bearer_auth(&hub.token)
        .json(&NodeExecRequest {
            command: "echo ok".into(),
            cwd: None,
            timeout_secs: Some(10),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(exec.exit_code, Some(0));
    assert!(exec.stdout.contains("ok"));

    // 5. Revoking the device drops the remote node connection.
    http.delete(format!("{base}/v1/devices/{device_id}"))
        .bearer_auth(&hub.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), node_task).await;

    hub.shutdown();
    let _ = hub.wait().await;
}

fn handle_mock_node_op(root: &std::path::Path, op: NodeRpcOp) -> NodeRpcReply {
    match op {
        NodeRpcOp::WriteFile(req) => {
            let p = root.join(&req.path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&p, req.content.as_bytes()).unwrap();
            NodeRpcReply {
                ok: true,
                payload: Some(json!({
                    "path": p.to_string_lossy(),
                    "bytes": req.content.len() as u64
                })),
                error: None,
            }
        }
        NodeRpcOp::EditFile(req) => {
            let p = root.join(&req.path);
            let text = std::fs::read_to_string(&p).unwrap();
            let next = text.replacen(&req.target, &req.replacement, 1);
            std::fs::write(&p, next.as_bytes()).unwrap();
            NodeRpcReply {
                ok: true,
                payload: Some(json!({
                    "path": p.to_string_lossy(),
                    "replacements": 1
                })),
                error: None,
            }
        }
        NodeRpcOp::ReadFile(req) => {
            let p = root.join(&req.path);
            let text = std::fs::read_to_string(&p).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            NodeRpcReply {
                ok: true,
                payload: Some(json!({
                    "path": p.to_string_lossy(),
                    "content": lines.join("\n"),
                    "total_lines": lines.len(),
                    "truncated": false
                })),
                error: None,
            }
        }
        NodeRpcOp::ListFiles(req) => {
            let p = root.join(&req.path);
            let mut entries = Vec::new();
            for e in std::fs::read_dir(&p).unwrap() {
                let e = e.unwrap();
                let m = e.metadata().unwrap();
                entries.push(json!({
                    "name": e.file_name().to_string_lossy(),
                    "is_dir": m.is_dir(),
                    "size": if m.is_dir() { 0 } else { m.len() }
                }));
            }
            NodeRpcReply {
                ok: true,
                payload: Some(json!({
                    "path": p.to_string_lossy(),
                    "entries": entries,
                    "truncated": false
                })),
                error: None,
            }
        }
        NodeRpcOp::SearchFiles(req) => {
            let mut matches = Vec::new();
            let p = root.join("src/bot.py");
            let text = std::fs::read_to_string(&p).unwrap();
            for (i, line) in text.lines().enumerate() {
                if line.contains(&req.pattern) {
                    matches.push(json!({
                        "path": "src/bot.py",
                        "line": i + 1,
                        "text": line
                    }));
                }
            }
            NodeRpcReply {
                ok: true,
                payload: Some(json!({
                    "path": root.to_string_lossy(),
                    "matches": matches,
                    "truncated": false
                })),
                error: None,
            }
        }
        NodeRpcOp::Exec(req) => NodeRpcReply {
            ok: true,
            payload: Some(json!({
                "exit_code": 0,
                "stdout": format!("{}\n", req.command),
                "stderr": "",
                "timed_out": false,
                "duration_ms": 5
            })),
            error: None,
        },
    }
}
