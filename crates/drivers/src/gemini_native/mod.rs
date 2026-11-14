//! `gemini-native`: Gemini CLI's headless `-p -o stream-json` (docs/research/gemini-cli.md).
//!
//! One process per turn; the conversation is the CLI's own session file, so a turn in a
//! new process carries on where the last one stopped. The first turn opens the session
//! with `--session-id` (the hub chooses the id), later turns and `resume` use `--resume`.
//! Sessions are stored per project (`~/.gemini/tmp/<project hash>/chats/`), so a session
//! cannot move to another working directory.
//!
//! - The collaboration MCP servers go in a settings file of the session's own, handed to
//!   the CLI as `GEMINI_CLI_SYSTEM_SETTINGS_PATH`. A file already named by that variable
//!   is read first and merged (the CLI merges `mcpServers` per server name), so an
//!   administrator's settings are kept.
//! - Headless mode has no per-action approval channel: the policy engine treats
//!   `ask_user` as `deny`. A bypassing session therefore runs `--approval-mode yolo`; a
//!   denying one runs `--approval-mode default`, where everything that would ask is
//!   denied. A permission callback cannot be honoured, and the session says so.
//! - A message too long for a Windows command line goes on stdin (`--prompt=` plus the
//!   CLI's own "append stdin" rule) instead of argv.

pub mod login;
pub mod wire;

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use crossagents_core::{
    Capabilities, CliStatus, CoreError, Driver, DriverKind, DriverSession, McpServerSpec,
    PermissionPolicy, Prompt, SessionEvent, SessionEventKind, SessionSpec, StopReason, TokenUsage,
    TurnOutcome,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::launch::{LaunchSpec, Launched, ProcessHandle};
use crate::locate::Cli;
use crate::process::{hide_console, resolve_program};
use wire::{Line, Turn, TurnArgs};

/// The variable that points the CLI at the settings file we write for the session.
const SETTINGS_ENV: &str = "GEMINI_CLI_SYSTEM_SETTINGS_PATH";
/// How long `gemini --list-models` gets.
const MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the process gets to exit once its output has ended.
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);
/// A prompt longer than this (bytes) goes on stdin: a Windows command line holds 32767
/// UTF-16 units, and the other arguments need room too.
const PROMPT_STDIN_LIMIT: usize = 16_000;

pub struct GeminiNativeDriver {
    name: String,
    cli: Cli,
    models: Vec<String>,
    /// What `gemini --list-models` answered, once it has.
    listed: Mutex<Vec<String>>,
}

impl GeminiNativeDriver {
    pub fn new(name: impl Into<String>, cli: Cli, models: Vec<String>) -> Self {
        Self {
            name: name.into(),
            cli,
            models,
            listed: Mutex::new(Vec::new()),
        }
    }

    fn command(&self, spec: &SessionSpec) -> Result<Vec<String>, CoreError> {
        if let Some(command) = spec.command.clone().filter(|c| !c.is_empty()) {
            return Ok(command);
        }
        self.cli.command().ok_or_else(|| {
            CoreError::Agent(
                self.cli
                    .missing_message(&self.name)
                    .unwrap_or_else(|| format!("the {} CLI is not installed", self.name)),
            )
        })
    }

    async fn session(
        &self,
        session_id: String,
        spec: SessionSpec,
        fresh: bool,
    ) -> Result<Box<dyn DriverSession>, CoreError> {
        let command = self.command(&spec)?;
        Ok(Box::new(GeminiSession::new(
            session_id, command, spec, fresh,
        )?))
    }

    /// Asks the CLI which models it offers. Best effort; the description's list stands in
    /// until it answers.
    async fn list_models(&self) {
        let Some(command) = self.cli.command() else {
            return;
        };
        let Some((program, extra)) = command.split_first() else {
            return;
        };
        let mut child = Command::new(resolve_program(program));
        child
            .args(extra)
            .arg("--list-models")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        hide_console(&mut child);
        let Ok(Ok(output)) = tokio::time::timeout(MODEL_LIST_TIMEOUT, child.output()).await else {
            tracing::debug!("gemini --list-models did not answer");
            return;
        };
        let models = wire::parse_models(&String::from_utf8_lossy(&output.stdout));
        if !models.is_empty() {
            *self.listed.lock().expect("model list") = models;
        }
    }
}

