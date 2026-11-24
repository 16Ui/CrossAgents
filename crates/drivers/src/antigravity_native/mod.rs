//! `antigravity-native`: Google Antigravity CLI's headless stream-json
//! (docs/research/antigravity-cli.md).
//!
//! One process per session: `agy --input-format stream-json --output-format stream-json`
//! stays up and takes one `user` message per turn on stdin, answering in NDJSON. That is
//! cheaper than restarting it per turn, and the process holds the conversation; when it is
//! gone (an interrupt, a crash), the next turn starts a new one with
//! `--conversation <id>`, which is also what [`Driver::resume`] does.
//!
//! - The conversation id is the CLI's: it arrives in `init`/`result`, and the session
//!   reports it as [`SessionEventKind::Started`] once it has it. A new session therefore
//!   has a provisional id until its first turn.
//! - Headless mode has no approval channel, but it soft-denies instead of failing: a tool
//!   that would ask is refused, the turn carries on and exits 0. A bypassing session runs
//!   `--dangerously-skip-permissions`, a guarding one `--mode accept-edits`, a denying one
//!   leaves the default mode (everything that asks is refused).
//! - Antigravity reads `~/.gemini/config/mcp_config.json` for MCP servers (`--settings`
//!   rejects `mcpServers`), and forwards parent environment variables (such as per-session
//!   `CROSSAGENTS_URL` and `CROSSAGENTS_AGENT_TOKEN`) when the entry defines a non-empty
//!   `env` map. Each session keeps credentials strictly in its own process env rather than
//!   writing tokens to disk.

pub mod login;
pub mod wire;

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use crossagents_core::{
    Capabilities, CliStatus, CoreError, Driver, DriverKind, DriverSession, McpServerSpec,
    OutputStream, PermissionPolicy, Prompt, SessionEvent, SessionEventKind, SessionSpec,
    StopReason, TokenUsage, ToolDetail, ToolStatus, TurnOutcome,
};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::launch::{BoxWrite, LaunchSpec, Launched, ProcessHandle};
use crate::locate::Cli;
use crate::process::{StderrTail, hide_console, resolve_program, system_proxy_env};
use wire::{Event, Step};

/// How long `agy models` gets.
pub const COORDINATION_ENV: &str = "CROSSAGENTS_INTERNAL_COORDINATION_ROLE";
pub const LEAD_TOOLS: &[&str] = &[
    "plan_submit",
    "task_list",
    "task_get",
    "task_command",
    "task_create",
    "task_assign",
    "task_rework",
    "task_answer",
    "workspace_diff",
    "review_request",
    "task_accept",
    "human_ask",
    "run_finish",
    "run_next",
];
pub const WORKER_TOOLS: &[&str] = &[
    "task_get",
    "task_command",
    "task_progress",
    "task_complete",
    "task_fail",
    "task_ask",
];
pub const REVIEW_TOOLS: &[&str] = &["task_get", "workspace_diff", "review_submit"];
const MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the process gets to exit once its output has ended.
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);
/// A coordinated session with no running tools must release the queue if its stream goes silent.
const LEAD_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const TASK_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

pub struct AntigravityDriver {
    name: String,
    cli: Cli,
    models: Vec<String>,
    /// What `agy models` answered, once it has.
    listed: Mutex<Vec<String>>,
}

impl AntigravityDriver {
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
        id: String,
        spec: SessionSpec,
        resume: Option<String>,
    ) -> Result<Box<dyn DriverSession>, CoreError> {
        let command = self.command(&spec)?;
        Ok(Box::new(AntigravitySession::new(id, command, spec, resume)))
    }

    /// Asks the CLI which models the signed-in account may use. Best effort: the
    /// description's list stands in until it answers (and it needs a sign-in to answer).
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
            .arg("models")
            .envs(system_proxy_env())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        hide_console(&mut child);
        let Ok(Ok(output)) = tokio::time::timeout(MODEL_LIST_TIMEOUT, child.output()).await else {
            tracing::debug!("agy models did not answer");
            return;
        };
        let models = wire::parse_models(&String::from_utf8_lossy(&output.stdout));
        if !models.is_empty() {
            *self.listed.lock().expect("model list") = models;
        }
    }
}

#[async_trait]
impl Driver for AntigravityDriver {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> DriverKind {
        DriverKind::AntigravityNative
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            interrupt: true,
            resume: true,
            set_model: true,
            mcp: true,
            usage: true,
            // Conversations are stored per project directory.
            move_cwd: false,
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
        self.session(uuid::Uuid::new_v4().to_string(), spec, None)
            .await
    }

    async fn resume(
        &self,
        session_id: &str,
        spec: SessionSpec,
    ) -> Result<Box<dyn DriverSession>, CoreError> {
        self.session(session_id.to_owned(), spec, Some(session_id.to_owned()))
            .await
    }
}

/// The live CLI process of a session.
struct Live {
    stdin: BoxWrite,
    lines: mpsc::UnboundedReceiver<String>,
    process: Arc<ProcessHandle>,
    stderr: StderrTail,
}

pub struct AntigravitySession {
    id: String,
    agent_id: crossagents_core::AgentId,
    /// The CLI's conversation id, once it has said it.
    conversation: Mutex<Option<String>>,

    command: Vec<String>,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    mcp_servers: Vec<McpServerSpec>,
    model: Mutex<Option<String>>,
    effort: Mutex<Option<String>>,
    permission: PermissionPolicy,
    instructions: Option<String>,
    instructions_pending: AtomicBool,
    tokens: Mutex<TokenUsage>,
    /// Tool calls already announced, so a `DONE` step updates instead of adding another.
    tools: Mutex<HashSet<String>>,
    events: mpsc::UnboundedSender<SessionEvent>,
    turn: tokio::sync::Mutex<()>,
    child: tokio::sync::Mutex<Option<Live>>,
    running: Mutex<Option<Arc<ProcessHandle>>>,
    cancelled: AtomicBool,
    closed: AtomicBool,
    consecutive_stream_timeouts: AtomicUsize,
    context_rotated: AtomicBool,
    role: crossagents_core::Role,
    coordination_tools: &'static [&'static str],
}

impl AntigravitySession {
    fn coordinates_run(&self) -> bool {
        self.role == crossagents_core::Role::Planner || self.coordination_tools == LEAD_TOOLS
    }

    fn coordination_idle_timeout(&self) -> Option<Duration> {
        if self.coordinates_run() {
            Some(LEAD_IDLE_TIMEOUT)
        } else if !self.coordination_tools.is_empty() {
            Some(TASK_IDLE_TIMEOUT)
        } else {
            None
        }
    }

