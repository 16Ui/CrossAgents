use std::{fs, io::Write, path::PathBuf, process::Stdio, sync::Mutex, time::Duration};

use regex::Regex;
use reqwest::{Client, RequestBuilder, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State, ipc::Channel};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

use crate::Hub;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    host: String,
    user: Option<String>,
    ssh_port: Option<u16>,
    command: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "via", rename_all = "lowercase")]
enum Route {
    Ssh,
    Link { link: Link },
}

#[derive(Clone, Deserialize, Serialize)]
struct Link {
    endpoint: String,
    relay: Option<String>,
    #[serde(default)]
    #[serde(alias = "customRelay")]
    custom_relay: bool,
    #[serde(default)]
    addrs: Vec<std::net::SocketAddr>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Pairing {
    target: Target,
    #[serde(flatten)]
    route: Route,
    device_name: String,
    device_id: String,
    paired_at: String,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Deserialize, Serialize)]
struct Saved {
    remote: bool,
    pairing: Option<Pairing>,
    #[serde(default = "default_true")]
    share_node: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            remote: false,
            pairing: None,
            share_node: true,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Tunnel {
    status: &'static str,
    local_port: Option<u16>,
    remote_port: Option<u16>,
    reason: Option<String>,
    stopped: bool,
}

impl Tunnel {
    fn connecting() -> Self {
        Self {
            status: "connecting",
            local_port: None,
            remote_port: None,
            reason: None,
            stopped: false,
        }
    }