#[async_trait]
impl Driver for GeminiNativeDriver {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> DriverKind {
        DriverKind::GeminiNative
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            interrupt: true,
            mcp: true,
            resume: true,
            // Sessions are stored per project directory; another cwd cannot find them.
            move_cwd: false,
            set_model: true,
            usage: true,
            ..Capabilities::default()
        }
    }

    fn models(&self) -> Vec<String> {
        let listed = self.listed.lock().expect("model list");
        if !listed.is_empty() {
            return listed.clone();
        }
        drop(listed);
        self.models.clone()
    }

    fn installed(&self) -> bool {
        self.cli.installed()
    }

    async fn cli_status(&self) -> Option<CliStatus> {
        Some(self.cli.status().await)
    }

    /// Asked at hub startup, in the background: the version, then the model list.
    async fn probe_cli(&self) {
        let _ = self.cli.version().await;
        self.list_models().await;
    }

    fn refresh_cli(&self) {
        self.cli.forget();
    }

    fn cli_places(&self) -> Vec<String> {
        self.cli.searched()
    }

    async fn open(&self, spec: SessionSpec) -> Result<Box<dyn DriverSession>, CoreError> {
        self.session(uuid::Uuid::new_v4().to_string(), spec, true)
            .await
    }

    /// The conversation is the CLI's to keep: the same id carries on, in the same cwd.
    async fn resume(
        &self,
        session_id: &str,
        spec: SessionSpec,
    ) -> Result<Box<dyn DriverSession>, CoreError> {
        self.session(session_id.to_owned(), spec, false).await
    }
}

pub struct GeminiSession {
    id: String,
    command: Vec<String>,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    files: SessionFiles,
    model: Mutex<Option<String>>,
    instructions: Mutex<Option<String>>,
    tokens: Mutex<TokenUsage>,
    approval: &'static str,
    /// The next turn opens the session; every turn after resumes it.
    first: AtomicBool,
    events: mpsc::UnboundedSender<SessionEvent>,
    turn: tokio::sync::Mutex<()>,
    running: Mutex<Option<Arc<ProcessHandle>>>,
    cancelled: AtomicBool,
    closed: AtomicBool,
}