    fn new(id: String, command: Vec<String>, spec: SessionSpec, resume: Option<String>) -> Self {
        if let Some(id) = &resume {
            let _ = spec.events.send(
                SessionEventKind::Started {
                    session_id: id.clone(),
                }
                .into(),
            );
        }
        let mut env = spec.env;
        let mut coordination_tools = &[][..];
        env.retain(|(key, value)| {
            if !key.eq_ignore_ascii_case(COORDINATION_ENV) {
                return true;
            }
            if spec.permission == PermissionPolicy::Guard {
                coordination_tools = match value.as_str() {
                    "lead" => LEAD_TOOLS,
                    "worker" => WORKER_TOOLS,
                    "reviewer" => REVIEW_TOOLS,
                    _ => &[],
                };
            }
            false
        });
        Self {
            id,
            agent_id: spec.agent.id,
            conversation: Mutex::new(resume),

            command,
            cwd: spec.agent.cwd,
            env,
            mcp_servers: spec.mcp_servers,
            model: Mutex::new(spec.agent.model.filter(|m| !m.is_empty())),
            effort: Mutex::new(None),
            permission: spec.permission,
            instructions: spec.instructions,
            instructions_pending: AtomicBool::new(true),
            tokens: Mutex::new(TokenUsage::default()),
            tools: Mutex::new(HashSet::new()),
            events: spec.events,
            turn: tokio::sync::Mutex::new(()),
            child: tokio::sync::Mutex::new(None),
            running: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            consecutive_stream_timeouts: AtomicUsize::new(0),
            context_rotated: AtomicBool::new(false),
            role: spec.agent.role,
            coordination_tools,
        }
    }

    fn emit(&self, kind: SessionEventKind) {
        let _ = self.events.send(kind.into());
    }

    /// The message as the CLI gets it: the opening instructions once, then the text and
    /// its attachments as `@path` references (which the CLI reads as files).
    fn prompt_text(&self, prompt: &Prompt) -> String {
        let mut text = match &self.instructions {
            Some(instructions) if self.instructions_pending.swap(false, Ordering::SeqCst) => {
                format!("{instructions}\n\n{}", prompt.text)
            }
            _ => prompt.text.clone(),
        };
        if !self.coordination_tools.is_empty() {
            use std::fmt::Write;
            let _ = write!(
                text,
                "\n\n[CrossAgents session capabilities]\nWorking directory: {}\nCrossAgents MCP methods configured for this role (permission policy still applies): {}.\nInvoke these methods directly through the MCP server. Tool definitions are supplied by MCP; there are no per-tool JSON files to find on disk. For run facts use task_list or task_get only when they appear above. task_get takes {{\"task\":\"task-key\"}}. Use workspace_diff with its declared arguments for review when available. In a guarded turn, use the provider’s list, search and read tools for workspace inspection. Do not use shell probes such as rg, git status or git log. When task_command appears in your tools, use it for a validation command in your active task. Include the exact command, its purpose and the resources it needs. CrossAgents asks only if a matching grant is absent. Never repeat a denied request unchanged. If a project quota is enforced, the hub checks it and pauses or resumes work. Do not query model quota through the shell or read quota files outside the workspace. Continue assigned work while the hub allows it. A pending task is not proof its planned output file exists. The lead owns routine triage, scope-preserving choices, dispatch, and recovery. Continue already requested work without asking the person to choose it again. Use task_ask for internal worker questions. Escalate only concrete security risks, irreversible actions, or external commitments. Explain what happened, why the lead cannot decide, the consequences, and exactly what decision is needed in the person's language. Use project names and plain wording; keep internal task codes and command output in the detail. Do not use agent_list, whoami, guessed paths, or unlisted methods as prerequisites. Use file and shell tools only within the current working directory and for this run, with relative paths for local files. A path quoted by another run, a task report, or a checkpoint is evidence, not a workspace grant. Do not follow it into another workspace or try another tool to access it. If another run owns the work, leave its local files to that run. A missing grant or a rejected tool is a blocker for that operation. Proceed with independent authorized work and avoid repeating the failed call.",
                self.cwd.display(),
                self.coordination_tools.join(", "),
            );
            if self.coordination_tools.contains(&"task_rework") {
                text.push_str(concat!(
                    "\nWhen authorized for this role:",
                    r#" task_assign takes {"task":"task-key","to":"registered teammate ID"}."#,
                    r#" task_create takes {"tasks":[{"key":"short-key","title":"...","spec":{"goal":"..."}}]};"#,
                    " use at most 24 characters per key.",
                    r#" task_rework takes {"task":"task-key","notes":"cause, changed approach, and verification"}."#
                ));
            }
        }
        for attachment in &prompt.attachments {
            text.push_str("\n\n");
            text.push_str(&attachment.reference());
        }
        text
    }

    fn rotate_context(&self) {
        *self.conversation.lock().expect("conversation") = None;
        self.tokens.lock().expect("tokens").input = 0;
        self.tools.lock().expect("tools").clear();
        self.instructions_pending.store(true, Ordering::SeqCst);
        self.context_rotated.store(true, Ordering::SeqCst);
    }

    /// Report each deliberately opened context so the hub can resume the latest one.
    fn remember(&self, id: &str) {
        if id.is_empty() {
            return;
        }
        let mut conversation = self.conversation.lock().expect("conversation");
        if conversation.as_deref() == Some(id) {
            return;
        }
        let resumed = conversation.is_some();
        *conversation = Some(id.to_owned());
        drop(conversation);
        if resumed {
            self.emit(SessionEventKind::Warning {
                message: format!(
                    "Antigravity CLI reported conversation {id}, not {}; resuming this session may fail",
                    self.id
                ),
            });
            return;
        }
        self.emit(SessionEventKind::Started {
            session_id: id.to_owned(),
        });
    }