    fn offline(reason: String, stopped: bool) -> Self {
        Self {
            status: "offline",
            reason: Some(reason),
            stopped,
            ..Self::connecting()
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    mode: &'static str,
    bundled_hub: bool,
    default_device_name: String,
    remote: Option<Pairing>,
    tunnel: Option<Tunnel>,
    share_node: bool,
    node_status: Option<String>,
}

#[derive(Default)]
struct Runtime {
    saved: Saved,
    tunnel: Option<Tunnel>,
    node_status: Option<String>,
    token: Option<String>,
    generation: u64,
    worker: Option<tauri::async_runtime::JoinHandle<()>>,
}

pub struct Remote {
    file: PathBuf,
    state: Mutex<Runtime>,
    actions: tokio::sync::Mutex<()>,
}

impl Remote {
    pub fn new(app: &AppHandle) -> tauri::Result<Self> {
        let file = app.path().app_data_dir()?.join("remote-v1.json");
        let saved = fs::read(&file)
            .ok()
            .and_then(|text| serde_json::from_slice(&text).ok())
            .unwrap_or_default();
        Ok(Self {
            file,
            state: Mutex::new(Runtime {
                saved,
                ..Default::default()
            }),
            actions: Default::default(),
        })
    }

    pub fn start(app: &AppHandle) {
        let remote = app.state::<Self>();
        let mut state = remote.state.lock().expect("remote state");
        state.generation += 1;
        if let Some(worker) = state.worker.take() {
            worker.abort();
        }
        state.token = None;
        state.tunnel = None;
        state.node_status = None;
        if let Some(pairing) = state
            .saved
            .pairing
            .as_ref()
            .filter(|_| state.saved.remote)
            .cloned()
        {
            let share_node = state.saved.share_node;
            state.tunnel = Some(Tunnel::connecting());
            if !share_node {
                state.node_status = Some("disabled".into());
            }
            let generation = state.generation;
            let app = app.clone();
            state.worker = Some(tauri::async_runtime::spawn(async move {
                supervise(app, pairing, share_node, generation).await;
            }));
        }
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().expect("remote state");
        state.generation += 1;
        if let Some(worker) = state.worker.take() {
            worker.abort();
        }
        state.token = None;
        state.tunnel = None;
        state.node_status = None;
    }

    pub fn is_remote(&self) -> bool {
        self.state.lock().expect("remote state").saved.remote
    }

    pub fn remote_summary(&self) -> Option<(String, bool)> {
        let state = self.state.lock().expect("remote state");
        if !state.saved.remote {
            return None;
        }
        let target = state
            .saved
            .pairing
            .as_ref()
            .map(|pairing| pairing.target.host.clone())
            .filter(|host| !host.is_empty())
            .unwrap_or_else(|| "远程".into());
        let connected = state
            .tunnel
            .as_ref()
            .is_some_and(|tunnel| tunnel.status == "connected");
        Some((target, connected))
    }

    pub fn workspace_key(&self) -> Option<String> {
        let state = self.state.lock().expect("remote state");
        if state.saved.remote {
            return state
                .saved
                .pairing
                .as_ref()
                .map(|pairing| format!("device:{}", pairing.device_id));
        }
        crate::hub_home()
            .ok()
            .map(|home| format!("local:{}", home.to_string_lossy()))
    }

    fn status(&self) -> Connection {
        let state = self.state.lock().expect("remote state");
        Connection {
            mode: if state.saved.remote {
                "remote"
            } else {
                "local"
            },
            bundled_hub: true,
            default_device_name: default_device_name(),
            remote: state.saved.pairing.clone(),
            tunnel: state.tunnel.clone(),
            share_node: state.saved.share_node,
            node_status: state.node_status.clone(),
        }
    }

    pub async fn hub(&self) -> Result<Hub, String> {
        {
            let state = self.state.lock().expect("remote state");
            if state.saved.remote {
                if let (Some(token), Some(port)) = (
                    &state.token,
                    state.tunnel.as_ref().and_then(|tunnel| tunnel.local_port),
                ) {
                    return Ok(Hub {
                        base: loopback(port),
                        token: token.clone(),
                        remote_generation: Some(state.generation),
                    });
                }
                return Err(state
                    .tunnel
                    .as_ref()
                    .and_then(|tunnel| tunnel.reason.clone())
                    .unwrap_or_else(|| "正在连接远程中心".into()));
            }
        }
        Hub::discover().await
    }

    pub fn unauthorized(&self, hub: &Hub) {
        let mut state = self.state.lock().expect("remote state");
        if hub.remote_generation == Some(state.generation) {
            state.generation += 1;
            if let Some(worker) = state.worker.take() {
                worker.abort();
            }
            state.token = None;
            state.tunnel = Some(Tunnel::offline("设备已撤销，请重新配对".into(), true));
        }
    }

    fn replace(&self, saved: Saved) -> Result<(), String> {
        let persist = || -> Result<(), Box<dyn std::error::Error>> {
            let parent = self.file.parent().ok_or("missing app data directory")?;
            fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            serde_json::to_writer(&mut file, &saved)?;
            file.flush()?;
            file.persist(&self.file)?;
            Ok(())
        };
        persist().map_err(|err| format!("无法保存配对设置：{err}"))?;
        self.state.lock().expect("remote state").saved = saved;
        Ok(())
    }
}

#[derive(Deserialize)]
struct Device {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct Issued {
    device: Device,
    token: String,
}

#[derive(Deserialize)]
struct Redeemed {
    #[serde(flatten)]
    issued: Issued,
    #[serde(flatten)]
    link: Link,
    host: String,
}

fn matches(pattern: &str, value: &str) -> bool {
    Regex::new(pattern)
        .expect("valid connection validation pattern")
        .is_match(value)
}

fn device_name(name: &str) -> Result<(), String> {
    if matches(r"\A[A-Za-z0-9][A-Za-z0-9._-]{0,63}\z", name) {
        Ok(())
    } else {
        Err("设备名无效".into())
    }
}

fn default_device_name() -> String {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    if raw.trim().is_empty() {
        raw = std::process::Command::new("scutil")
            .args(["--get", "LocalHostName"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
    }
    #[cfg(unix)]
    if raw.trim().is_empty() {
        raw = std::process::Command::new("hostname")
            .arg("-s")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
    }
    let name: String = raw
        .trim()
        .strip_suffix(".local")
        .unwrap_or(raw.trim())
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || "._-".contains(character) {
                character
            } else {
                '-'
            }
        })
        .take(58)
        .collect();
    let name = name.trim_matches(['.', '_', '-']);
    if name.is_empty() {
        if cfg!(target_os = "macos") {
            "Mac".into()
        } else {
            "CrossAgents".into()
        }
    } else {
        name.to_owned()
    }
}

fn validate_target(target: &Target) -> Result<(), String> {
    let name = |text: &str| matches(r"\A[A-Za-z0-9._][A-Za-z0-9._-]*\z", text);
    if !name(&target.host) && !matches(r"\A\[[0-9A-Fa-f:.]+(%[A-Za-z0-9._-]+)?\]\z", &target.host) {
        return Err("SSH 主机名无效".into());
    }
    if target.user.as_deref().is_some_and(|user| !name(user))
        || target.ssh_port == Some(0)
        || target
            .command
            .as_deref()
            .is_some_and(|command| !safe_command(command))
    {
        return Err("SSH 用户名、端口或命令路径无效".into());
    }
    Ok(())
}

fn safe_command(command: &str) -> bool {
    !command.trim().is_empty()
        && !command.contains([
            '"', '$', '`', '%', '!', '&', '|', ';', '<', '>', '^', '(', ')', '@', '\0', '\r', '\n',
        ])
}

fn credential(id: &str) -> Result<keyring::Entry, String> {
    if !cfg!(any(windows, target_os = "macos")) {
        return Err("当前系统没有配置配对凭据存储".into());
    }
    keyring::Entry::new("dev.crossagents.tauri.remote", id).map_err(|err| err.to_string())
}

fn loopback(port: u16) -> Url {
    Url::parse(&format!("http://127.0.0.1:{port}")).expect("valid loopback port")
}

pub(crate) fn client_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else {
        std::env::consts::OS
    }
}

pub(crate) fn client_headers(request: RequestBuilder, hub: &Hub) -> RequestBuilder {
    if hub.remote_generation.is_some() {
        request.header("x-crossagents-client-os", client_os())
    } else {
        request
    }
}

fn hidden(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command.kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}

#[tauri::command]
pub async fn tailscale_status() -> serde_json::Value {
    use serde_json::{Value, json};
    let mut programs = vec!["tailscale"];
    if cfg!(windows) {
        programs.push(r"C:\Program Files\Tailscale\tailscale.exe");
    }
    if cfg!(target_os = "macos") {
        programs.push("/Applications/Tailscale.app/Contents/MacOS/Tailscale");
    }
    let peer = |node: &Value| {
        let os = match node["OS"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "darwin" | "macos" => "macos",
            "windows" => "windows",
            "linux" => "linux",
            "ios" => "ios",
            "android" => "android",
            _ => "other",
        };
        let ips: Vec<_> = node["TailscaleIPs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        json!({
            "hostName": node["HostName"].as_str().unwrap_or_default(),
            "dnsName": node["DNSName"].as_str().map(|name| name.trim_end_matches('.')),
            "ips": ips, "os": os, "online": node["Online"] == true,
            "lastSeen": node["LastSeen"].as_str().filter(|value| !value.starts_with("0001-01-01")),
        })
    };
    for program in programs {
        let mut command = hidden(program);
        command.args(["status", "--json"]).stdin(Stdio::null());
        match tokio::time::timeout(Duration::from_secs(5), command.output()).await {
            Err(_) => {
                return json!({"state":"stopped","peers":[],"error":"tailscale status 超过 5 秒没有回应"});
            }
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Ok(Err(err)) => return json!({"state":"stopped","peers":[],"error":err.to_string()}),
            Ok(Ok(output)) => {
                let text = String::from_utf8_lossy(&output.stdout);
                let value: Value =
                    serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
                let Some(backend) = value["BackendState"].as_str() else {
                    let message = String::from_utf8_lossy(&output.stderr);
                    return json!({"state":"stopped","peers":[],"error":message.trim().chars().take(300).collect::<String>()});
                };
                let state = match backend {
                    "Running" => "running",
                    "NeedsLogin" => "needs-login",
                    _ => "stopped",
                };
                let peers: Vec<_> = value["Peer"]
                    .as_object()
                    .into_iter()
                    .flat_map(|nodes| nodes.values())
                    .filter(|node| node.is_object())
                    .map(peer)
                    .collect();
                return json!({"state":state,"self":value["Self"].is_object().then(|| peer(&value["Self"])),"peers":peers});
            }
        }
    }
    json!({"state":"missing","peers":[]})
}

fn link_program() -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "crossagents-link.exe"
    } else {
        "crossagents-link"
    };
    let path = std::env::current_exe()
        .map_err(|err| err.to_string())?
        .with_file_name(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err("此版本未随附 crossagents-link，请使用安装包".into())
    }
}

