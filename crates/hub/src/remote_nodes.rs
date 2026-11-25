//! Registry of connected remote nodes, RPC dispatch, and `crates/node` driver bridging
//! over the hub's HTTP/SSE node channel.
//!
//! Each remote machine runs `crossagents node run --hub <url> --token <cad_...>` and holds an
//! outbound SSE stream (`GET /v1/node/stream`). Two protocols multiplex cleanly on that stream:
//! - `event: call` / `POST /v1/node/results/{call_id}`: stateless file/exec RPC (`NodeRpcOp`).
//! - `event: frame` / `POST /v1/node/frames`: `crates/node` JSON-line frames (`ToNode` / `ToHub`)
//!   driving real CLI sessions (`codex`, `claude`, etc.) natively on the remote machine via
//!   [`RemoteNode`] + [`SnapshotDriverInfo`].

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crossagents_api_types::{
    CliInfo, NodePluginInfo, NodeRpcCall, NodeRpcOp, NodeRpcReply, PluginInfo, RemoteNodeInfo,
};
use crossagents_core::{
    Capabilities, CoreError, DeviceKind, Driver, DriverKind, EventBus, EventKind, Store,
};
use crossagents_node::{
    HubLink, PluginSnapshot, RemoteNode, SnapshotDriverInfo, Transport, TransportError,
};
use crossagents_plugins::PluginRegistry;
use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot, watch};

/// Messages pushed from the hub to a connected remote node over `GET /v1/node/stream`.
pub enum NodeStreamMsg {
    Call(NodeRpcCall),
    Frame(String),
}

/// Hub end of a single HTTP/SSE node control connection implementing [`Transport`].
struct HttpHubTransport {
    out_tx: mpsc::UnboundedSender<NodeStreamMsg>,
    in_rx: Mutex<mpsc::UnboundedReceiver<String>>,
    closed: watch::Receiver<bool>,
}

#[async_trait]
impl Transport for HttpHubTransport {
    async fn send_line(&self, line: String) -> Result<(), TransportError> {
        if *self.closed.borrow() {
            return Err(TransportError("remote node connection closed".into()));
        }
        self.out_tx
            .send(NodeStreamMsg::Frame(line))
            .map_err(|_| TransportError("remote node stream went away".into()))
    }

    async fn recv_line(&self) -> Option<String> {
        let mut rx = self.in_rx.lock().await;
        let mut closed = self.closed.clone();
        tokio::select! {
            line = rx.recv() => line,
            _ = closed.wait_for(|c| *c) => None,
        }
    }
}

/// Reverse [`HubLink`]: awaits inbound [`HttpHubTransport`] connections pushed when a remote node
/// opens `GET /v1/node/stream`.
struct InboundHubLink {
    accepted: Mutex<mpsc::UnboundedReceiver<Arc<dyn Transport>>>,
}

#[async_trait]
impl HubLink for InboundHubLink {
    async fn connect(&self) -> Result<Arc<dyn Transport>, TransportError> {
        self.accepted
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| TransportError("hub node link closed".into()))
    }
}

/// Persistent per-device slot keeping [`RemoteNode`] alive across transient disconnects so
/// session routes and sequence deduplication survive brief network drops.
struct NodeSlot {
    remote: Arc<RemoteNode>,
    accept_tx: mpsc::UnboundedSender<Arc<dyn Transport>>,
}

struct ConnectedNode {
    info: RemoteNodeInfo,
    out_tx: mpsc::UnboundedSender<NodeStreamMsg>,
    in_tx: mpsc::UnboundedSender<String>,
    close_tx: watch::Sender<bool>,
    connection_id: u64,
}

/// Tracks online remote nodes, bridges [`RemoteNode`] sessions, and matches in-flight RPC replies.
#[derive(Default)]
pub struct RemoteNodes {
    nodes: RwLock<HashMap<String, ConnectedNode>>,
    slots: RwLock<HashMap<String, Arc<NodeSlot>>>,
    pending: RwLock<HashMap<String, oneshot::Sender<NodeRpcReply>>>,
    next_conn: std::sync::atomic::AtomicU64,
    bus: std::sync::OnceLock<Weak<EventBus>>,
}

/// Guard returned by [`RemoteNodes::register`]; dropping it marks the node offline and closes its
/// transport (unless a newer connection for the same device already replaced it).
pub struct NodeRegistration {
    registry: Arc<RemoteNodes>,
    device_id: String,
    connection_id: u64,
    close_tx: watch::Sender<bool>,
}