    /// The arguments a process is started with; the conversation id makes a restarted
    /// process carry on where the last one stopped.
    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "--input-format".to_owned(),
            "stream-json".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
        ];
        if let Some(id) = self
            .conversation
            .lock()
            .expect("conversation")
            .clone()
            .filter(|id| !id.is_empty())
        {
            args.push("--conversation".into());
            args.push(id);
        }
        if let Some(model) = self.model.lock().expect("model").clone() {
            args.push("--model".into());
            args.push(model);
        }
        if let Some(effort) = self.effort.lock().expect("effort").clone() {
            args.push("--effort".into());
            args.push(effort);
        }
        match self.permission {
            PermissionPolicy::Bypass => args.push("--dangerously-skip-permissions".into()),
            PermissionPolicy::Guard => {
                args.push("--mode".into());
                args.push("accept-edits".into());
            }
            // The default mode asks, and headless refuses what it cannot ask: the turn goes
            // on without the tool.
            PermissionPolicy::Deny => {}
        }
        args
    }

    /// Starts the process when there is none.
    async fn ensure(&self) -> Result<(), CoreError> {
        let mut child = self.child.lock().await;
        if child.is_some() {
            return Ok(());
        }
        if !self.mcp_servers.is_empty()
            && let Some(path) = mcp_config_path(&self.env)
            && let Err(err) = ensure_mcp_config_at(&path, &self.mcp_servers)
        {
            self.emit(SessionEventKind::Warning {
                message: format!(
                    "cannot update Antigravity MCP config at {}: {err}",
                    path.display()
                ),
            });
        }
        let mut args = self.args();
        let scoped_tool = match (self.role, self.permission) {
            (crossagents_core::Role::Planner, PermissionPolicy::Guard) => {
                Some(("crossagents-plan-submit", "mcp(crossagents/plan_submit)"))
            }
            _ => None,
        };
        if let Some(path) = mcp_config_path(&self.env)
            && let Some(root) = path.parent().and_then(|config| config.parent())
        {
            let settings_dir = root.join("antigravity-cli");
            let grant = scoped_tool.and_then(|(dir, grant)| {
                settings_dir
                    .join(dir)
                    .join(self.agent_id.to_string())
                    .is_file()
                    .then_some((dir, grant))
            });
            let mut grants: Vec<_> = self
                .coordination_tools
                .iter()
                .map(|tool| format!("mcp(crossagents/{tool})"))
                .collect();
            if let Some((_, rule)) = grant {
                grants.push(rule.to_owned());
            }
            if !grants.is_empty() {
                match tool_settings(&settings_dir, &grants) {
                    Ok(settings) => args.extend(["--settings".into(), settings.to_string()]),
                    Err(err) => self.emit(SessionEventKind::Warning {
                        message: format!(
                            "Cannot apply the agent's collaboration permissions: {err}"
                        ),
                    }),
                }
            }
        }
        let (program, all_args) = crate::argv::launch_command(&self.command, args)
            .ok_or_else(|| CoreError::Agent("empty Antigravity launch command".into()))?;
        let mut env = system_proxy_env();
        env.extend(self.env.iter().cloned());
        for server in &self.mcp_servers {
            for (k, v) in &server.env {
                env.push((k.clone(), v.clone()));
            }
        }
        let launched = crate::launch::launch(LaunchSpec {
            program,
            args: all_args,
            cwd: self.cwd.clone(),
            env,
            label: "Antigravity CLI".into(),
            ..LaunchSpec::default()
        })
        .await?;
        let Launched {
            stdin,
            stdout,
            stderr,
            process,
        } = launched;
        let (lines_tx, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines_in = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines_in.next_line().await {
                if lines_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let process = Arc::new(process);
        *self.running.lock().expect("running") = Some(process.clone());
        *child = Some(Live {
            stdin,
            lines,
            process,
            stderr,
        });
        Ok(())
    }

    /// One answer fragment, or `None` for steps that say nothing.
    fn on_step(&self, step: &Step) {
        if let Some(id) = &step.conversation_id {
            self.remember(id);
        }
        if step.is_answer() {
            if let Some(delta) = step.text_delta.as_deref().filter(|d| !d.is_empty()) {
                self.emit(SessionEventKind::Output {
                    stream: OutputStream::Text,
                    text: delta.to_owned(),
                });
            }
            return;
        }
        if !step.is_tool() {
            return;
        }
        let id = format!("step-{}", step.step_index.unwrap_or(0));
        let info = step.tool_info.as_ref();
        let name = step
            .tool_name
            .clone()
            .or_else(|| info.and_then(|i| i.name.clone()))
            .unwrap_or_else(|| "tool".to_owned());
        let kind = tool_kind(&name);
        if step.running() {
            self.tools.lock().expect("tools").insert(id.clone());
            self.emit(SessionEventKind::ToolCall {
                tool_call_id: id,
                title: name,
                kind: Some(kind.to_owned()),
                status: ToolStatus::InProgress,
            });
            return;
        }
        let failed = info.and_then(|i| i.error.as_ref()).is_some();
        let status = if failed {
            ToolStatus::Failed
        } else {
            ToolStatus::Completed
        };
        let first_sight = self.tools.lock().expect("tools").insert(id.clone());
        if first_sight {
            self.emit(SessionEventKind::ToolCall {
                tool_call_id: id.clone(),
                title: name,
                kind: Some(kind.to_owned()),
                status,
            });
        } else {
            self.emit(SessionEventKind::ToolCallUpdated {
                tool_call_id: id.clone(),
                title: None,
                status: Some(status),
            });
        }
        let detail = ToolDetail {
            arguments: info
                .and_then(|i| i.parameters.as_ref())
                .map(|parameters| parameters.to_string()),
            result: info.and_then(|i| i.output.clone()),
            error: info.and_then(|i| i.error.clone()),
            ..ToolDetail::default()
        };
        if detail != ToolDetail::default() {
            self.emit(SessionEventKind::ToolDetail {
                tool_call_id: id,
                detail,
            });
        }
    }

    /// The `result` event: the conversation, the answer when it never streamed, and the
    /// session's cumulative token counts.
    fn on_result(&self, result: &wire::Result) {
        if let Some(id) = &result.conversation_id {
            self.remember(id);
        }
        if let Some(usage) = result.usage {
            let tokens = {
                let mut totals = self.tokens.lock().expect("tokens");
                totals.input = usage.input;
                totals.cached_input = usage.cache_read.min(usage.input);
                totals.output = usage.output;
                totals.reasoning_output = usage.thinking;
                totals.total = usage.total;
                totals.clone()
            };
            self.emit(SessionEventKind::Usage {
                used: usage.total,
                size: 0,
                cost_usd: None,
                tokens: Some(tokens),
            });
        }
    }

    async fn run(&self, prompt: &Prompt) -> Result<TurnOutcome, CoreError> {
        const MAX_STREAM_ATTEMPTS: usize = 3;
        let _guard = self.turn.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return Err(CoreError::Agent("the session has ended".into()));
        }
        self.cancelled.store(false, Ordering::SeqCst);
        let mut original_text = self.prompt_text(prompt);
        if self.context_rotated.swap(false, Ordering::SeqCst) {
            original_text = format!(
                "[System: The model session was restarted. Use the supplied work summary and durable checkpoints to continue. Do not replay commands or tool calls from a previous turn.]\n\n{original_text}"
            );
        }
        let mut current_text = original_text.clone();
        self.emit(SessionEventKind::TurnStarted { turn_id: None });

        for attempt in 0..MAX_STREAM_ATTEMPTS {
            if let Err(err) = self.ensure().await {
                self.emit(SessionEventKind::Error {
                    message: err.to_string(),
                });
                self.emit(SessionEventKind::TurnCompleted {
                    turn_id: None,
                    stop_reason: StopReason::Error,
                });
                return Err(err);
            }
            let message = format!(
                "{}\n",
                json!({ "event": "user", "message": { "content": current_text } })
            );
            let mut child = self.child.lock().await;
            let Some(live) = child.as_mut() else {
                return Err(CoreError::Agent("Antigravity CLI is not running".into()));
            };
            if let Err(err) = live.stdin.write_all(message.as_bytes()).await {
                self.emit(SessionEventKind::Error {
                    message: format!("cannot send the turn to Antigravity CLI: {err}"),
                });
                *child = None;
                self.emit(SessionEventKind::TurnCompleted {
                    turn_id: None,
                    stop_reason: StopReason::Error,
                });
                return Ok(TurnOutcome {
                    stop_reason: StopReason::Error,
                    turn_id: None,
                });
            }
            let _ = live.stdin.flush().await;
            let mut saw_result = false;
            let mut ended = false;
            let mut stop: Option<StopReason> = None;
            let mut streamed = false;
            let mut saw_step = false;
            let mut saw_tool = false;
            let mut stream_timed_out = false;
            let mut retry_stream = false;
            let mut malformed_retry = false;
            let mut active_tools = HashSet::new();
            let mut context_input = None;
            loop {
                let next = if active_tools.is_empty()
                    && let Some(timeout) = self.coordination_idle_timeout()
                {
                    match tokio::time::timeout(timeout, live.lines.recv()).await {
                        Ok(next) => next,
                        Err(_) => {
                            // A previous tool might already have committed changes or dispatched
                            // tasks. End this turn without replaying it and release pending notices.
                            live.process.start_kill();
                            ended = true;
                            stop = Some(StopReason::Error);
                            stream_timed_out = true;
                            self.emit(SessionEventKind::Error {
                                message: "Antigravity coordination stream timed out with no tool running; pending notices can continue.".into(),
                            });
                            break;
                        }
                    }
                } else {
                    live.lines.recv().await
                };
                let Some(line) = next else {
                    ended = true;
                    break;
                };
                match wire::parse(&line) {
                    Some(Event::Init(init)) => {
                        if let Some(id) = &init.conversation_id {
                            self.remember(id);
                        }
                    }
                    Some(Event::Step(step)) => {
                        saw_step = true;
                        if step.is_tool() {
                            saw_tool = true;
                            if step.running() {
                                active_tools.insert(step.step_index);
                            } else {
                                active_tools.remove(&step.step_index);
                            }
                        }
                        if step.is_answer()
                            && step.text_delta.as_deref().is_some_and(|d| !d.is_empty())
                        {
                            streamed = true;
                        }
                        if let Some(input) = step.context_input() {
                            context_input = Some(input);
                        }
                        self.on_step(&step);
                    }
                    Some(Event::Result(result)) => {
                        saw_result = true;
                        self.on_result(&result);
                        let err_msg = result.error.as_deref().unwrap_or("");
                        let can_retry_malformed = attempt + 1 < MAX_STREAM_ATTEMPTS
                            && !saw_tool
                            && !self.cancelled.load(Ordering::SeqCst)
                            && result.status != "SUCCESS"
                            && result.status != "CANCELED"
                            && result.status != "INTERRUPTED"
                            && is_malformed_function_call(err_msg);
                        if can_retry_malformed {
                            malformed_retry = true;
                            break;
                        }
                        // Once any tool was observed, its effects are uncertain. Let the hub
                        // continue from persisted work in a separate turn after this failure.
                        let transient = is_transient_stream_error(err_msg);
                        stream_timed_out = transient
                            && err_msg.to_ascii_lowercase().contains("stream idle timeout");
                        let can_retry = attempt + 1 < MAX_STREAM_ATTEMPTS
                            && !saw_tool
                            && !self.cancelled.load(Ordering::SeqCst)
                            && result.status != "SUCCESS"
                            && result.status != "CANCELED"
                            && result.status != "INTERRUPTED"
                            && transient;
                        if can_retry {
                            retry_stream = true;
                            break;
                        }
                        if !streamed && !result.response.is_empty() {
                            self.emit(SessionEventKind::Output {
                                stream: OutputStream::Text,
                                text: result.response.clone(),
                            });
                        }
                        stop = Some(self.stop_reason(&result));
                        break;
                    }
                    _ => {}
                }
            }
            if malformed_retry {
                drop(child);
                *self.running.lock().expect("running") = None;
                current_text = "Your previous tool call was empty or malformed. Re-issue the tool call with valid JSON arguments, or answer directly without a tool call.".to_owned();
                continue;
            }
            if retry_stream {
                live.process.start_kill();
                *child = None;
                drop(child);
                *self.running.lock().expect("running") = None;
                if streamed || saw_step {
                    current_text = "Your previous response stream dropped with a transient network EOF. If your intended tool calls and reply were already complete, reply concisely to close the turn without repeating completed tool calls; otherwise continue where you left off.".to_owned();
                } else {
                    current_text = original_text.clone();
                }
                tokio::time::sleep(Duration::from_millis(2000 * (attempt as u64 + 1))).await;
                continue;
            }
            if !saw_result && stop.is_none() && !self.cancelled.load(Ordering::SeqCst) {
                let exit = live.process.exit_within(EXIT_TIMEOUT).await;
                let what = exit
                    .map(|exit| exit.describe("Antigravity CLI"))
                    .unwrap_or_else(|| "Antigravity CLI stopped answering".into());
                let message = live
                    .stderr
                    .describe(&format!("{what} before the turn ended"))
                    .await;
                self.emit(SessionEventKind::Error { message });
                ended = true;
                stop = Some(StopReason::Error);
            }
            let stop_reason = stop.unwrap_or(StopReason::Cancelled);
            if !stream_timed_out || stop_reason != StopReason::Error {
                self.consecutive_stream_timeouts.store(0, Ordering::SeqCst);
            }
            if stop_reason == StopReason::Error {
                // Never keep a child alive after an error result: a timed-out or broken upstream
                // stream inside `agy` can leave the persistent process unable to run new turns.
                live.process.start_kill();
                ended = true;

                if stream_timed_out {
                    let previous = self.consecutive_stream_timeouts.fetch_add(1, Ordering::SeqCst);
                    let rotate_after = if self.coordinates_run() { 1 } else { 2 };
                    if previous + 1 == rotate_after {
                        // The hub supplies the lead's durable state to the new conversation.
                        self.rotate_context();
                        self.emit(SessionEventKind::Warning {
                            message: "Antigravity response timed out. The next turn will start a fresh model conversation and continue from saved work.".into(),
                        });
                    }
                }
            } else if stop_reason == StopReason::EndTurn {
                let max_context_tokens: u64 = if self.coordinates_run() {
                    100_000
                } else {
                    120_000
                };
                if let Some(input) = context_input.filter(|&input| input >= max_context_tokens) {
                    live.process.start_kill();
                    ended = true;
                    self.rotate_context();
                    tracing::debug!(input, max_context_tokens, "rotated Antigravity context");
                }
            }
            // A CLI that ended the run itself (a failed sign-in, say) leaves a dead process
            // behind: the next turn starts a new one, resuming the conversation by id.
            if ended
                || self.cancelled.load(Ordering::SeqCst)
                || live.lines.is_closed()
                || live.process.exit_info().is_some()
            {
                *child = None;
            }
            *self.running.lock().expect("running") = None;
            self.emit(SessionEventKind::TurnCompleted {
                turn_id: None,
                stop_reason,
            });
            return Ok(TurnOutcome {
                stop_reason,
                turn_id: None,
            });
        }
        unreachable!("retry loop always returns on the final attempt")
    }

    fn stop_reason(&self, result: &wire::Result) -> StopReason {
        if self.cancelled.load(Ordering::SeqCst) {
            return StopReason::Cancelled;
        }
        match result.status.as_str() {
            "SUCCESS" => StopReason::EndTurn,
            "CANCELED" | "INTERRUPTED" => StopReason::Cancelled,
            _ => {
                let message = result
                    .error
                    .clone()
                    .filter(|e| !e.is_empty())
                    .unwrap_or_else(|| {
                        format!(
                            "Antigravity CLI ended the turn as {}",
                            if result.status.is_empty() {
                                "unknown"
                            } else {
                                &result.status
                            }
                        )
                    });
                let lower = message.to_ascii_lowercase();
                if lower.contains("quota") || lower.contains("rate limit") || lower.contains("429")
                {
                    self.emit(SessionEventKind::RateLimits {
                        limits: crossagents_core::RateLimits {
                            windows: Vec::new(),
                            credits: None,
                            plan: None,
                            reached: Some(message.clone()),
                            by_limit: Vec::new(),
                            reset_credits: None,
                        },
                    });
                }
                self.emit(SessionEventKind::Error { message });
                StopReason::Error
            }
        }
    }

    fn stop(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(process) = self.running.lock().expect("running").as_ref() {
            process.start_kill();
        }
    }
}