impl GeminiSession {
    fn new(
        id: String,
        command: Vec<String>,
        spec: SessionSpec,
        fresh: bool,
    ) -> Result<Self, CoreError> {
        let files = SessionFiles::new(&spec.mcp_servers, &spec.env);
        if let Some(warning) = &files.warning {
            let _ = spec.events.send(
                SessionEventKind::Warning {
                    message: warning.clone(),
                }
                .into(),
            );
        }
        if spec.permissions.is_some() {
            let _ = spec.events.send(
                SessionEventKind::Warning {
                    message: "Gemini CLI headless has no per-action approval channel: the session's permission policy decides for the whole run (yolo for bypass, default/deny otherwise).".into(),
                }
                .into(),
            );
        }
        let _ = spec.events.send(
            SessionEventKind::Started {
                session_id: id.clone(),
            }
            .into(),
        );
        Ok(Self {
            id,
            command,
            cwd: spec.agent.cwd,
            env: spec.env,
            files,
            model: Mutex::new(spec.agent.model.filter(|m| !m.is_empty())),
            instructions: Mutex::new(spec.instructions),
            tokens: Mutex::new(TokenUsage::default()),
            approval: match spec.permission {
                PermissionPolicy::Deny => "default",
                PermissionPolicy::Bypass | PermissionPolicy::Guard => "yolo",
            },
            first: AtomicBool::new(fresh),
            events: spec.events,
            turn: tokio::sync::Mutex::new(()),
            running: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    fn emit(&self, kind: SessionEventKind) {
        let _ = self.events.send(kind.into());
    }

    /// The message as the CLI gets it: the opening instructions once, then the text and
    /// its attachments as `@path` references (which Gemini CLI reads as files).
    fn prompt_text(&self, prompt: &Prompt) -> String {
        let mut text = match self.instructions.lock().expect("instructions").take() {
            Some(instructions) => format!("{instructions}\n\n{}", prompt.text),
            None => prompt.text.clone(),
        };
        for attachment in &prompt.attachments {
            text.push_str("\n\n");
            text.push_str(&attachment.reference());
        }
        text
    }

    /// One cumulative `Usage` event from the terminal `result`'s `stats`. Nothing is
    /// emitted when the CLI reported no stats.
    fn emit_usage(&self, stats: Option<&Value>) {
        let Some(stats) = stats else {
            return;
        };
        let num = |key: &str| stats[key].as_u64().unwrap_or(0);
        let input = num("input_tokens");
        let cached = num("cached");
        let output = num("output_tokens");
        let used = input + output;
        let tokens = {
            let mut totals = self.tokens.lock().expect("tokens");
            totals.input += input;
            totals.cached_input += cached.min(input);
            totals.output += output;
            totals.total += used;
            totals.clone()
        };
        self.emit(SessionEventKind::Usage {
            used,
            size: 0,
            cost_usd: None,
            tokens: Some(tokens),
        });
    }

    async fn run(&self, prompt: &Prompt) -> Result<TurnOutcome, CoreError> {
        let _guard = self.turn.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return Err(CoreError::Agent("the session has ended".into()));
        }
        self.cancelled.store(false, Ordering::SeqCst);
        let text = self.prompt_text(prompt);
        let model = self.model.lock().expect("model").clone();
        let first = self.first.swap(false, Ordering::SeqCst);
        // A message past the command-line limit is handed over on stdin; the CLI appends
        // stdin to `--prompt`.
        let (prompt_arg, on_stdin) = if text.len() > PROMPT_STDIN_LIMIT {
            ("", Some(text.as_str()))
        } else {
            (text.as_str(), None)
        };
        let args = wire::turn_args(&TurnArgs {
            session_id: &self.id,
            first,
            prompt: prompt_arg,
            model: model.as_deref(),
            approval: self.approval,
        });
        let (program, all_args) = crate::argv::launch_command(&self.command, args)
            .ok_or_else(|| CoreError::Agent("empty Gemini launch command".into()))?;
        let mut env = self.env.clone();
        env.extend(self.files.env());
        self.emit(SessionEventKind::TurnStarted { turn_id: None });
        let launched = crate::launch::launch(LaunchSpec {
            program,
            args: all_args,
            cwd: self.cwd.clone(),
            env,
            label: "Gemini CLI".into(),
            ..LaunchSpec::default()
        })
        .await;
        let Launched {
            mut stdin,
            stdout,
            stderr,
            process,
        } = match launched {
            Ok(launched) => launched,
            Err(err) => {
                // The session never opened, so the next turn tries to open it again.
                self.first.store(first, Ordering::SeqCst);
                self.emit(SessionEventKind::Error {
                    message: err.to_string(),
                });
                self.emit(SessionEventKind::TurnCompleted {
                    turn_id: None,
                    stop_reason: StopReason::Error,
                });
                return Err(err);
            }
        };
        if let Some(text) = on_stdin {
            let _ = stdin.write_all(text.as_bytes()).await;
        }
        drop(stdin);
        let process = Arc::new(process);
        *self.running.lock().expect("running") = Some(process.clone());
        // An interrupt may have arrived while launch was awaiting process creation.
        if self.cancelled.load(Ordering::SeqCst) || self.closed.load(Ordering::SeqCst) {
            process.start_kill();
        }
        let mut turn = Turn::default();
        let mut text_lines = Vec::new();
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match turn.line(&line) {
                    Line::Events(events) => {
                        for event in events {
                            let _ = self.events.send(event);
                        }
                    }
                    Line::Text(text) if text_lines.len() < 20 => text_lines.push(text),
                    _ => {}
                },
                Ok(None) => break,
                Err(err) => {
                    text_lines.push(format!("Gemini output: {err}"));
                    break;
                }
            }
        }
        let exit = process.exit_within(EXIT_TIMEOUT).await;
        if exit.is_none() {
            process.start_kill();
        }
        *self.running.lock().expect("running") = None;
        let stop_reason = if self.cancelled.load(Ordering::SeqCst) {
            StopReason::Cancelled
        } else if let Some(reason) = turn.stop_reason {
            reason
        } else if exit.is_some_and(|e| e.code == Some(53)) {
            // The CLI's own "turn limit exceeded".
            StopReason::MaxTurnRequests
        } else {
            let what = exit
                .map(|e| e.describe("Gemini CLI"))
                .unwrap_or_else(|| "Gemini CLI stopped answering".into());
            let message = if text_lines.is_empty() {
                stderr
                    .describe(&format!("{what} before the turn ended"))
                    .await
            } else {
                format!("{what}: {}", text_lines.join("\n"))
            };
            self.emit(SessionEventKind::Error { message });
            StopReason::Error
        };
        // The session id is the one we chose: if the CLI echoed another, resuming by ours
        // would fail, so say so rather than switch silently.
        if let Some(reported) = turn.session_id.as_deref()
            && reported != self.id
        {
            self.emit(SessionEventKind::Warning {
                message: format!(
                    "Gemini CLI reported session {reported}, not {}; resuming this session may fail",
                    self.id
                ),
            });
        }
        self.emit_usage(turn.stats.as_ref());
        self.emit(SessionEventKind::TurnCompleted {
            turn_id: None,
            stop_reason,
        });
        Ok(TurnOutcome {
            stop_reason,
            turn_id: None,
        })
    }