fn crossagents_program() -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "crossagents.exe"
    } else {
        "crossagents"
    };
    let path = std::env::current_exe()
        .map_err(|err| err.to_string())?
        .with_file_name(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err("此版本未随附 crossagents 执行节点程序".into())
    }
}

#[cfg(unix)]
async fn unix_enriched_path() -> Option<String> {
    use std::sync::OnceLock;
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    if let Some(cached) = CACHED.get() {
        return cached.clone();
    }
    const START: &str = "__CA_PATH_START__";
    const END: &str = "__CA_PATH_END__";
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let script = format!("printf '\\n{START}%s{END}\\n' \"$PATH\"");
    let mut shell_path: Option<String> = None;
    for flag in ["-lic", "-lc"] {
        let mut cmd = hidden(&shell);
        cmd.args([flag, &script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(3), cmd.output()).await
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(after) = text.split(START).nth(1)
                && let Some(raw) = after.split(END).next()
            {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    shell_path = Some(trimmed.to_owned());
                    break;
                }
            }
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !p.as_os_str().is_empty() && !dirs.iter().any(|d| d == &p) {
            dirs.push(p);
        }
    };
    if let Some(raw) = shell_path {
        for p in std::env::split_paths(&raw) {
            push(p);
        }
    }
    if let Some(current) = std::env::var_os("PATH") {
        for p in std::env::split_paths(&current) {
            push(p);
        }
    }
    for fixed in [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
    ] {
        let p = PathBuf::from(fixed);
        if p.is_dir() {
            push(p);
        }
    }
    if let Some(home) = dirs::home_dir() {
        for rel in [
            ".local/bin",
            ".cargo/bin",
            ".volta/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".pnpm",
            "Library/pnpm",
            ".asdf/shims",
            ".mise/shims",
            ".opencode/bin",
        ] {
            let p = home.join(rel);
            if p.is_dir() {
                push(p);
            }
        }
        for (root, suffix) in [
            (home.join(".nvm/versions/node"), "bin"),
            (home.join(".fnm/node-versions"), "installation/bin"),
        ] {
            if let Ok(entries) = fs::read_dir(&root) {
                let mut versions: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path().join(suffix))
                    .filter(|p| p.is_dir())
                    .collect();
                versions.sort();
                versions.reverse();
                for p in versions {
                    push(p);
                }
            }
        }
    }
    let joined = std::env::join_paths(dirs)
        .ok()
        .and_then(|os| os.into_string().ok())
        .filter(|s| !s.is_empty());
    let _ = CACHED.set(joined.clone());
    joined
}