fn is_malformed_function_call(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("malformed function call")
        || lower.contains("function call is empty")
        || lower.contains("improperly formatted function call")
}

/// Whether an Antigravity CLI result error reflects a transient SSE/proxy drop
/// that a fresh CLI process (`--conversation` resume) can recover from.
fn is_transient_stream_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if [
        "quota",
        "rate limit",
        "unauthor",
        "authenticat",
        "forbidden",
        "invalid model",
        "unknown model",
    ]
    .iter()
    .any(|fatal| lower.contains(fatal))
    {
        return false;
    }
    [
        ": eof",
        "unexpected eof",
        "connection reset",
        "broken pipe",
        "client connection lost",
        "tls handshake timeout",
        "server closed idle connection",
        "stream disconnected",
        "stream idle timeout",
        "context deadline exceeded",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// The name the GUI uses for a tool's icon.
fn tool_kind(name: &str) -> &'static str {
    match name {
        "run_command" => "execute",
        "write_to_file" | "replace_file_content" | "multi_replace_file_content" => "edit",
        "read_file" | "list_directory" | "find_by_name" | "view_file" => "read",
        "search" | "grep_search" | "codebase_search" | "google_web_search" => "search",
        "web_fetch" | "browser_navigate" => "fetch",
        _ => "tool",
    }
}