    fn stop(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(process) = self.running.lock().expect("running").as_ref() {
            process.start_kill();
        }
    }
}

#[async_trait]
impl DriverSession for GeminiSession {
    fn session_id(&self) -> &str {
        &self.id
    }
    async fn prompt(&self, text: &str) -> Result<TurnOutcome, CoreError> {
        self.run(&Prompt::text(text)).await
    }
    async fn prompt_with(&self, prompt: &Prompt) -> Result<TurnOutcome, CoreError> {
        self.run(prompt).await
    }
    async fn interrupt(&self) -> Result<(), CoreError> {
        self.stop();
        Ok(())
    }
    async fn suspend(&self) -> Result<(), CoreError> {
        if self.turn.try_lock().is_err() {
            return Err(CoreError::Agent("a turn is running".into()));
        }
        Ok(())
    }
    async fn set_model(&self, model: Option<&str>, _effort: Option<&str>) -> Result<(), CoreError> {
        if let Some(model) = model {
            *self.model.lock().expect("model") =
                Some(model.into()).filter(|m: &String| !m.is_empty());
        }
        Ok(())
    }
    async fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.stop();
        let _guard = self.turn.lock().await;
        self.files.remove();
    }
}

/// The session's settings file, and the warning when it could not be written.
struct SessionFiles {
    dir: PathBuf,
    settings: Option<PathBuf>,
    warning: Option<String>,
}