impl Drop for NodeRegistration {
    fn drop(&mut self) {
        let _ = self.close_tx.send(true);
        let registry = self.registry.clone();
        let device_id = self.device_id.clone();
        let connection_id = self.connection_id;
        tokio::spawn(async move {
            let removed = {
                let mut nodes = registry.nodes.write().await;
                if nodes
                    .get(&device_id)
                    .is_some_and(|n| n.connection_id == connection_id)
                {
                    nodes.remove(&device_id);
                    true
                } else {
                    false
                }
            };
            if removed {
                tracing::info!(node = %device_id, "remote node disconnected");
                registry.notify_changed(&device_id).await;
            }
        });
    }
}

impl RemoteNodes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bus(bus: Weak<EventBus>) -> Self {
        let this = Self::default();
        let _ = this.bus.set(bus);
        this
    }

    async fn notify_changed(&self, device_id: &str) {
        if let Some(bus) = self.bus.get().and_then(Weak::upgrade) {
            let _ = bus
                .publish(EventKind::NodeChanged {
                    id: device_id.to_owned(),
                })
                .await;
        }
    }

    async fn slot_for(&self, device_id: &str) -> Arc<NodeSlot> {
        if let Some(slot) = self.slots.read().await.get(device_id).cloned() {
            return slot;
        }
        let mut slots = self.slots.write().await;
        slots
            .entry(device_id.to_owned())
            .or_insert_with(|| {
                let (accept_tx, accepted) = mpsc::unbounded_channel();
                let link = Arc::new(InboundHubLink {
                    accepted: Mutex::new(accepted),
                });
                let remote = Arc::new(RemoteNode::new(link));
                Arc::new(NodeSlot { remote, accept_tx })
            })
            .clone()
    }

    /// Registers a newly connected remote node, hooks up its [`RemoteNode`] transport, and returns
    /// its outbound SSE message receiver plus a drop guard that unregisters it on disconnect.
    pub async fn register(
        self: &Arc<Self>,
        mut info: RemoteNodeInfo,
    ) -> (mpsc::UnboundedReceiver<NodeStreamMsg>, NodeRegistration) {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (close_tx, close_rx) = watch::channel(false);
        let connection_id = self
            .next_conn
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let device_id = info.id.clone();
        info.online = true;

        let slot = self.slot_for(&device_id).await;
        if let Some(reg) = slot.remote.register() {
            info.plugins = snapshots_to_api(&reg.plugins);
        }

        tracing::info!(
            node = %info.name,
            id = %device_id,
            os = %info.os,
            hostname = %info.hostname,
            "remote node connected"
        );
        {
            let mut nodes = self.nodes.write().await;
            if let Some(prev) = nodes.insert(
                device_id.clone(),
                ConnectedNode {
                    info,
                    out_tx: out_tx.clone(),
                    in_tx,
                    close_tx: close_tx.clone(),
                    connection_id,
                },
            ) {
                let _ = prev.close_tx.send(true);
            }
        }

        let transport: Arc<dyn Transport> = Arc::new(HttpHubTransport {
            out_tx,
            in_rx: Mutex::new(in_rx),
            closed: close_rx.clone(),
        });
        let _ = slot.accept_tx.send(transport);

        // Watch for `Register` frames arriving on this connection so `GET /v1/nodes` and the GUI
        // learn the node's installed plugins/models as soon as it finishes handshaking.
        let registry = self.clone();
        let watch_id = device_id.clone();
        let mut reg_rx = slot.remote.watch_register();
        let mut closed_watch = close_rx;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    closed = closed_watch.changed() => {
                        if closed.is_err() || *closed_watch.borrow() {
                            break;
                        }
                    }
                    changed = reg_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let reg = reg_rx.borrow().clone();
                        if let Some(reg) = reg {
                            let plugins = snapshots_to_api(&reg.plugins);
                            let updated = {
                                let mut nodes = registry.nodes.write().await;
                                if let Some(conn) = nodes.get_mut(&watch_id)
                                    && conn.connection_id == connection_id
                                {
                                    conn.info.plugins = plugins;
                                    if !reg.hostname.trim().is_empty() {
                                        conn.info.hostname = reg.hostname.clone();
                                    }
                                    true
                                } else {
                                    false
                                }
                            };
                            if updated {
                                registry.notify_changed(&watch_id).await;
                            }
                        }
                    }
                }
            }
        });

        self.notify_changed(&device_id).await;

        (
            out_rx,
            NodeRegistration {
                registry: self.clone(),
                device_id,
                connection_id,
                close_tx,
            },
        )
    }

    /// Feeds one or more newline-delimited `ToHub` JSON frames posted by a remote node.
    pub async fn feed_frames(&self, device_id: &str, body: &str) -> bool {
        let tx = {
            let nodes = self.nodes.read().await;
            nodes.get(device_id).map(|n| n.in_tx.clone())
        };
        let Some(tx) = tx else {
            return false;
        };
        for raw in body.lines() {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut line = trimmed.to_owned();
            line.push('\n');
            if tx.send(line).is_err() {
                return false;
            }
        }
        true
    }

    /// Finds an online remote node by id, name, or hostname (case-insensitively).
    pub async fn resolve_online(&self, key: &str) -> Option<RemoteNodeInfo> {
        let key = key.trim();
        if key.is_empty() || key.eq_ignore_ascii_case("local") {
            return None;
        }
        let nodes = self.nodes.read().await;
        let found = nodes.get(key).or_else(|| {
            nodes.values().find(|n| {
                n.info.name.eq_ignore_ascii_case(key) || n.info.hostname.eq_ignore_ascii_case(key)
            })
        })?;
        let mut info = found.info.clone();
        if info.plugins.is_empty()
            && let Some(slot) = self.slots.read().await.get(&info.id)
            && let Some(reg) = slot.remote.register()
        {
            info.plugins = snapshots_to_api(&reg.plugins);
        }
        Some(info)
    }

    /// Returns a [`Driver`] executing on the connected remote node `node_key`.
    pub async fn driver(
        &self,
        node_key: &str,
        plugin: &str,
        driver_override: Option<&str>,
    ) -> Result<Arc<dyn Driver>, CoreError> {
        let info = self.resolve_online(node_key).await.ok_or_else(|| {
            CoreError::Agent(format!(
                "节点 {node_key:?} 当前离线，无法启动或连接会话。请在该机器上运行 `crossagents node run` 后重试。"
            ))
        })?;
        let slot = self
            .slots
            .read()
            .await
            .get(&info.id)
            .cloned()
            .ok_or_else(|| {
                CoreError::Agent(format!("节点 {:?} 尚未建立会话通道", info.name))
            })?;

        // Give the initial `Register` frame a brief moment if the node just connected.
        if !slot.remote.is_online() {
            let mut rx = slot.remote.watch_register();
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                rx.wait_for(|reg| reg.is_some()),
            )
            .await;
        }

        let reg = slot.remote.register();
        let snapshot = reg
            .as_ref()
            .and_then(|r| r.plugins.iter().find(|p| p.name == plugin).cloned())
            .unwrap_or_else(|| PluginSnapshot {
                name: plugin.to_owned(),
                installed: true,
                version: None,
                driver: driver_override.map(str::to_owned),
                models: Vec::new(),
                efforts: Vec::new(),
                efforts_by_model: Default::default(),
                capabilities: None,
            });
        let kind = driver_override
            .and_then(|d| DriverKind::from_str(d).ok())
            .or_else(|| snapshot.driver.as_deref().and_then(|d| DriverKind::from_str(d).ok()));
        let driver_info = Arc::new(SnapshotDriverInfo::new(snapshot));
        Ok(Arc::new(slot.remote.driver(plugin, kind, driver_info)))
    }

    /// Returns `PluginInfo` entries reported by a connected remote node (empty if offline).
    pub async fn remote_plugins(&self, node_key: &str) -> Result<Vec<PluginInfo>, CoreError> {
        let Some(info) = self.resolve_online(node_key).await else {
            return Ok(Vec::new());
        };
        let Some(slot) = self.slots.read().await.get(&info.id).cloned() else {
            return Ok(Vec::new());
        };
        let Some(reg) = slot.remote.register() else {
            return Ok(Vec::new());
        };
        Ok(reg
            .plugins
            .into_iter()
            .map(snapshot_to_plugin_info)
            .collect())
    }

    /// Lists all known nodes: the hub's own `"local"` machine first, then connected remote nodes,
    /// then any non-revoked offline `DeviceKind::Node` devices from the store.
    pub async fn list(
        &self,
        store: &dyn Store,
        plugins: &PluginRegistry,
        started_at: DateTime<Utc>,
        version: &str,
    ) -> Vec<RemoteNodeInfo> {
        let mut out = vec![local_node_info(plugins, started_at, version)];
        let connected: HashMap<String, RemoteNodeInfo> = {
            let nodes = self.nodes.read().await;
            let slots = self.slots.read().await;
            nodes
                .iter()
                .map(|(id, n)| {
                    let mut info = n.info.clone();
                    if info.plugins.is_empty()
                        && let Some(slot) = slots.get(id)
                        && let Some(reg) = slot.remote.register()
                    {
                        info.plugins = snapshots_to_api(&reg.plugins);
                    }
                    (id.clone(), info)
                })
                .collect()
        };

        let mut remote: Vec<RemoteNodeInfo> = connected.values().cloned().collect();
        let known_slots: std::collections::HashSet<String> = {
            let slots = self.slots.read().await;
            slots.keys().cloned().collect()
        };
        let agent_nodes: std::collections::HashSet<String> = store
            .list_agents()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.archived_at.is_none())
            .filter_map(|a| a.node)
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty() && n != "local")
            .collect();
        if let Ok(devices) = store.list_devices().await {
            for dev in devices {
                if dev.revoked_at.is_some() || connected.contains_key(&dev.id) {
                    continue;
                }
                let is_node_device = dev.kind == DeviceKind::Node
                    || known_slots.contains(&dev.id)
                    || agent_nodes.contains(&dev.id.to_ascii_lowercase())
                    || agent_nodes.contains(&dev.name.to_ascii_lowercase());
                if !is_node_device {
                    continue;
                }
                remote.push(RemoteNodeInfo {
                    id: dev.id,
                    name: dev.name.clone(),
                    os: dev
                        .os
                        .map(|o| match o {
                            crossagents_core::DeviceOs::Windows => "windows",
                            crossagents_core::DeviceOs::Macos => "macos",
                            crossagents_core::DeviceOs::Linux => "linux",
                            crossagents_core::DeviceOs::Other => "unknown",
                        })
                        .unwrap_or("unknown")
                        .into(),
                    arch: String::new(),
                    hostname: dev.name,
                    shell: String::new(),
                    default_cwd: String::new(),
                    chats_dir: None,
                    version: String::new(),
                    connected_at: dev.last_seen_at.unwrap_or(dev.created_at),
                    local: false,
                    online: false,
                    plugins: Vec::new(),
                });
            }
        }
        remote.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
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
                Some(n) => (n.info.name.clone(), n.out_tx.clone()),
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

        let send_res = tx.send(NodeStreamMsg::Call(NodeRpcCall {
            call_id: call_id.clone(),
            op,
        }));
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