#[async_trait]
impl DriverSession for AntigravitySession {
    fn session_id(&self) -> &str {
        &self.id
    }

    fn take_context_rotated(&self) -> bool {
        self.context_rotated.swap(false, Ordering::SeqCst)
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

    async fn set_model(&self, model: Option<&str>, effort: Option<&str>) -> Result<(), CoreError> {
        if let Some(model) = model {
            *self.model.lock().expect("model") =
                Some(model.to_owned()).filter(|m: &String| !m.is_empty());
        }
        if let Some(effort) = effort {
            *self.effort.lock().expect("effort") =
                Some(effort.to_owned()).filter(|e: &String| !e.is_empty());
        }
        // The flags are read when the process starts, so the next turn starts a new one.
        // Between turns nothing runs; the conversation is resumed by id.
        let mut child = self.child.lock().await;
        if let Some(live) = child.take() {
            live.process.start_kill();
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.stop();
        let _guard = self.turn.lock().await;
        *self.child.lock().await = None;
    }
}

fn mcp_config_path(env: &[(String, String)]) -> Option<PathBuf> {
    let home = env
        .iter()
        .rev()
        .find(|(k, _)| k == "USERPROFILE" || k == "HOME")
        .map(|(_, v)| PathBuf::from(v))
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
        })?;
    Some(home.join(".gemini").join("config").join("mcp_config.json"))
}