impl SessionFiles {
    fn new(servers: &[McpServerSpec], env: &[(String, String)]) -> Self {
        let mut files = Self {
            dir: std::env::temp_dir().join(format!("crossagents-gemini-{}", uuid::Uuid::new_v4())),
            settings: None,
            warning: None,
        };
        if servers.is_empty() {
            return files;
        }
        // An administrator's system settings stay in force: their file is the base, and
        // only our servers are added to it.
        let mut root = match existing_settings(env) {
            None => json!({}),
            Some(path) => match std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            {
                Some(value) if value.is_object() => value,
                _ => {
                    files.warning = Some(format!(
                        "cannot read the Gemini settings file at {path}; the collaboration tools were not mounted for this session"
                    ));
                    return files;
                }
            },
        };
        let mut entries = root
            .get("mcpServers")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for server in servers {
            entries.insert(
                server.name.clone(),
                json!({
                    "command": server.command,
                    "args": server.args,
                    "env": server.env,
                    // The hub's own tools need no confirmation; headless mode would deny them.
                    "trust": true,
                }),
            );
        }
        root["mcpServers"] = Value::Object(entries);
        let settings = files.dir.join("settings.json");
        let written = std::fs::create_dir_all(&files.dir)
            .and_then(|()| std::fs::write(&settings, root.to_string()));
        match written {
            Ok(()) => files.settings = Some(settings),
            Err(err) => {
                files.warning = Some(format!(
                    "cannot write the Gemini settings file at {}: {err}; the collaboration tools were not mounted for this session",
                    settings.display()
                ));
            }
        }
        files
    }

    /// The environment that points the CLI at the session's settings.
    fn env(&self) -> Vec<(String, String)> {
        self.settings
            .as_ref()
            .map(|path| vec![(SETTINGS_ENV.to_string(), path.display().to_string())])
            .unwrap_or_default()
    }

    fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for SessionFiles {
    fn drop(&mut self) {
        self.remove();
    }
}

/// The settings file the CLI would read anyway, if the person (or an administrator) named
/// one. A session's own environment wins over the hub's.
fn existing_settings(env: &[(String, String)]) -> Option<String> {
    env.iter()
        .rev()
        .find(|(key, _)| key == SETTINGS_ENV)
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var(SETTINGS_ENV).ok())
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossagents_core::{Agent, AgentId, AgentStatus, Role};
    use crossagents_core::{PermissionPolicy, SessionSpec};
    use serde_json::json;
    use std::path::Path;

    fn spec(cwd: &Path) -> (SessionSpec, mpsc::UnboundedReceiver<SessionEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        (
            SessionSpec {
                agent: Agent {
                    node: None,
                    id: AgentId::new(),
                    name: "Gemini".into(),
                    name_source: Default::default(),
                    plugin: "gemini".into(),
                    driver: None,
                    model: Some("gemini-2.5-pro".into()),
                    role: Role::Worker,
                    home: cwd.into(),
                    cwd: cwd.into(),
                    config: json!({}),
                    status: AgentStatus::Stopped,
                    created_at: chrono::Utc::now(),
                    profile: Default::default(),
                    topic: None,
                    archived_at: None,
                },
                mcp_servers: vec![],
                permission: PermissionPolicy::Bypass,
                permissions: None,
                command: None,
                instructions: None,
                env: vec![],
                events,
            },
            rx,
        )
    }

    fn server() -> McpServerSpec {
        McpServerSpec {
            name: "crossagents".into(),
            command: "crossagents".into(),
            args: vec!["mcp".into()],
            env: Default::default(),
        }
    }

    /// A mock CLI: logs every argv to `CROSSAGENTS_GEMINI_TEST_LOG`, then prints one
    /// stream. `windows` / `unix` are the script bodies.
    fn mock_cli(root: &Path, windows: &str, unix: &str) -> Vec<String> {
        let path = root.join(if cfg!(windows) { "mock.ps1" } else { "mock.sh" });
        std::fs::write(&path, if cfg!(windows) { windows } else { unix }).unwrap();
        if cfg!(windows) {
            vec![
                "powershell".into(),
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                path.display().to_string(),
            ]
        } else {
            vec!["sh".into(), path.display().to_string()]
        }
    }