#[cfg(not(unix))]
async fn unix_enriched_path() -> Option<String> {
    None
}

fn ssh() -> Command {
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        let path = PathBuf::from(root).join("System32/OpenSSH/ssh.exe");
        if path.is_file() {
            return hidden(path);
        }
    }
    hidden("ssh")
}

fn destination(command: &mut Command, target: &Target) {
    if let Some(user) = &target.user {
        command.args(["-l", user]);
    }
    if let Some(port) = target.ssh_port {
        command.args(["-p", &port.to_string()]);
    }
    command.args(["--", target.host.trim_matches(['[', ']'])]);
}

async fn run(mut command: Command, input: Option<&str>) -> Result<String, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn().map_err(|err| err.to_string())?;
    if let Some(input) = input {
        feed(&mut child, input).await?;
    }
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| "连接命令等待超时".to_owned())?
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().chars().take(1200).collect::<String>());
    }
    String::from_utf8(output.stdout).map_err(|err| err.to_string())
}

async fn feed(child: &mut Child, input: &str) -> Result<(), String> {
    let mut stdin = child.stdin.take().ok_or("连接工具没有输入通道")?;
    stdin
        .write_all(format!("{input}\n").as_bytes())
        .await
        .map_err(|err| err.to_string())
}

async fn ssh_exec(target: &Target, script: &str) -> Result<String, String> {
    let mut command = ssh();
    command.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
    destination(&mut command, target);
    command.arg(script);
    run(command, None).await
}

#[derive(Deserialize)]
struct SshHub {
    port: u16,
    exe: Option<String>,
}