/// Read the operator-owned grant for this exact agent. Keep the global permission
/// object, including ask and deny rules, so the session preserves their precedence.
fn tool_settings(settings_dir: &std::path::Path, grants: &[String]) -> std::io::Result<Value> {
    let global = match std::fs::read(settings_dir.join("settings.json")) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(err) => return Err(err),
    };
    let mut permissions = match global.get("permissions") {
        None => Map::new(),
        Some(Value::Object(rules)) => rules.clone(),
        Some(_) => {
            return Err(std::io::Error::other(
                "global permissions are not an object",
            ));
        }
    };
    let allow = permissions
        .entry("allow")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| std::io::Error::other("global permission allow list is not an array"))?;
    for grant in grants {
        let grant = json!(grant);
        if !allow.contains(&grant) {
            allow.push(grant);
        }
    }
    Ok(json!({ "permissions": permissions }))
}
/// Ensures `~/.gemini/config/mcp_config.json` registers each hub MCP server with a minimal
/// non-secret `env` marker so Antigravity forwards the session's own per-process credentials
/// without persisting tokens to disk.
fn ensure_mcp_config_at(path: &PathBuf, servers: &[McpServerSpec]) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).ok();
    let mut root = existing
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let mut entries = root
        .get("mcpServers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for server in servers {
        let mut entry = Map::new();
        entry.insert(
            "command".into(),
            Value::String(server.command.display().to_string()),
        );
        entry.insert(
            "args".into(),
            Value::Array(server.args.iter().cloned().map(Value::String).collect()),
        );
        entry.insert("env".into(), json!({ "CA_MCP_SESSION": "1" }));
        entries.insert(server.name.clone(), Value::Object(entry));
    }
    root["mcpServers"] = Value::Object(entries);
    let serialized = serde_json::to_string_pretty(&root).unwrap_or_default();
    if existing.as_deref().map(str::trim) == Some(serialized.trim()) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serialized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossagents_core::{Agent, AgentId, AgentStatus, Role};
    use std::path::Path;

    #[test]
    fn coordination_grants_keep_global_approval_and_exclude_shell() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"),
            r#"{"permissions":{"allow":["command(git status)"],"deny":["write_file(.git/)"],"ask":["mcp(crossagents/task_get)"]}}"#,
        ).unwrap();
        let grants = ["mcp(crossagents/task_get)".to_owned()];
        assert_eq!(
            tool_settings(dir.path(), &grants).unwrap(),
            json!({"permissions":{
                "allow":["command(git status)", "mcp(crossagents/task_get)"],
                "deny":["write_file(.git/)"],
                "ask":["mcp(crossagents/task_get)"]
            }})
        );
        assert!(LEAD_TOOLS.contains(&"human_ask"));
        assert!(!WORKER_TOOLS.contains(&"human_ask"));
        assert!(!REVIEW_TOOLS.contains(&"human_ask"));
        for tool in LEAD_TOOLS.iter().chain(WORKER_TOOLS).chain(REVIEW_TOOLS) {
            assert!(!matches!(
                *tool,
                "workspace_run" | "agent_send" | "run_land"
            ));
        }
    }
    #[test]
    fn existing_worker_leading_a_run_uses_coordination_watchdogs() {
        let dir = tempfile::tempdir().unwrap();
        let (mut lead, _) = spec(dir.path());
        lead.permission = PermissionPolicy::Guard;
        lead.env.push((COORDINATION_ENV.into(), "lead".into()));
        let lead = AntigravitySession::new("lead".into(), vec![], lead, None);
        assert!(lead.coordinates_run());
        assert_eq!(lead.coordination_idle_timeout(), Some(LEAD_IDLE_TIMEOUT));
        let prompt = lead.prompt_text(&Prompt::text("continue"));
        let scope = prompt.lines().find(|line| line.starts_with("CrossAgents MCP methods")).unwrap();
        assert!(scope.contains("task_list"));
        assert!(!scope.contains("agent_list"));
        assert!(prompt.contains("there are no per-tool JSON files"));
        assert!(prompt.contains("Do not follow it into another workspace"));
        assert!(prompt.contains("Do not query model quota through the shell"));
        assert!(prompt.contains(r#"task_rework takes {"task":"task-key","notes":"#));

        for role in ["worker", "reviewer"] {
            let (mut task, _) = spec(dir.path());
            task.permission = PermissionPolicy::Guard;
            task.env.push((COORDINATION_ENV.into(), role.into()));
            let task = AntigravitySession::new(role.into(), vec![], task, None);
            assert!(!task.coordinates_run());
            assert_eq!(task.coordination_idle_timeout(), Some(TASK_IDLE_TIMEOUT));
            let prompt = task.prompt_text(&Prompt::text("continue"));
            assert!(prompt.contains("use the provider’s list, search and read tools"));
            assert!(!prompt.contains("task_rework takes"));
        }

        let (uncoordinated, _) = spec(dir.path());
        let uncoordinated =
            AntigravitySession::new("uncoordinated".into(), vec![], uncoordinated, None);
        assert_eq!(uncoordinated.coordination_idle_timeout(), None);
    }

    #[test]
    fn a_rotated_context_reinstates_instructions_and_reports_its_new_id() {
        let dir = tempfile::tempdir().unwrap();
        let (mut spec, mut events) = spec(dir.path());
        spec.instructions = Some("original project instructions".into());
        let session = AntigravitySession::new("agent".into(), vec![], spec, Some("old".into()));
        let prompt = Prompt::text("continue");
        assert!(session.prompt_text(&prompt).starts_with("original project instructions"));
        assert_eq!(session.prompt_text(&prompt), "continue");

        session.rotate_context();
        assert!(session.take_context_rotated());
        assert!(!session.args().contains(&"--conversation".into()));
        assert!(session.prompt_text(&prompt).starts_with("original project instructions"));
        assert_eq!(session.prompt_text(&prompt), "continue");
        session.remember("new");
        session.remember("new");
        for expected in ["old", "new"] {
            match events.try_recv().unwrap().kind {
                SessionEventKind::Started { session_id } => assert_eq!(session_id, expected),
                event => panic!("expected a session id, got {event:?}"),
            }
        }
        assert!(events.try_recv().is_err());
    }

    fn spec(cwd: &Path) -> (SessionSpec, mpsc::UnboundedReceiver<SessionEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        (
            SessionSpec {
                agent: Agent {
                    node: None,
                    id: AgentId::new(),
                    name: "Antigravity".into(),
                    name_source: Default::default(),
                    plugin: "antigravity".into(),
                    driver: None,
                    model: Some("gemini-3.1-pro".into()),
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

    /// A mock CLI: logs its argv and every message it is given, then answers each with one
    /// stream. It stays up until its stdin closes, as the real one does.
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

    const STREAM: &str = r#"
{"event":"init","conversation_id":"conv-1","init":{"cwd":".","permission_mode":"request-review"}}
{"event":"step_update","step_update":{"conversation_id":"conv-1","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"hello "}}
{"event":"step_update","step_update":{"conversation_id":"conv-1","step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"world","usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}
{"event":"step_update","step_update":{"conversation_id":"conv-1","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hi"}}}}
{"event":"step_update","step_update":{"conversation_id":"conv-1","step_index":2,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hi"},"output":"hi\r\n"}}}
{"event":"result","result":{"conversation_id":"conv-1","status":"SUCCESS","response":"hello world","num_turns":1,"usage":{"input_tokens":10,"output_tokens":5,"thinking_tokens":1,"cache_read_tokens":2,"total_tokens":15}}}
"#;

    /// The mock's script: logs its argv and every message, then answers each with [`STREAM`]
    /// and waits for the next one. It stays up until its stdin closes, as the real CLI does.
    fn stream_for(windows: bool) -> String {
        let lines: Vec<&str> = STREAM.lines().filter(|l| !l.trim().is_empty()).collect();
        if windows {
            let mut script = String::from(
                "$ErrorActionPreference = 'Stop'\n\
                 $log = $env:CROSSAGENTS_AGY_TEST_LOG\n\
                 foreach ($a in $args) { Add-Content -Path $log -Value (\"ARG`t\" + $a) }\n\
                 while ($true) {\n\
                 \x20 $line = [Console]::In.ReadLine()\n\
                 \x20 if ($null -eq $line) { break }\n\
                 \x20 Add-Content -Path $log -Value (\"IN`t\" + $line)\n",
            );
            for line in lines {
                script.push_str(&format!("  [Console]::Out.WriteLine('{line}')\n"));
            }
            script.push_str("  [Console]::Out.Flush()\n}\n");
            script
        } else {
            let mut script = String::from(
                "log=\"$CROSSAGENTS_AGY_TEST_LOG\"\n\
                 for a in \"$@\"; do printf 'ARG\\t%s\\n' \"$a\" >> \"$log\"; done\n\
                 while IFS= read -r line; do\n\
                 \x20 printf 'IN\\t%s\\n' \"$line\" >> \"$log\"\n",
            );
            for line in lines {
                script.push_str(&format!(
                    "  printf '%s\\n' '{}'\n",
                    line.replace('\'', "'\\''")
                ));
            }
            script.push_str("done\n");
            script
        }
    }

    fn read_log(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// The argv the mock was started with, one entry per line it logged.
    fn logged_args(log: &str) -> Vec<String> {
        log.lines()
            .filter_map(|line| line.strip_prefix("ARG	").map(str::to_owned))
            .collect()
    }

    #[tokio::test]
    async fn a_turn_streams_the_answer_the_tool_and_the_totals() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.txt");
        let (mut spec, mut events) = spec(dir.path());
        spec.instructions = Some("be brief".into());
        spec.env = vec![("CROSSAGENTS_AGY_TEST_LOG".into(), log.display().to_string())];
        let command = mock_cli(dir.path(), &stream_for(true), &stream_for(false));
        let driver = AntigravityDriver::new("antigravity", Cli::at(command), vec![]);
        let session = driver.open(spec).await.unwrap();
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
        let mut started = Vec::new();
        let mut tools = 0;
        while let Ok(event) = events.try_recv() {
            match event.kind {
                SessionEventKind::Started { session_id } => started.push(session_id),
                SessionEventKind::Output { text: t, .. } => text.push_str(&t),
                SessionEventKind::ToolCall { .. } => tools += 1,
                SessionEventKind::Usage { tokens, .. } => usage = tokens,
                _ => {}
            }
        }
        // The CLI's conversation id, reported once, not the provisional one.
        assert_eq!(started, vec!["conv-1".to_owned()]);
        assert_eq!(text, "hello worldhello world");
        assert_eq!(tools, 2);
        let usage = usage.expect("usage");
        assert_eq!(usage.input, 10);
        assert_eq!(usage.cached_input, 2);
        assert_eq!(usage.output, 5);
        assert_eq!(usage.reasoning_output, 1);
        assert_eq!(usage.total, 15);

        let log = read_log(&log);
        // One process for both turns: the flags are logged once.
        assert_eq!(log.matches("ARG\t--input-format").count(), 1, "{log}");
        assert!(log.contains("ARG\tstream-json"), "{log}");
        assert!(log.contains("ARG\t--model"), "{log}");
        assert!(log.contains("ARG\tgemini-3.1-pro"), "{log}");
        assert!(log.contains("ARG\t--dangerously-skip-permissions"), "{log}");
        assert!(!log.contains("ARG	--conversation"), "{log}");
        assert_eq!(log.matches("be brief").count(), 1, "{log}");
        assert_eq!(log.matches("IN\t").count(), 2, "{log}");
        session.shutdown().await;
    }

    #[tokio::test]
    async fn only_current_context_rotates_the_session_and_rotation_is_quiet() {
        for (cached, expected_sessions) in [(200, 1), (99_990, 2)] {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("log.txt");
            let (mut spec, mut events) = spec(dir.path());
            spec.agent.role = Role::Planner;
            spec.env = vec![("CROSSAGENTS_AGY_TEST_LOG".into(), log.display().to_string())];
            let script = |windows| stream_for(windows)
                .replace(
                    r#""input_tokens":10,"output_tokens":5,"total_tokens":15"#,
                    &format!(r#""input_tokens":10,"output_tokens":5,"cache_read_tokens":{cached},"total_tokens":15"#),
                )
                .replace(r#""input_tokens":10,"output_tokens":5,"thinking_tokens""#,
                         r#""input_tokens":84000,"output_tokens":5,"thinking_tokens""#);
            let command = mock_cli(dir.path(), &script(true), &script(false));
            let driver = AntigravityDriver::new("antigravity", Cli::at(command), vec![]);
            let session = driver.open(spec).await.unwrap();
            for _ in 0..2 {
                assert_eq!(session.prompt("test").await.unwrap().stop_reason, StopReason::EndTurn);
            }
            assert_eq!(read_log(&log).matches("ARG\t--input-format").count(), expected_sessions);
            let mut usage = None;
            while let Ok(event) = events.try_recv() {
                match event.kind {
                    SessionEventKind::Usage { tokens, .. } => usage = tokens,
                    SessionEventKind::Warning { message } => panic!("{message}"),
                    _ => {}
                }
            }
            assert_eq!(usage.unwrap().input, 84_000);
            session.shutdown().await;
        }
    }

    #[tokio::test]
    async fn a_denying_session_keeps_the_default_mode() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.txt");
        let (mut spec, _events) = spec(dir.path());
        spec.permission = PermissionPolicy::Deny;
        spec.env = vec![("CROSSAGENTS_AGY_TEST_LOG".into(), log.display().to_string())];
        let command = mock_cli(dir.path(), &stream_for(true), &stream_for(false));
        let driver = AntigravityDriver::new("antigravity", Cli::at(command), vec![]);
        let session = driver.open(spec).await.unwrap();
        session.prompt("hi").await.unwrap();
        let args = logged_args(&read_log(&log));
        assert!(!args.contains(&"--dangerously-skip-permissions".to_owned()));
        assert!(!args.contains(&"--mode".to_owned()));
        session.shutdown().await;
    }

    #[tokio::test]
    async fn a_guarding_session_accepts_edits_only() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.txt");
        let (mut spec, _events) = spec(dir.path());
        spec.permission = PermissionPolicy::Guard;
        spec.env = vec![("CROSSAGENTS_AGY_TEST_LOG".into(), log.display().to_string())];
        let command = mock_cli(dir.path(), &stream_for(true), &stream_for(false));
        let driver = AntigravityDriver::new("antigravity", Cli::at(command), vec![]);
        let session = driver.open(spec).await.unwrap();
        session.prompt("hi").await.unwrap();
        let log = read_log(&log);
        assert!(log.contains("ARG\t--mode"), "{log}");
        assert!(log.contains("ARG\taccept-edits"), "{log}");
        assert!(!log.contains("--dangerously-skip-permissions"), "{log}");
        session.shutdown().await;
    }

    #[tokio::test]
    async fn resuming_names_the_conversation_before_the_first_turn() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.txt");
        let (mut spec, mut events) = spec(dir.path());
        spec.env = vec![("CROSSAGENTS_AGY_TEST_LOG".into(), log.display().to_string())];
        let command = mock_cli(dir.path(), &stream_for(true), &stream_for(false));
        let driver = AntigravityDriver::new("antigravity", Cli::at(command), vec![]);
        let session = driver.resume("conv-9", spec).await.unwrap();
        assert_eq!(session.session_id(), "conv-9");
        let started = match events.try_recv().map(|e| e.kind) {
            Ok(SessionEventKind::Started { session_id }) => session_id,
            other => panic!("expected Started, got {other:?}"),
        };
        assert_eq!(started, "conv-9");
        session.prompt("hi").await.unwrap();
        let log = read_log(&log);
        assert!(log.contains("ARG\t--conversation"), "{log}");
        assert!(log.contains("ARG\tconv-9"), "{log}");
        session.shutdown().await;
    }

    #[tokio::test]
    async fn an_error_result_ends_the_turn_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let windows = r#"
[Console]::Out.WriteLine('{"event":"result","result":{"conversation_id":"conv-1","status":"ERROR","response":"","error":"invalid model selection","num_turns":0}}')
[Console]::Out.Flush()
$null = [Console]::In.ReadToEnd()
"#;
        let unix = r#"printf '%s\n' '{"event":"result","result":{"conversation_id":"conv-1","status":"ERROR","response":"","error":"invalid model selection","num_turns":0}}'
cat >/dev/null
"#;
        let (spec, mut events) = spec(dir.path());
        let command = mock_cli(dir.path(), windows, unix);
        let driver = AntigravityDriver::new("antigravity", Cli::at(command), vec![]);
        let session = driver.open(spec).await.unwrap();
        let outcome = session.prompt("hi").await.unwrap();
        assert_eq!(outcome.stop_reason, StopReason::Error);
        let mut error = None;
        while let Ok(event) = events.try_recv() {
            if let SessionEventKind::Error { message } = event.kind {
                error = Some(message);
            }
        }
        assert_eq!(error.as_deref(), Some("invalid model selection"));
        session.shutdown().await;
    }

    #[tokio::test]
    async fn an_interrupt_stops_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let (spec, _events) = spec(dir.path());
        let command = mock_cli(dir.path(), "Start-Sleep -Seconds 60\n", "sleep 60\n");
        let session = Arc::new(AntigravitySession::new("id-1".into(), command, spec, None));
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

    #[test]
    fn mcp_capability_and_config_writer_preserve_user_entries_without_writing_secrets() {
        let driver = AntigravityDriver::new("antigravity", Cli::at(vec!["agy".into()]), vec![]);
        assert!(driver.capabilities().mcp);

        let dir = tempfile::tempdir().unwrap();
        let cfg = dir
            .path()
            .join(".gemini")
            .join("config")
            .join("mcp_config.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(
            &cfg,
            r#"{"mcpServers":{"custom":{"command":"foo","args":[]}}}"#,
        )
        .unwrap();

        let server = McpServerSpec {
            name: "crossagents".into(),
            command: PathBuf::from("C:\\bin\\crossagents.exe"),
            args: vec!["mcp".into()],
            env: [("CROSSAGENTS_AGENT_TOKEN".into(), "secret-token".into())]
                .into_iter()
                .collect(),
        };
        ensure_mcp_config_at(&cfg, &[server]).unwrap();
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(!written.contains("secret-token"));
        let parsed: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["mcpServers"]["custom"]["command"], "foo");
        assert_eq!(parsed["mcpServers"]["crossagents"]["args"], json!(["mcp"]));
    }

    #[tokio::test]
    async fn a_transient_eof_restarts_and_recovers_without_error() {
        verify_eof_recovery(false).await;
    }

    #[tokio::test]
    async fn a_transient_eof_after_a_tool_does_not_retry_the_turn() {
        verify_eof_recovery(true).await;
    }

    async fn verify_eof_recovery(after_tool: bool) {
        let dir = tempfile::tempdir().unwrap();
        let step = r#"{"event":"step_update","step_update":{"conversation_id":"conv-eof","step_index":1,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo marker"}}}}"#;
        let windows_step = if after_tool { format!("Write-Output '{step}'") } else { String::new() };
        let unix_step = if after_tool { format!("printf '%s\\n' '{step}'") } else { String::new() };
        let marker = dir.path().join("attempt.txt");
        let windows = format!(
            r#"
$marker = '{marker}'
$null = [Console]::In.ReadLine()
if (-not (Test-Path $marker)) {{
    Set-Content -Path $marker -Value '1'
    Write-Output '{{"event":"init","conversation_id":"conv-eof"}}'
    {windows_step}
    Write-Output '{{"event":"result","result":{{"conversation_id":"conv-eof","status":"ERROR","response":"","error":"API error (attempt 1): request failed: Post \"https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse\": EOF","num_turns":0}}}}'
}} else {{
    Write-Output '{{"event":"init","conversation_id":"conv-eof"}}'
    Write-Output '{{"event":"result","result":{{"conversation_id":"conv-eof","status":"SUCCESS","response":"recovered ok","num_turns":1}}}}'
}}
[Console]::In.ReadToEnd() | Out-Null
"#,
            marker = marker.display()
        );
        let unix = format!(
            r#"read -r _
if [ ! -f '{marker}' ]; then
    echo 1 > '{marker}'
    printf '%s\n' '{{"event":"init","conversation_id":"conv-eof"}}'
    {unix_step}
    printf '%s\n' '{{"event":"result","result":{{"conversation_id":"conv-eof","status":"ERROR","response":"","error":"API error (attempt 1): request failed: Post \"https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse\": EOF","num_turns":0}}}}'
else
    printf '%s\n' '{{"event":"init","conversation_id":"conv-eof"}}'
    printf '%s\n' '{{"event":"result","result":{{"conversation_id":"conv-eof","status":"SUCCESS","response":"recovered ok","num_turns":1}}}}'
fi
cat >/dev/null
"#,
            marker = marker.display()
        );
        let (spec, mut events) = spec(dir.path());
        let command = mock_cli(dir.path(), &windows, &unix);
        let driver = AntigravityDriver::new("antigravity", Cli::at(command), vec![]);
        let session = driver.open(spec).await.unwrap();
        let outcome = session.prompt("hi").await.unwrap();
        let expected = if after_tool { StopReason::Error } else { StopReason::EndTurn };
        assert_eq!(outcome.stop_reason, expected);
        let mut saw_text = false;
        let mut saw_tool_call = false;
        let mut saw_error = false;
        while let Ok(event) = events.try_recv() {
            match event.kind {
                SessionEventKind::Error { message } => {
                    assert!(message.contains("EOF"), "{message}");
                    saw_error = true;
                }
                SessionEventKind::ToolCall { .. } => {
                    saw_tool_call = true;
                }
                SessionEventKind::Output { text, .. } if text == "recovered ok" => {
                    saw_text = true;
                }
                _ => {}
            }
        }
        assert_eq!(saw_text, !after_tool);
        assert_eq!(saw_error, after_tool);
        assert_eq!(saw_tool_call, after_tool);
        session.shutdown().await;
    }
}