fn snapshots_to_api(snapshots: &[PluginSnapshot]) -> Vec<NodePluginInfo> {
    snapshots
        .iter()
        .map(|p| NodePluginInfo {
            name: p.name.clone(),
            installed: p.installed,
            version: p.version.clone(),
            driver: p.driver.clone(),
            models: p.models.clone(),
            efforts: p.efforts.clone(),
            efforts_by_model: p.efforts_by_model.clone(),
        })
        .collect()
}

fn snapshot_to_plugin_info(p: PluginSnapshot) -> PluginInfo {
    let driver = p
        .driver
        .as_deref()
        .and_then(|d| DriverKind::from_str(d).ok())
        .unwrap_or(DriverKind::AcpGeneric);
    let capabilities = p.capabilities.unwrap_or(Capabilities {
        resume: true,
        interrupt: true,
        set_model: true,
        ..Capabilities::default()
    });
    let models = if p.installed {
        p.models.clone()
    } else {
        Vec::new()
    };
    PluginInfo {
        name: p.name,
        models,
        all_models: p.models,
        shown: None,
        models_pinned: false,
        driver,
        capabilities,
        efforts: p.efforts,
        efforts_by_model: p.efforts_by_model,
        cli: Some(CliInfo {
            installed: p.installed,
            probing: false,
            path: None,
            source: None,
            version: p.version,
            install_hint: None,
            docs: None,
            installed_via: None,
            install_command: None,
            upgrade_command: None,
            can_upgrade: false,
            installing: None,
            described_by: Some("remote-node".into()),
            min_version: None,
            below_minimum: false,
            verified: None,
            unverified: false,
            version_status: None,
            last_ok_at: None,
            last_protocol_fail_at: None,
            protocol_fail_message: None,
            last_good_version: None,
            self_checking: false,
        }),
    }
}

fn local_node_info(
    plugins: &PluginRegistry,
    started_at: DateTime<Utc>,
    version: &str,
) -> RemoteNodeInfo {
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
    let local_plugins: Vec<NodePluginInfo> = plugins
        .defaults()
        .into_iter()
        .map(|d| {
            let name = d.name().to_owned();
            let models = if d.installed() {
                d.models()
            } else {
                Vec::new()
            };
            let efforts = plugins
                .config(&name)
                .map(|c| c.efforts.clone())
                .unwrap_or_default();
            let efforts_by_model = models
                .iter()
                .filter_map(|m| d.model_efforts(m).map(|e| (m.clone(), e)))
                .collect();
            NodePluginInfo {
                name,
                installed: d.installed(),
                version: None,
                driver: Some(d.kind().as_str().to_owned()),
                models,
                efforts,
                efforts_by_model,
            }
        })
        .collect();
    RemoteNodeInfo {
        id: "local".into(),
        name: "local".into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        hostname,
        shell,
        default_cwd,
        chats_dir: None,
        version: version.into(),
        connected_at: started_at,
        local: true,
        online: true,
        plugins: local_plugins,
    }
}