async fn locate(target: &Target) -> Result<(SshHub, bool), String> {
    let mut last = String::new();
    for (script, cmd) in [
        ("cat ~/.crossagents/hub.json", false),
        (r#"type "%USERPROFILE%\.crossagents\hub.json""#, true),
    ] {
        match ssh_exec(target, script).await {
            Ok(text) => {
                if let Ok(hub) =
                    serde_json::from_str::<SshHub>(text.trim_start_matches('\u{feff}').trim())
                    && hub.port > 0
                {
                    return Ok((hub, cmd));
                }
            }
            Err(err) => last = err,
        }
    }
    Err(format!("找不到远程 Hub：{last}"))
}

enum Failure {
    Offline(String),
    Revoked,
}

impl From<String> for Failure {
    fn from(error: String) -> Self {
        Self::Offline(error)
    }
}

async fn connect(pairing: &Pairing, token: &str) -> Result<(Child, u16, Option<u16>), Failure> {
    match &pairing.route {
        Route::Ssh => {
            validate_target(&pairing.target)?;
            let local = std::net::TcpListener::bind("127.0.0.1:0")
                .map_err(|err| err.to_string())?
                .local_addr()
                .map_err(|err| err.to_string())?
                .port();
            let (hub, _) = locate(&pairing.target).await?;
            let mut command = ssh();
            command.args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-N",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-L",
                &format!("127.0.0.1:{local}:127.0.0.1:{}", hub.port),
            ]);
            destination(&mut command, &pairing.target);
            let child = command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|err| err.to_string())?;
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok((child, local, Some(hub.port)))
        }
        Route::Link { link } => {
            let mut command = hidden(link_program()?);
            command.args([
                "connect",
                "--endpoint",
                &link.endpoint,
                "--listen",
                "127.0.0.1:0",
            ]);
            if let Some(relay) = &link.relay {
                command.args(["--relay", relay]);
                if link.custom_relay {
                    command.arg("--custom-relay");
                }
            }
            for address in link.addrs.iter().take(16) {
                command.args(["--addr", &address.to_string()]);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|err| err.to_string())?;
            feed(&mut child, token).await?;
            let stdout = child
                .stdout
                .take()
                .ok_or("连接工具没有输出通道".to_owned())?;
            let mut lines = BufReader::new(stdout).lines();
            let port = tokio::time::timeout(Duration::from_secs(15), async {
                while let Some(line) = lines.next_line().await.map_err(|err| err.to_string())? {
                    let event: serde_json::Value =
                        serde_json::from_str(&line).map_err(|err| err.to_string())?;
                    if event["event"] == "listening" {
                        return event["port"]
                            .as_u64()
                            .and_then(|port| u16::try_from(port).ok())
                            .filter(|port| *port != 0)
                            .ok_or_else(|| "连接工具返回了无效的端口".to_owned());
                    }
                }
                Err("连接工具未建立转发".to_owned())
            })
            .await
            .map_err(|_| "等待配对隧道超时".to_owned())??;
            tauri::async_runtime::spawn(async move {
                while let Ok(Some(_)) = lines.next_line().await {}
            });
            Ok((child, port, None))
        }
    }
}