    const OK_WINDOWS: &str = r#"
$ErrorActionPreference = 'Stop'
$log = $env:CROSSAGENTS_GEMINI_TEST_LOG
$sid = ''
foreach ($a in $args) {
  if ($a -match '^--(session-id|resume)=(.+)$') { $sid = $Matches[2] }
  Add-Content -Path $log -Value ("ARG`t" + $a)
}
Add-Content -Path $log -Value ("SET`t" + $env:GEMINI_CLI_SYSTEM_SETTINGS_PATH)
if ($env:GEMINI_CLI_SYSTEM_SETTINGS_PATH) {
  Add-Content -Path $log -Value ("SETTINGS`t" + (Get-Content -Raw $env:GEMINI_CLI_SYSTEM_SETTINGS_PATH))
}
Add-Content -Path $log -Value 'END'
Write-Output ('{"type":"init","timestamp":"t","session_id":"' + $sid + '","model":"gemini-2.5-pro"}')
Write-Output '{"type":"message","timestamp":"t","role":"user","content":"hi"}'
Write-Output '{"type":"message","timestamp":"t","role":"assistant","content":"hello","delta":true}'
Write-Output '{"type":"message","timestamp":"t","role":"assistant","content":"hello","delta":true}'
Write-Output '{"type":"tool_use","timestamp":"t","tool_name":"read_file","tool_id":"t1","parameters":{"file_path":"a.txt"}}'
Write-Output '{"type":"tool_result","timestamp":"t","tool_id":"t1","status":"success","output":"contents"}'
Write-Output '{"type":"result","timestamp":"t","status":"success","stats":{"total_tokens":15,"input_tokens":10,"output_tokens":5,"cached":2}}'
"#;

    const OK_UNIX: &str = r#"
log="$CROSSAGENTS_GEMINI_TEST_LOG"
sid=""
for a in "$@"; do
  case "$a" in
    --session-id=*|--resume=*) sid="${a#*=}" ;;
  esac
  printf 'ARG\t%s\n' "$a" >> "$log"
done
printf 'SET\t%s\n' "$GEMINI_CLI_SYSTEM_SETTINGS_PATH" >> "$log"
if [ -n "$GEMINI_CLI_SYSTEM_SETTINGS_PATH" ]; then
  printf 'SETTINGS\t%s\n' "$(cat "$GEMINI_CLI_SYSTEM_SETTINGS_PATH")" >> "$log"