async fn probe(client: &Client, child: &mut Child, port: u16, token: &str) -> Result<(), Failure> {
    let url = loopback(port).join("/v1/info").expect("valid endpoint");
    for _ in 0..20 {
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Err(if status.code() == Some(3) {
                Failure::Revoked
            } else {
                Failure::Offline(format!("转发进程已退出：{status}"))
            });
        }
        if let Ok(response) = client
            .get(url.clone())
            .bearer_auth(token)
            .header("x-crossagents-client-os", client_os())
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            if response.status().is_success() {
                return Ok(());
            }
            if response.status() == StatusCode::UNAUTHORIZED {
                return Err(Failure::Revoked);
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Err(Failure::Offline("远程中心暂时没有回应".into()))
}

fn update(app: &AppHandle, generation: u64, tunnel: Tunnel, token: Option<String>) -> bool {
    let remote = app.state::<Remote>();
    let mut state = remote.state.lock().expect("remote state");
    if state.generation != generation {
        return false;
    }
    state.tunnel = Some(tunnel);
    state.token = token;
    true
}

fn update_node_status(app: &AppHandle, generation: u64, status: Option<String>) -> bool {
    let remote = app.state::<Remote>();
    let mut state = remote.state.lock().expect("remote state");
    if state.generation != generation {
        return false;
    }
    state.node_status = status;
    true
}

struct AbortOnDrop(Option<tauri::async_runtime::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

async fn supervise_local_node(app: AppHandle, generation: u64, port: u16, token: String) {
    let program = match crossagents_program() {
        Ok(path) => path,
        Err(_) => {
            update_node_status(&app, generation, Some("missing".into()));
            return;
        }
    };
    let cwd = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let enriched_path = unix_enriched_path().await;
    let hub_url = format!("http://127.0.0.1:{port}");
    let mut delay = 3u64;
    loop {
        if !update_node_status(&app, generation, Some("connecting".into())) {
            return;
        }
        let mut command = hidden(&program);
        command
            .args(["node", "run", "--hub", &hub_url, "--cwd"])
            .arg(&cwd)
            .env("CROSSAGENTS_NODE_TOKEN", &token)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(path) = &enriched_path {
            command.env("PATH", path);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                update_node_status(&app, generation, Some(format!("error: {err}")));
                return;
            }
        };
        let mut rejected = false;
        let mut last_err = String::new();
        if let Some(stderr) = child.stderr.take() {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.contains("node connected to ") {
                    delay = 3;
                    if !update_node_status(&app, generation, Some("connected".into())) {
                        return;
                    }
                } else if line.contains("disconnected from hub") {
                    if !update_node_status(&app, generation, Some("connecting".into())) {
                        return;
                    }
                } else if line.contains("hub rejected node token") {
                    rejected = true;
                    last_err = line;
                } else if !line.trim().is_empty() {
                    last_err = line;
                }
            }
        }
        let _ = child.wait().await;
        if rejected {
            let status = if last_err.contains("403") {
                "unsupported".to_owned()
            } else {
                "revoked".to_owned()
            };
            update_node_status(&app, generation, Some(status));
            return;
        }
        if !update_node_status(&app, generation, Some("connecting".into())) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}

async fn supervise(app: AppHandle, pairing: Pairing, share_node: bool, generation: u64) {
    let token = tauri::async_runtime::spawn_blocking({
        let id = pairing.device_id.clone();
        move || {
            credential(&id)?
                .get_password()
                .map_err(|err| err.to_string())
        }
    })
    .await
    .ok()
    .and_then(Result::ok);
    let Some(token) = token else {
        update(
            &app,
            generation,
            Tunnel::offline("无法读取系统凭据，请重新配对".into(), true),
            None,
        );
        return;
    };
    let client = app.state::<Client>().inner().clone();
    let mut delay = 1;
    loop {
        if !update(&app, generation, Tunnel::connecting(), None) {
            return;
        }
        update_node_status(
            &app,
            generation,
            if share_node {
                Some("connecting".into())
            } else {
                Some("disabled".into())
            },
        );
        let result = async {
            let (mut child, port, upstream) = connect(&pairing, &token).await?;
            probe(&client, &mut child, port, &token).await?;
            delay = 1;
            if !update(
                &app,
                generation,
                Tunnel {
                    status: "connected",
                    local_port: Some(port),
                    remote_port: upstream,
                    reason: None,
                    stopped: false,
                },
                Some(token.clone()),
            ) {
                return Ok(());
            }
            let _node_guard = AbortOnDrop(share_node.then(|| {
                tauri::async_runtime::spawn(supervise_local_node(
                    app.clone(),
                    generation,
                    port,
                    token.clone(),
                ))
            }));
            let exit = child.wait().await.map_err(|err| err.to_string())?;
            if matches!(pairing.route, Route::Link { .. }) && exit.code() == Some(3) {
                Err(Failure::Revoked)
            } else {
                Err(Failure::Offline(format!("隧道已断开：{exit}")))
            }
        }
        .await;
        match result {
            Ok(()) => return,
            Err(Failure::Revoked) => {
                update_node_status(&app, generation, None);
                update(
                    &app,
                    generation,
                    Tunnel::offline("设备已撤销，请重新配对".into(), true),
                    None,
                );
                return;
            }
            Err(Failure::Offline(reason)) => {
                update_node_status(
                    &app,
                    generation,
                    if share_node {
                        Some("offline".into())
                    } else {
                        Some("disabled".into())
                    },
                );
                if !update(&app, generation, Tunnel::offline(reason, false), None) {
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}

async fn commit_pair(
    app: &AppHandle,
    remote: &Remote,
    target: Target,
    route: Route,
    issued: Issued,
    steps: &Channel<String>,
) -> Result<Connection, String> {
    device_name(&issued.device.id)?;
    device_name(&issued.device.name)?;
    if !matches(r"\A[A-Za-z0-9_.-]{8,256}\z", &issued.token) {
        return Err("中心没有提供有效的设备凭据".into());
    }
    let pairing = Pairing {
        target,
        route,
        device_name: issued.device.name,
        device_id: issued.device.id,
        paired_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| err.to_string())?,
    };
    let _ = steps.send("check".into());
    store_pairing(app, remote, pairing, issued.token, true).await
}

async fn store_pairing(
    app: &AppHandle,
    remote: &Remote,
    pairing: Pairing,
    token: String,
    active: bool,
) -> Result<Connection, String> {
    let (mut child, port, _) = connect(&pairing, &token).await.map_err(describe_failure)?;
    probe(&app.state::<Client>(), &mut child, port, &token)
        .await
        .map_err(describe_failure)?;
    drop(child);
    let share_node = remote.state.lock().expect("remote state").saved.share_node;
    let saved = Saved {
        remote: active,
        pairing: Some(pairing.clone()),
        share_node,
    };
    let previous = remote
        .state
        .lock()
        .expect("remote state")
        .saved
        .pairing
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        credential(&pairing.device_id)?
            .set_password(&token)
            .map_err(|err| format!("无法安全保存设备凭据：{err}"))
    })
    .await
    .map_err(|err| err.to_string())??;
    remote.replace(saved)?;
    if let Some(previous) = previous.filter(|old| {
        remote
            .status()
            .remote
            .is_some_and(|next| next.device_id != old.device_id)
    }) {
        let _ = tauri::async_runtime::spawn_blocking(move || {
            credential(&previous.device_id)
                .and_then(|entry| entry.delete_credential().map_err(|err| err.to_string()))
        })
        .await;
    }
    Remote::start(app);
    Ok(remote.status())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPairing {
    mode: String,
    via: Option<String>,
    link: Option<Link>,
    target: Target,
    device_name: String,
    device_id: String,
    paired_at: String,
    token: String,
}

#[tauri::command]
pub async fn connection_import_pairing(
    app: AppHandle,
    remote: State<'_, Remote>,
    file: String,
    key: String,
) -> Result<Connection, String> {
    let _lock = remote.actions.lock().await;
    let old: LegacyPairing =
        crate::transfer::open(&file, &key, "crossagents-pairing-v1", 64 * 1024)?;
    validate_target(&old.target)?;
    device_name(&old.device_id)?;
    device_name(&old.device_name)?;
    if !matches(r"\A[A-Za-z0-9_.-]{8,256}\z", &old.token) {
        return Err("配对文件中的设备凭据无效".into());
    }
    let route = match old.via.as_deref() {
        None | Some("ssh") => Route::Ssh,
        Some("link") => Route::Link {
            link: old.link.ok_or("配对文件缺少 Link 地址")?,
        },
        _ => return Err("配对文件中的连接方式无效".into()),
    };
    let pairing = Pairing {
        target: old.target,
        route,
        device_name: old.device_name,
        device_id: old.device_id,
        paired_at: old.paired_at,
    };
    store_pairing(&app, &remote, pairing, old.token, old.mode == "remote").await
}

fn describe_failure(failure: Failure) -> String {
    match failure {
        Failure::Offline(reason) => reason,
        Failure::Revoked => "设备凭据无效或已撤销".into(),
    }
}

#[tauri::command]
pub fn connection_get(remote: State<'_, Remote>) -> Connection {
    remote.status()
}

#[tauri::command]
pub async fn connection_pair_code(
    app: AppHandle,
    remote: State<'_, Remote>,
    code: String,
    device_name: String,
    steps: Channel<String>,
) -> Result<Connection, String> {
    let _lock = remote.actions.lock().await;
    self::device_name(&device_name)?;
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if !matches(r"\Aca1-[A-Za-z0-9_-]{1,4096}\z", &code) {
        return Err("这不是有效的 CrossAgents 配对码".into());
    }
    let _ = steps.send("redeem".into());
    let mut command = hidden(link_program()?);
    command.args(["redeem", "--name", &device_name, "--os", client_os()]);
    let reply = run(command, Some(&code)).await?;
    let reply: Redeemed = serde_json::from_str(&reply).map_err(|_| "配对响应无效".to_owned())?;
    let target = Target {
        host: reply.host.chars().take(120).collect(),
        user: None,
        ssh_port: None,
        command: None,
    };
    let _ = steps.send("forward".into());
    commit_pair(
        &app,
        &remote,
        target,
        Route::Link { link: reply.link },
        reply.issued,
        &steps,
    )
    .await
}

#[tauri::command]
pub async fn connection_pair(
    app: AppHandle,
    remote: State<'_, Remote>,
    target: Target,
    device_name: String,
    steps: Channel<String>,
) -> Result<Connection, String> {
    let _lock = remote.actions.lock().await;
    validate_target(&target)?;
    self::device_name(&device_name)?;
    let _ = steps.send("locate".into());
    let (hub, cmd) = locate(&target).await?;
    let path = target
        .command
        .as_deref()
        .or_else(|| hub.exe.as_deref().filter(|path| safe_command(path)));
    let mut program = path.map_or("crossagents".into(), |path| format!("\"{path}\""));
    if !cmd
        && path.is_some_and(|path| matches(r"\A(?:[A-Za-z]:[\\/]|\\\\)", path))
        && ssh_exec(&target, "echo $PID")
            .await
            .is_ok_and(|value| value.trim().parse::<u32>().is_ok())
    {
        program = format!("& {program}");
    }
    let _ = steps.send("pair".into());
    let reply = ssh_exec(
        &target,
        &format!("{program} device add {device_name} --json"),
    )
    .await?;
    let issued = serde_json::from_str(reply.trim_start_matches('\u{feff}'))
        .map_err(|_| "设备配对响应无效".to_owned())?;
    let _ = steps.send("tunnel".into());
    commit_pair(&app, &remote, target, Route::Ssh, issued, &steps).await
}

#[tauri::command]
pub async fn connection_use(
    app: AppHandle,
    remote: State<'_, Remote>,
    mode: String,
) -> Result<Connection, String> {
    let _lock = remote.actions.lock().await;
    let mut saved = remote.state.lock().expect("remote state").saved.clone();
    saved.remote = match mode.as_str() {
        "local" => false,
        "remote" if saved.pairing.is_some() => true,
        "remote" => return Err("还没有与远程中心配对".into()),
        _ => return Err("连接模式无效".into()),
    };
    remote.replace(saved)?;
    Remote::start(&app);
    Ok(remote.status())
}

#[tauri::command]
pub async fn connection_set_share_node(
    app: AppHandle,
    remote: State<'_, Remote>,
    enabled: bool,
) -> Result<Connection, String> {
    let _lock = remote.actions.lock().await;
    let mut saved = remote.state.lock().expect("remote state").saved.clone();
    saved.share_node = enabled;
    remote.replace(saved)?;
    Remote::start(&app);
    Ok(remote.status())
}

#[tauri::command]
pub async fn connection_reconnect(
    app: AppHandle,
    remote: State<'_, Remote>,
) -> Result<Connection, String> {
    let _lock = remote.actions.lock().await;
    Remote::start(&app);
    Ok(remote.status())
}

#[tauri::command]
pub async fn connection_forget(
    app: AppHandle,
    remote: State<'_, Remote>,
) -> Result<Connection, String> {
    let _lock = remote.actions.lock().await;
    let pairing = remote
        .state
        .lock()
        .expect("remote state")
        .saved
        .pairing
        .clone();
    remote.replace(Saved::default())?;
    Remote::start(&app);
    if let Some(pairing) = pairing {
        let _ = tauri::async_runtime::spawn_blocking(move || {
            credential(&pairing.device_id)
                .and_then(|entry| entry.delete_credential().map_err(|err| err.to_string()))
        })
        .await;
    }
    Ok(remote.status())
}