fi
printf 'END\n' >> "$log"
printf '%s\n' '{"type":"init","timestamp":"t","session_id":"'"$sid"'","model":"gemini-2.5-pro"}'
printf '%s\n' '{"type":"message","timestamp":"t","role":"user","content":"hi"}'
printf '%s\n' '{"type":"message","timestamp":"t","role":"assistant","content":"hello","delta":true}'
printf '%s\n' '{"type":"message","timestamp":"t","role":"assistant","content":"hello","delta":true}'
printf '%s\n' '{"type":"tool_use","timestamp":"t","tool_name":"read_file","tool_id":"t1","parameters":{"file_path":"a.txt"}}'
printf '%s\n' '{"type":"tool_result","timestamp":"t","tool_id":"t1","status":"success","output":"contents"}'
printf '%s\n' '{"type":"result","timestamp":"t","status":"success","stats":{"total_tokens":15,"input_tokens":10,"output_tokens":5,"cached":2}}'
"#;

    fn read_log(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[tokio::test]
    async fn first_turn_opens_the_session_then_each_turn_resumes_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.txt");
        let (mut spec, mut events) = spec(dir.path());
        spec.instructions = Some("be brief".into());
        spec.mcp_servers = vec![server()];
        spec.env = vec![(
            "CROSSAGENTS_GEMINI_TEST_LOG".into(),
            log.display().to_string(),
        )];
        let driver = GeminiNativeDriver::new(
            "gemini",
            Cli::at(mock_cli(dir.path(), OK_WINDOWS, OK_UNIX)),
            vec![],
        );
        let session = driver.open(spec).await.unwrap();
        let id = session.session_id().to_owned();

        assert_eq!(
            session.prompt("hi").await.unwrap().stop_reason,
            StopReason::EndTurn
        );
        assert_eq!(
            session.prompt("again").await.unwrap().stop_reason,
            StopReason::EndTurn
        );

        let mut text = String::new();
        let mut usage = None;
        let mut started = 0;
        let mut tools = 0;
        while let Ok(event) = events.try_recv() {
            match event.kind {
                SessionEventKind::Started { .. } => started += 1,
                SessionEventKind::Output { text: t, .. } => text.push_str(&t),
                SessionEventKind::ToolCall { .. } => tools += 1,
                SessionEventKind::Usage { tokens, .. } => usage = tokens,
                _ => {}
            }
        }
        assert_eq!(started, 1);
        assert_eq!(text, "hellohellohellohello");
        assert_eq!(tools, 2);
        let usage = usage.expect("usage");
        assert_eq!(usage.input, 20);
        assert_eq!(usage.cached_input, 4);
        assert_eq!(usage.output, 10);

        let log = read_log(&log);
        assert_eq!(log.matches(&format!("--session-id={id}")).count(), 1);
        assert_eq!(log.matches(&format!("--resume={id}")).count(), 1);
        assert_eq!(log.matches("--approval-mode=yolo").count(), 2);
        assert_eq!(log.matches("--model=gemini-2.5-pro").count(), 2);
        assert_eq!(log.matches("be brief").count(), 1, "{log}");
        // The collaboration server is mounted, trusted, and merged into the session's
        // settings; the file is gone once the session ends.
        assert!(log.contains(r#""mcpServers":{"crossagents""#), "{log}");
        assert!(log.contains(r#""trust":true"#), "{log}");
        let settings = log
            .lines()
            .find_map(|line| line.strip_prefix("SET\t"))
            .filter(|path| !path.is_empty())
            .expect("settings path");
        assert!(Path::new(settings).is_file());
        session.shutdown().await;
        assert!(!Path::new(settings).exists());
    }

    #[tokio::test]
    async fn a_session_that_denies_permissions_runs_without_yolo() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.txt");
        let (mut spec, _events) = spec(dir.path());
        spec.permission = PermissionPolicy::Deny;
        spec.env = vec![(
            "CROSSAGENTS_GEMINI_TEST_LOG".into(),
            log.display().to_string(),
        )];
        let driver = GeminiNativeDriver::new(
            "gemini",
            Cli::at(mock_cli(dir.path(), OK_WINDOWS, OK_UNIX)),
            vec![],
        );
        let session = driver.open(spec).await.unwrap();
        session.prompt("hi").await.unwrap();
        let log = read_log(&log);
        assert!(log.contains("--approval-mode=default"), "{log}");
        assert!(!log.contains("--approval-mode=yolo"), "{log}");
        // No servers: no settings file, and the CLI keeps its own.
        assert!(!log.contains("SETTINGS\t"), "{log}");
    }

    #[tokio::test]
    async fn the_turn_limit_exit_code_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let (spec, _events) = spec(dir.path());
        let driver = GeminiNativeDriver::new(
            "gemini",
            Cli::at(mock_cli(dir.path(), "exit 53\n", "exit 53\n")),
            vec![],
        );
        let session = driver.open(spec).await.unwrap();
        let outcome = session.prompt("hi").await.unwrap();
        assert_eq!(outcome.stop_reason, StopReason::MaxTurnRequests);
    }

    #[tokio::test]
    async fn an_interrupt_stops_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let (spec, _events) = spec(dir.path());
        let command = mock_cli(dir.path(), "Start-Sleep -Seconds 60\n", "sleep 60\n");
        let session = Arc::new(GeminiSession::new("id-1".into(), command, spec, true).unwrap());
        let prompt = tokio::spawn({
            let session = session.clone();
            async move { session.prompt("hi").await }
        });
        for _ in 0..100 {
            if session.running.lock().expect("running").is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        session.interrupt().await.unwrap();
        let outcome = prompt.await.unwrap().unwrap();
        assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    }
}
