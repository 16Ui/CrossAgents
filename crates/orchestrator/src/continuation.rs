//! Opt-in, bounded run supervision. Observe durable facts, then ask the lead to decide.
//! Never replay commands, accept work, answer questions or bypass landing checks here.

use chrono::{DateTime, Duration, Utc};
use crossagents_core::{
    Agent, AgentId, AgentKey, AgentStatus, ContinuationState, CoreError, EventKind, HumanItem,
    HumanItemKind, HumanItemStatus, MessageSource, Run, RunId, RunStatus, TaskStatus,
    model::{ProviderFallback, RecoveryWatch, StuckWatch},
};

use crate::Orchestrator;

const UNCHANGED_RECOVERY_LIMIT: u32 = 2;
const PROVIDER_STREAM_FAILURE: &str = "provider-stream-failure";

fn known_provider_stream_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    if message.starts_with("antigravity coordination stream timed out with no tool running;")
        || message == "failed to start conversation: backend returned an empty conversation id"
    {
        return true;
    }
    message.starts_with("api error")
        && !["quota", "rate limit", "unauthor", "authenticat", "forbidden"]
            .iter()
            .any(|marker| message.contains(marker))
        && ["stream idle timeout", ": eof", "unexpected eof", "connection reset", "stream disconnected"]
            .iter()
            .any(|marker| message.contains(marker))
}

fn verified_local_checkpoint(run: &Run) -> Option<std::path::PathBuf> {
    if run.versioned() {
        return None;
    }
    let root = dunce::canonicalize(&run.repo).ok()?;
    let path = dunce::canonicalize(root.join("CHECKPOINT.md")).ok()?;
    (path.starts_with(&root) && path.is_file()).then_some(path)
}

fn provider_retry_at(
    last_attempt: DateTime<Utc>,
    backoff: Duration,
    checkpoint_changed: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let normal = last_attempt + backoff;
    if let Some(changed) = checkpoint_changed.filter(|at| *at > last_attempt && *at <= now) {
        // A new, real checkpoint can justify a single earlier probe. Once reserved, the
        // attempt time is newer than this checkpoint and the regular cooldown applies.
        normal.min(changed.max(last_attempt + Duration::minutes(5)))
    } else {
        normal
    }
}

/// A long checkpoint is evidence for recovery, not extra conversation history.
async fn checkpoint_tail(path: &std::path::Path) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    const MAX_BYTES: u64 = 2048;
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let start = file.metadata().await.ok()?.len().saturating_sub(MAX_BYTES);
    file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
    let mut bytes = Vec::with_capacity(MAX_BYTES as usize);
    file.take(MAX_BYTES).read_to_end(&mut bytes).await.ok()?;
    let tail = if start > 0 {
        let newline = bytes.iter().position(|byte| *byte == b'\n')?;
        String::from_utf8_lossy(&bytes[newline + 1..])
    } else {
        String::from_utf8_lossy(&bytes)
    };
    let tail = tail.trim();
    (!tail.is_empty()).then(|| tail.to_owned())
}

/// Transient outages get a progressively longer cooling period, within the
/// run's existing wakeup cap. Two consecutive timeouts wait at least 15 minutes.
fn provider_cooldown(repeats: u32) -> Duration {
    let shift = repeats.saturating_sub(2).min(3);
    Duration::minutes(15_i64 << shift)
}
/// A phase that waits on a busy agent, named so a report can say who is being waited for.
struct Waiting {
    /// "the lead «Luna»" or "agent «w-3» on #a (title)": how the lead should read it.
    what: String,
}

/// Allow a single new turn after a known transport failure. Permission and
/// unknown failures remain stopped. An actual successful turn re-arms the probe.
fn temporary_provider_failure(
    events: &[crossagents_core::Event],
    reserved_at: Option<DateTime<Utc>>,
) -> bool {
    if let Some(at) = reserved_at
        && !events.iter().rev().take_while(|event| event.ts > at).any(|event| {
            matches!(event.kind, EventKind::AgentTurnCompleted {
                stop_reason: crossagents_core::StopReason::EndTurn, ..
            })
        })
    {
        return false;
    }

    // The runtime trips after three failures. An older failure is not part
    // of that circuit break; a denial in this streak still vetoes the probe.
    let mut transient = false;
    let mut completed = 0;
    for event in events.iter().rev() {
        match &event.kind {
            EventKind::AgentTurnCompleted {
                stop_reason: crossagents_core::StopReason::EndTurn, ..
            } => return transient,
            EventKind::AgentTurnCompleted { .. } => {
                completed += 1;
                if completed > 3 {
                    return transient;
                }
            }
            EventKind::AgentError { message, .. } => {
                let message = message.to_ascii_lowercase();
                if message.starts_with("circuit breaker: agent hit consecutive error limit") {
                    continue;
                }
                if !known_provider_stream_failure(&message) {
                    return false;
                }
                transient = true;
            }
            EventKind::AgentToolDetail { detail, .. }
                if detail.error.as_deref().and_then(crate::engine::permission_denial).is_some() =>
            {
                return false;
            }
            _ => {}
        }
    }
    transient
}

struct LeadFailure {
    key: String,
    detail: String,
    first_turn: crossagents_core::MessageId,
    latest_at: DateTime<Utc>,
    repeats: u32,
}

/// Use completed turns only. A new response without this failure breaks the streak.
fn recent_lead_failure(events: &[crossagents_core::Event]) -> Option<LeadFailure> {
    let mut completed = None;
    let mut detail = None;
    let mut denial = None;
    let mut streak: Option<LeadFailure> = None;
    for event in events.iter().rev() {
        match &event.kind {
            EventKind::AgentTurnCompleted { message_id, stop_reason, .. } => {
                if completed.is_some() {
                    break;
                }
                completed = Some((*message_id, *stop_reason, event.ts));
            }
            EventKind::AgentToolDetail { detail, .. } if completed.is_some() => {
                if let Some(reason) = detail.error.as_deref().and_then(crate::engine::permission_denial) {
                    denial.get_or_insert_with(|| reason.to_owned());
                }
            }
            EventKind::AgentToolUsed { tool, ok: true, .. } if completed.is_some()
                && matches!(tool.as_str(), "task_rework" | "task_create" | "task_assign" | "task_answer" | "run_finish") =>
            {
                // The lead made a durable coordination decision in this turn.
                break;
            }
            EventKind::AgentError { message, .. } if completed.is_some() => {
                let message = crate::engine::first_line(message, 240);
                if !message.starts_with("Circuit breaker:") {
                    detail.get_or_insert(message);
                }
            }
            EventKind::AgentTurnStarted { message_id, .. } => {
                let Some((turn_id, reason, ended_at)) = completed.take() else { continue };
                if *message_id != turn_id {
                    break;
                }
                let failure = denial.take().map(|reason| (reason.clone(), reason)).or_else(|| {
                    let message = detail.take()?;
                    if reason != crossagents_core::StopReason::Error {
                        return None;
                    }
                    let key = if known_provider_stream_failure(&message) {
                        PROVIDER_STREAM_FAILURE.to_owned()
                    } else {
                        message.clone()
                    };
                    Some((key, message))
                });
                detail = None;
                if failure.is_none() && matches!(reason, crossagents_core::StopReason::Cancelled | crossagents_core::StopReason::Interrupted) {
                    // A hub restart or an interrupted probe does not prove the stream recovered.
                    continue;
                }
                let Some((key, detail)) = failure else { break };
                match &mut streak {
                    Some(streak) if streak.key == key => {
                        streak.first_turn = turn_id;
                        streak.repeats += 1;
                    }
                    Some(_) => break,
                    None => streak = Some(LeadFailure {
                        key, detail, first_turn: turn_id, latest_at: ended_at, repeats: 1,
                    }),
                }
            }
            _ => {}
        }
    }
    streak
}

/// One observation of a run.
struct Step {
    phase: &'static str,
    next_step: String,
    /// Whether this observation may wake the lead once the persisted interval allows.
    eligible: bool,
    /// Set for the phases that wait on a busy agent (the only ones a stuck watch applies to).
    waiting: Option<Waiting>,
}

impl Orchestrator {
    /// Called after budget settlement by the hub clock. Each run is isolated: an error
    /// reading or delivering one run must not prevent supervision of another.
    pub async fn continue_runs(&self) {
        let runs = match self.store.list_runs().await {
            Ok(runs) => runs,
            Err(error) => {
                tracing::warn!(%error, "cannot list runs for continuation");
                return;
            }
        };
        for run in runs {
            if run.config.policy.continuation.is_none() {
                if run.continuation.as_ref().and_then(|state| state.provider_fallback.as_ref())
                    .is_none_or(|probe| !probe.active)
                {
                    continue;
                }
            } else if let Err(error) = self.accept_verified_from_clock(&run).await {
                tracing::warn!(%error, run = %run.id, "cannot integrate verified task");
            }
            if let Err(error) = self.continue_run(run.id, true, None).await {
                tracing::warn!(%error, run = %run.id, "cannot continue run");
            }
        }
    }

    /// The lead's turn ended: rewrite the checkpoint from the facts as they are now.
    ///
    /// The clock wrote "working" while the lead was in the turn a wakeup bought, with nothing
    /// due; a restart derives "continue" (or another settled phase) and reserves the next
    /// wakeup afresh, so the run read differently before and after a restart. Doing the same
    /// observation here, as soon as the agent is idle, keeps the two the same without waiting
    /// for the next tick.
    ///
    /// Nothing is woken and no attempt is counted ([`Self::continue_run`] with `deliver`
    /// false): the lead has just finished the turn, and a due time that passed during it is
    /// left due for the clock to deliver.
    ///
    /// The stuck watch is not judged here either: a quiet stretch that has already run out
    /// must reach the lead as the report it is, so only a delivery pass does the judging
    /// (see the branch in [`Self::continue_run`]).
    pub async fn settle_continuation(&self, lead: AgentId) {
        let runs = match self.store.list_runs().await {
            Ok(runs) => runs,
            Err(error) => {
                tracing::warn!(%error, "cannot list runs to settle a continuation");
                return;
            }
        };
        for run in runs {
            if run.planner != lead || run.continuation.is_none() {
                continue;
            }
            if let Err(error) = self.continue_run(run.id, false, None).await {
                tracing::warn!(%error, run = %run.id, "cannot settle the run's continuation");
            }
        }
    }

    /// The run's durable facts, and the arm of its stuck watch, moved back by `ago`: the
    /// stand-in a test uses for elapsed time, since the hub itself never moves these
    /// backwards. Nothing outside such a test has a use for it.
    ///
    /// It holds the run's lock — the one every continuation pass takes around its whole
    /// read-modify-write — while it rewrites the record. A rewrite from outside that lock
    /// races a pass that read the run just before it and writes that copy back a few
    /// milliseconds later, silently undoing the fabricated stretch; the test then sees no
    /// overdue watch for a reason nothing in the run shows.
    #[doc(hidden)]
    pub async fn age_run_records_for_test(
        &self,
        id: RunId,
        ago: Duration,
    ) -> Result<(), CoreError> {
        let _guard = self.lock(id).await;
        let mut run = self.store.get_run(id).await?;
        run.updated_at -= ago;
        if let Some(watch) = run
            .continuation
            .as_mut()
            .and_then(|continuation| continuation.stuck_watch.as_mut())
        {
            watch.progress_at -= ago;
        }
        self.store.update_run(&run).await?;
        for mut task in self.store.list_tasks(id).await? {
            task.updated_at -= ago;
            self.store.update_task(&task).await?;
        }
        Ok(())
    }

    /// The agent, when it counts as busy: its turn is starting or running, or the host holds
    /// queued messages for it. This says a process holds the agent, nothing about whether it
    /// is getting anywhere.
    async fn continuation_busy(&self, id: AgentId) -> Result<Option<Agent>, CoreError> {
        let agent = self.store.get_agent(&AgentKey::Id(id)).await?;
        let busy = matches!(agent.status, AgentStatus::Running | AgentStatus::Starting)
            || self.host.is_busy(id).await;
        Ok(busy.then_some(agent))
    }

    async fn continuation_step(&self, run: &Run) -> Result<Step, CoreError> {
        let waiting = |phase, reason: String| Step {
            phase,
            next_step: reason,
            eligible: false,
            waiting: None,
        };
        if run.status.is_terminal() {
            return Ok(waiting("ended", "Run ended; no further wakeups.".into()));
        }
        if run.status == RunStatus::Paused {
            return Ok(waiting(
                "paused",
                run.note.clone().unwrap_or_else(|| "Run paused.".into()),
            ));
        }
        if run.status == RunStatus::AwaitingApproval {
            return Ok(waiting(
                "waiting_approval",
                "Wait for the person to approve the plan.".into(),
            ));
        }
        let lead = self.store.get_agent(&AgentKey::Id(run.planner)).await?;
        if lead.archived_at.is_some() {
            return Ok(waiting(
                "unavailable",
                "The lead is archived; restore it before continuing.".into(),
            ));
        }
        if self
            .store
            .list_human_items(true)
            .await?
            .iter()
            .any(|item| item.run_id == Some(run.id) && item.kind == HumanItemKind::Question)
        {
            return Ok(waiting(
                "waiting_human",
                "Wait for the open human question or approval.".into(),
            ));
        }
        if lead.status == AgentStatus::Error {
            if !self.host.is_busy(lead.id).await && self.lead_provider_recoverable(run).await? {
                return Ok(Step {
                    phase: "provider_recover",
                    next_step: "The provider stream failed temporarily and the lead stopped. Inspect the current checkpoint and continue from durable work without replaying previously executed tools.".into(),
                    eligible: true,
                    waiting: None,
                });
            }
            return Ok(waiting(
                "error",
                format!(
                    "The lead «{}» is in an error state. Inspect its logs or restart it before continuing.",
                    lead.name
                ),
            ));
        }
        if let Some(busy) = self.continuation_busy(run.planner).await? {
            return Ok(Step {
                phase: "working",
                next_step: "The lead has a turn or queued messages; wait for it.".into(),
                eligible: false,
                waiting: Some(Waiting {
                    what: format!("the lead «{}»", busy.name),
                }),
            });
        }
        if !run.versioned() {
            return Ok(Step {
                phase: "continue",
                next_step: "Continue the work directly in the assigned project directory. Report required validation or a concrete blocker.".into(),
                eligible: true,
                waiting: None,
            });
        }
        let mut tasks = self.store.list_tasks(run.id).await?;
        for task in tasks.iter_mut().filter(|task| task.status == TaskStatus::Running) {
            if let Some(id) = task.assignee
                && let Ok(worker) = self.store.get_agent(&AgentKey::Id(id)).await
                && worker.status == AgentStatus::Stopped
                && !self.host.is_busy(id).await
                && let Some(reason) = crate::approval::unusable_on_platform(
                    &worker, Some(&run.config.policy), cfg!(windows),
                )
            {
                crate::attempts::close(task, crossagents_core::TaskRunOutcome::AgentError,
                    Some(reason.clone()), false);
                task.report.failure = Some(reason);
                self.transition(task, TaskStatus::Stalled).await?;
            }
        }
        // A restarted hub can mark an adopted worker stalled. Its actual running turn
        // still wins over that label. Never use silence or elapsed time to interrupt it.
        // (The opt-in stuck watch below may *report* a quiet busy run to the lead; it still
        // never interrupts, and a live turn stays authoritative here.)
        for task in tasks.iter().filter(|task| !task.status.is_terminal()) {
            for agent in [task.assignee, task.reviewer].into_iter().flatten() {
                if let Some(busy) = self.continuation_busy(agent).await? {
                    return Ok(Step {
                        phase: "waiting_tasks",
                        next_step: format!(
                            "Wait for #{} and its dependents; an agent is working or has queued messages.",
                            task.key
                        ),
                        eligible: false,
                        waiting: Some(Waiting {
                            what: format!(
                                "agent «{}» on #{} ({})",
                                busy.name, task.key, task.title
                            ),
                        }),
                    });
                }
            }
            if task.status == TaskStatus::Verifying {
                return Ok(Step {
                    phase: "waiting_checks",
                    next_step: format!("Wait for #{} verification.", task.key),
                    eligible: false,
                    waiting: Some(Waiting {
                        what: format!("the verification of #{} ({})", task.key, task.title),
                    }),
                });
            }
        }
        for member in &run.config.team {
            if *member == run.planner {
                continue;
            }
            if let Some(busy) = self.continuation_busy(*member).await? {
                return Ok(Step {
                    phase: "waiting_teammates",
                    next_step: format!(
                        "Wait for teammate «{}»; it is working or has queued messages.",
                        busy.name
                    ),
                    eligible: false,
                    waiting: Some(Waiting {
                        what: format!("teammate «{}»", busy.name),
                    }),
                });
            }
        }
        if tasks.is_empty()
            && let Ok(all) = self.store.list_agents().await
        {
            for agent in all {
                if agent.id == run.planner
                    || agent.archived_at.is_some()
                    || run.config.team.contains(&agent.id)
                    || agent.cwd != run.repo
                {
                    continue;
                }
                if let Some(busy) = self.continuation_busy(agent.id).await? {
                    return Ok(Step {
                        phase: "waiting_teammates",
                        next_step: format!(
                            "Wait for teammate «{}»; it is working or has queued messages.",
                            busy.name
                        ),
                        eligible: false,
                        waiting: Some(Waiting {
                            what: format!("teammate «{}»", busy.name),
                        }),
                    });
                }
            }
        }
        if !tasks.is_empty() && tasks.iter().all(|task| task.status.is_terminal()) {
            return Ok(Step {
                phase: "finalize",
                next_step: "All tasks ended. Check the original request, failed/cancelled tasks and integration results. Repair missing work if needed; only then run the project checks and finish/land as policy permits.".into(),
                eligible: true,
                waiting: None,
            });
        }
        if tasks.is_empty() {
            return Ok(Step {
                phase: "continue",
                next_step: "No task is executing. Inspect the run checkout and original request; continue implementation or verify and finish. Do not treat a completed turn as a completed request.".into(),
                eligible: true,
                waiting: None,
            });
        }
        Ok(Step {
            phase: "recover",
            next_step: "No task agent is executing. Inspect task reports, questions, dependencies, diffs and check results; answer worker questions, recover stalled work, review completed work or repair failures. Do not blindly replay commands.".into(),
            eligible: true,
            waiting: None,
        })
    }

    /// How long the run has shown no progress, when a stuck watch is on and that stretch is
    /// at least `policy.stuck_minutes`. Advances `watch` over the durable facts it reads.
    ///
    /// Progress is a fact the hub already persisted, never the mere absence of words:
    ///
    /// - a newer `updated_at` on the run or on any of its tasks. Every task mutation goes
    ///   through `store_task`/`transition` (`task_progress`, reports, status changes,
    ///   handoffs, reroutes), and every run mutation through `set_run_status`/landing, so a
    ///   task or run that is actually getting somewhere keeps moving these timestamps.
    /// - an event one of the run's own agents appended since the last look: a turn starting,
    ///   output, a tool call, a message. An agent that is producing work is streaming these.
    ///
    /// Both are durable, so a restarted hub reads the same quiet stretch out of the run JSON
    /// instead of forgetting it. Neither proves work is happening: a turn stuck inside one
    /// very slow command (a compile, a fetch, a model call that has not streamed a token yet)
    /// writes neither, and would look quiet after the threshold. That is why the timeout only
    /// ever tells the lead — who can look at the checkout and decide — and why it is off by
    /// default and measured in minutes, not seconds.
    async fn stuck_progress(
        &self,
        run: &Run,
        watch: &mut StuckWatch,
        now: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let mut records_at = run.updated_at;
        let mut agents = vec![run.planner];
        agents.extend(run.config.team.iter().copied());
        for task in self.store.list_tasks(run.id).await? {
            records_at = records_at.max(task.updated_at);
            agents.extend(task.assignee);
            agents.extend(task.reviewer);
        }
        let latest = self.store.latest_event_seq().await?;
        if latest > watch.event_seq {
            let mut after = watch.event_seq;
            while after < latest {
                let page = self.store.events_since(after, 500).await?;
                let Some(end) = page.last().map(|event| event.seq) else {
                    break;
                };
                for event in page {
                    if event.kind.agent_id().is_some_and(|id| agents.contains(&id)) {
                        watch.progress_at = watch.progress_at.max(event.ts);
                    }
                }
                after = end;
            }
            // Only events after this point are new; keeps the next look cheap.
            watch.event_seq = latest;
        }
        // A record timestamp is at most this observation, and the bookmark never runs
        // ahead of it either: the reported stretch must not be a negative number if the
        // hub's clock moved backwards.
        watch.progress_at = watch.progress_at.max(records_at).min(now);
        Ok(())
    }

    /// A notice for the person about the run, through the existing human-item channel.
    /// Callers persist the flag that makes this the only time a fact is reported before
    /// calling, so a duplicate is never made even if the hub stops right after.
    async fn continuation_notice(&self, run: &Run, text: String) -> Result<(), CoreError> {
        let item = HumanItem {
            id: format!("n-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
            detail: None,
            kind: HumanItemKind::Notice,
            from: run.planner,
            run_id: Some(run.id),
            task_id: None,
            text,
            options: Vec::new(),
            default: None,
            timeout_at: None,
            status: HumanItemStatus::Open,
            answer: None,
            answered_at: None,
            action: None,
            created_at: Utc::now(),
        };
        self.store.put_human_item(&item).await?;
        self.publish(EventKind::HumanAsked { item }).await;
        Ok(())
    }

    /// Count a deadlocked batch as one recovery case even when it contains pending work.
    /// A runnable task, genuine in-flight review or human decision is handled separately.
    async fn stalled_recovery(
        &self,
        run: &Run,
    ) -> Result<Option<(String, DateTime<Utc>, String)>, CoreError> {
        let tasks = self.store.list_tasks(run.id).await?;
        let mut stalled: Vec<_> = tasks
            .iter()
            .filter(|task| !task.status.is_terminal())
            .collect();
        if stalled.is_empty()
            || stalled.iter().any(|task| !matches!(task.status,
                TaskStatus::Stalled | TaskStatus::Pending | TaskStatus::Passed | TaskStatus::Reviewing))
            || crate::plan::ready_tasks_matching(&tasks, run.config.max_parallel, |task| {
                !crate::engine::previously_refused_dispatch(task)
            }).iter().any(|task| task.status == TaskStatus::Pending)
        {
            return Ok(None);
        }
        stalled.sort_by_key(|task| task.id.to_string());
        let mut affected = Vec::new();
        let mut fingerprint = Vec::new();
        let mut changed_at = stalled[0].updated_at;
        for task in stalled {
            changed_at = changed_at.max(task.updated_at);
            let detail = task
                .report
                .failure
                .as_deref()
                .or_else(|| task.runs.last()?.note.as_deref())
                .or(task.progress.as_deref())
                .unwrap_or(match task.status {
                    TaskStatus::Pending => "尚未派出。检查依赖、写入范围或占用并行位的停滞任务。任务计划里的文件路径不能当成已有产物。",
                    TaskStatus::Passed => "报告已提交。读取验收结论与检查结果，核对是否可以正常合入。",
                    TaskStatus::Reviewing => "等待独立评审结果。先核对评审是否仍然在执行。",
                    _ => "执行者结束时没有提交结果，具体操作见任务日志",
                });
            // Reassigning the same blocked task can write an identical task row
            // and still advance updated_at. Only a different blocker or progress
            // should buy another recovery window.
            fingerprint.push(serde_json::json!([
                task.id,
                task.status,
                task.depends_on,
                task.spec.stage,
                task.spec.scope,
                task.progress,
                task.report.verdict,
                task.report.verified_sha,
                task.report.interrupted,
                detail,
            ]).to_string());
            affected.push(format!(
                "#{} — {}",
                task.key,
                super::engine::first_line(detail, 220)
            ));
        }
        Ok(Some((
            fingerprint.join("|"),
            changed_at,
            affected.join("\n"),
        )))
    }

    /// Recover bounded evidence from the old scheduler on upgrade, without counting
    /// wakeups before the most recent change to the affected tasks.
    async fn previous_recovery_wakes(
        &self,
        run: &Run,
        changed_at: DateTime<Utc>,
        attempts: u32,
    ) -> Result<u32, CoreError> {
        if attempts == 0 {
            return Ok(0);
        }
        let after = self.store.latest_event_seq().await?.saturating_sub(8_000);
        let topic = format!("continuation:{}", run.id);
        let count = self
            .store
            .agent_events(run.planner, after, 8_000)
            .await?
            .into_iter()
            .filter(|event| {
                event.ts > changed_at
                    && matches!(&event.kind, EventKind::NoticeRecorded { topic: sent, line, .. }
                    if *sent == topic && line.contains("No task agent is executing."))
            })
            .take(UNCHANGED_RECOVERY_LIMIT as usize)
            .count() as u32;
        Ok(count.min(attempts))
    }
    async fn lead_provider_recoverable(&self, run: &Run) -> Result<bool, CoreError> {
        let reserved_at = run.continuation.as_ref().and_then(|state| state.provider_recovery_at);
        let after = self.store.latest_event_seq().await?.saturating_sub(8_000);
        let events = self.store.agent_events(run.planner, after, 8_000).await?;
        Ok(temporary_provider_failure(&events, reserved_at))
    }

    /// Select one independent model already registered in this managed run, leaving the
    /// account, lead identity, permissions and task claims intact.
    async fn lead_transport_fallback(
        &self, run: &Run, state: &ContinuationState, failure: &LeadFailure,
    ) -> Result<Option<ProviderFallback>, CoreError> {
        if failure.key != PROVIDER_STREAM_FAILURE
            || failure.repeats < UNCHANGED_RECOVERY_LIMIT
            || state.provider_fallback.as_ref().is_some_and(|probe| probe.failure == failure.first_turn)
            || !self.lead_manages_questions(run).await
        {
            return Ok(None);
        }
        let lead = self.store.get_agent(&AgentKey::Id(run.planner)).await?;
        let Some(primary) = lead.model.as_deref() else { return Ok(None) };
        if lead.plugin != "antigravity" || !primary.starts_with("gemini-4-neon-") {
            return Ok(None);
        }
        // Use a model the operator already assigned to an existing working role.
        for id in std::iter::once(run.config.worker)
            .chain(run.config.reviewer)
            .chain(run.config.team.iter().copied())
        {
            let Ok(member) = self.store.get_agent(&AgentKey::Id(id)).await else { continue };
            if member.id != lead.id && member.plugin == lead.plugin && member.archived_at.is_none()
                && let Some(model) = member.model.filter(|model| model.starts_with("gemini-4-hydrogen-"))
            {
                return Ok(Some(ProviderFallback {
                    failure: failure.first_turn,
                    primary: primary.to_owned(),
                    alternate: model,
                    active: true,
                }));
            }
        }
        Ok(None)
    }

    pub(crate) async fn continue_run(
        &self,
        id: RunId,
        deliver: bool,
        now: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError> {
        let _guard = self.lock(id).await;
        let mut run = self.store.get_run(id).await?;
        if deliver && let Some(fallback) = run.continuation.as_mut()
            .and_then(|state| state.provider_fallback.as_mut()).filter(|probe| probe.active)
            && !self.host.is_busy(run.planner).await
        {
            let lead = self.store.get_agent(&AgentKey::Id(run.planner)).await?;
            // Do not undo a manual change to another model. A restart between reserving
            // and delivering the probe also restores the primary without replaying it.
            if lead.model.as_deref() == Some(&fallback.alternate) {
                if !self.host.renew_session(run.planner).await? {
                    return Ok(());
                }
                self.host.set_model(run.planner, Some(fallback.primary.clone())).await?;
            }
            fallback.active = false;
            if run.config.policy.continuation.is_none() {
                run.continuation = None;
            }
            self.store.update_run(&run).await?;
        }
        let Some(policy) = run.config.policy.continuation.clone() else {
            return Ok(());
        };
        // The automation breaker stopped these wakeups (docs/research/multica.md §2.14): only a
        // person re-arming the run (`reset_attempts`, which clears the health) starts them again.
        if run
            .continuation
            .as_deref()
            .is_some_and(|state| state.health.is_paused())
        {
            return Ok(());
        }
        let now = now.unwrap_or_else(Utc::now);
        if deliver && run.status == RunStatus::Running && run.versioned()
            && !self.host.is_busy(run.planner).await
        {
            let tasks = self.store.list_tasks(id).await?;
            let has_ready = !crate::plan::ready_tasks_matching(
                &tasks,
                run.config.max_parallel,
                |task| !crate::engine::previously_refused_dispatch(task),
            ).is_empty();
            if (has_ready || !crate::plan::blocked_tasks(&tasks).is_empty())
                && self.store.list_human_items(true).await?.iter().all(|item|
                    item.run_id != Some(id) || item.kind != HumanItemKind::Question)
                && self.host.continuation_dispatch_allowed(run.planner).await.is_ok()
            {
                self.schedule_pending(&mut run, false).await?;
            }
        }
        let step = self.continuation_step(&run).await?;
        let observed_phase = step.phase;
        let old = run.continuation.as_deref().cloned();
        let mut state = old.clone().unwrap_or(ContinuationState {
            owner: run.planner,
            phase: String::new(),
            next_step: String::new(),
            attempts: 0,
            merged_tasks_at_credit: None,
            next_at: None,
            last_attempt_at: None,
            delivery_error: None,
            exhausted_notified: false,
            delivery_error_notified: false,
            stuck_watch: None,
            recovery: None,
            provider_recovery_at: None,
            lead_failure_notice: None,
            provider_fallback: None,
            health: crossagents_core::Health::default(),
        });
        state.owner = run.planner;
        let merged = self.store.list_tasks(id).await?.iter()
            .filter(|task| task.status == TaskStatus::Merged).count();
        if state.merged_tasks_at_credit.is_some_and(|previous| merged > previous) {
            state.attempts = 0;
            state.next_at = None;
            state.exhausted_notified = false;
        }
        state.merged_tasks_at_credit = Some(state.merged_tasks_at_credit.unwrap_or(0).max(merged));
        let changed_phase = state.phase != step.phase;
        state.phase = step.phase.into();
        let mut next_step = step.next_step;
        let mut eligible = step.eligible;
        // The opt-in stuck watch. A phase that waits on a busy agent is not by itself a
        // reason to act (a compile may just be slow), and a busy agent's silence is not
        // evidence either: only the run's own durable facts going quiet for `stuck_minutes`
        // is. Even then the lead is only asked to decide; nothing is interrupted or replayed.
        //
        // Only a delivery pass judges that quiet stretch. Settling (`deliver` false) runs when
        // the lead goes Idle and sends nothing, so judging there would write the stuck advice
        // and re-arm `progress_at` with nothing delivered: the clock's next pass would then
        // see a fresh stretch and send the plain advice, and the "no progress for N minutes,
        // waiting for whom" would be lost. A settle therefore leaves the watch alone, with one
        // exception it owes: `state.phase` is written further up, so a phase change it records
        // here would be invisible to the next delivery pass (`changed_phase` false there) and
        // the quiet stretch of the old phase would be charged to the new one. Re-arming the
        // existing watch for the new phase keeps that stretch from leaking and swallows
        // nothing: it is the same reset a delivery pass makes on a phase change, not a stuck
        // judgement. A watch that does not exist yet is left for the delivery pass to create.
        let mut stuck = false;
        if let (Some(minutes), Some(waiting)) = (policy.stuck_minutes, step.waiting.as_ref()) {
            if deliver {
                let mut watch = match state.stuck_watch.clone() {
                    // A new phase starts its quiet stretch now: records written before it say
                    // nothing about what this wait is doing.
                    Some(watch) if !changed_phase => watch,
                    _ => StuckWatch {
                        progress_at: now,
                        event_seq: self.store.latest_event_seq().await?,
                    },
                };
                self.stuck_progress(&run, &mut watch, now).await?;
                let quiet = now - watch.progress_at;
                if quiet >= Duration::minutes(i64::from(minutes)) {
                    stuck = true;
                    eligible = true;
                    next_step = format!(
                        "No progress for {} minutes while waiting for {}: nothing in the run changed and none of its agents reported anything. Inspect it and decide yourself whether to interrupt it, reassign it or keep waiting. Nothing was interrupted or replayed.",
                        quiet.num_minutes().max(1),
                        waiting.what
                    );
                    // The threshold was the wait: start the next stretch here, so a later report
                    // needs another full `stuck_minutes` instead of one interval.
                    watch.progress_at = now;
                }
                state.stuck_watch = Some(watch);
            } else if changed_phase && let Some(watch) = state.stuck_watch.as_mut() {
                watch.progress_at = now;
                watch.event_seq = self.store.latest_event_seq().await?;
            }
        } else if deliver {
            // Not waiting on anyone: a quiet stretch from here says nothing. A settle leaves
            // the watch for the phase it saw; nothing reads it before the delivery pass that
            // clears it here.
            state.stuck_watch = None;
        }
        let mut tell_recovery_blocked = None;
        let mut use_bounded_recovery = false;
        let mut recovery_facts = None;
        let mut provider_retry = None;
        let mut discovery_recovery = None;
        if run.versioned()
            && matches!(step.phase, "recover" | "provider_recover" | "error")
            && let Some((fingerprint, changed_at, affected)) = self.stalled_recovery(&run).await?
        {
            use_bounded_recovery = true;
            recovery_facts = Some(affected.clone());
            if state
                .recovery
                .as_ref()
                .is_none_or(|watch| watch.fingerprint != fingerprint)
            {
                let legacy_checked = state.recovery.is_some();
                state.recovery = Some(RecoveryWatch {
                    fingerprint,
                    attempts: 0,
                    notified: false,
                    legacy_checked,
                });
            }
            if let Some(watch) = state.recovery.as_mut()
                && !watch.legacy_checked
            {
                let historical = self
                    .previous_recovery_wakes(&run, changed_at, state.attempts)
                    .await?;
                watch.attempts = watch.attempts.max(historical);
                watch.legacy_checked = true;
            }
            if let Some(watch) = state.recovery.as_mut()
                && watch.attempts >= UNCHANGED_RECOVERY_LIMIT
                && state.attempts < policy.max_attempts
            {
                eligible = false;
                state.phase = "recovery_blocked".into();
                next_step = format!(
                    "两次恢复唤醒后任务状态仍未变化。已停止重复唤醒，等待阻塞解决。\n{affected}"
                );
                if deliver && !watch.notified {
                    let topic = format!("continuation:{}", run.id);
                    let pending: Vec<_> = self
                        .store
                        .list_pending_notices(Some(run.planner))
                        .await?
                        .into_iter()
                        .filter(|row| row.topic == topic)
                        .map(|row| (row.topic, row.updated_at))
                        .collect();
                    self.store
                        .delete_pending_versions(run.planner, &pending)
                        .await?;
                    watch.notified = true;
                    tell_recovery_blocked = Some(next_step.clone());
                }
            }
        }
        // Inspect the lead as well as stalled workers. An idle lead or a fresh
        // driver process can hide a repeated turn failure from the status check.
        if state.phase != "recovery_blocked"
            && matches!(step.phase, "continue" | "recover" | "finalize" | "provider_recover" | "error")
        {
            let events = self.store.agent_events(
                run.planner,
                self.store.latest_event_seq().await?.saturating_sub(8_000),
                8_000,
            ).await?;
            let start = events.partition_point(|event| event.ts < run.created_at);
            if let Some(failure) = recent_lead_failure(&events[start..]) {
                // Agent directory listing is unnecessary for a scoped lead: task state is
                // already available. Permit one corrected turn using the existing grant.
                let discovery_failure = failure.detail.starts_with(
                    "permission check failed for mcp \"crossagents/agent_list\"",
                ) && self.store.get_agent(&AgentKey::Id(run.planner)).await.is_ok_and(|agent| {
                    agent.plugin == "antigravity"
                        && agent.driver.as_deref().is_none_or(|driver| driver == "antigravity-native")
                });
                let notice = if discovery_failure {
                    format!("scoped-discovery:{}", failure.first_turn)
                } else {
                    failure.first_turn.to_string()
                };
                if discovery_failure
                    && failure.repeats >= UNCHANGED_RECOVERY_LIMIT
                    && state.lead_failure_notice.as_deref() != Some(&notice)
                {
                    eligible = true;
                    state.phase = step.phase.into();
                    discovery_recovery = Some(notice);
                    tell_recovery_blocked = None;
                    next_step = "The lead tried an unavailable agent directory lookup. Continue with the supplied task facts and use the granted task_list or task_get if needed. Do not make a discovery call, retry a refused command, or alter permissions. Advance independently verifiable work only.".into();
                } else if failure.repeats >= UNCHANGED_RECOVERY_LIMIT
                    && failure.key == PROVIDER_STREAM_FAILURE
                    && temporary_provider_failure(&events[start..], None)
                {
                    let cooldown = provider_cooldown(failure.repeats)
                        .max(Duration::seconds(i64::from(policy.interval_seconds)));
                    let since = state.last_attempt_at.unwrap_or(failure.latest_at).max(failure.latest_at);
                    let changed = verified_local_checkpoint(&run).and_then(|path| {
                        std::fs::metadata(path).ok()?.modified().ok().map(DateTime::<Utc>::from)
                    });
                    let retry_at = provider_retry_at(since, cooldown, changed, now);
                    provider_retry = Some(retry_at);
                    eligible = true;
                    state.phase = "provider_cooldown".into();
                    next_step = if retry_at < since + cooldown {
                        "New local validation evidence was saved after the model connection failed. Resume once from that checkpoint and confirm the outcome. Do not replay commands or retry denied operations.".into()
                    } else {
                        format!(
                            "The model connection failed {} times. After a {} minute cooldown, resume from the last durable checkpoint and check whether the provider is responsive. Do not replay previous commands or retry denied operations.",
                            failure.repeats, cooldown.num_minutes(),
                        )
                    };
                } else if failure.repeats >= UNCHANGED_RECOVERY_LIMIT {
                    eligible = false;
                    state.phase = "recovery_blocked".into();
                    next_step = format!(
                        "总工连续 {} 轮遇到同一原因，已停止重复唤醒。确认故障或权限状态发生变化后再继续。\n{}",
                        failure.repeats, failure.detail,
                    );
                    if deliver && state.lead_failure_notice.as_deref() != Some(&notice) {
                        state.lead_failure_notice = Some(notice);
                        tell_recovery_blocked = Some(next_step.clone());
                    }
                } else if eligible {
                    next_step.push_str(&format!(
                        "\nPrevious lead failure: {}. First check the cause in the transcript or tool result, identify what changed, and verify a targeted repair. Do not repeat a failed call or a denied operation. When the failure is external, record the blocker and continue only independent work.",
                        failure.detail,
                    ));
                }
            }
        }
        let mut transport_fallback = None;
        if deliver && run.status == RunStatus::Running && run.versioned()
            && matches!(step.phase, "recover" | "provider_recover" | "error")
            && (state.phase == "recovery_blocked" || provider_retry.is_some())
            && state.attempts < policy.max_attempts
            && self.host.continuation_dispatch_allowed(run.planner).await.is_ok()
        {
            let events = self.store.agent_events(
                run.planner,
                self.store.latest_event_seq().await?.saturating_sub(8_000),
                8_000,
            ).await?;
            let start = events.partition_point(|event| event.ts < run.created_at);
            let events = &events[start..];
            if temporary_provider_failure(events, None)
                && let Some(failure) = recent_lead_failure(events)
                && let Some(fallback) = self.lead_transport_fallback(&run, &state, &failure).await?
            {
                // New recovery route, not a replay of the timed-out turn.
                transport_fallback = Some(fallback);
                tell_recovery_blocked = None;
                eligible = true;
                provider_retry = Some(now);
                state.phase = "provider_fallback".into();
                next_step = "总工连续连接超时。临时使用已登记的备用模型，从保存的任务状态接续一次。".into();
            }
        }
        state.next_step = next_step;
        let interval = Duration::seconds(i64::from(policy.interval_seconds.max(
            if step.phase == "provider_recover" { 300 } else { 1 },
        )));
        let mut wake = false;
        let mut tell_exhausted = false;
        if !eligible {
            state.next_at = None;
        } else if state.attempts >= policy.max_attempts {
            state.phase = "exhausted".into();
            state.next_step = format!(
                "Automatic continuation limit ({}) reached without a new merged milestone. Review the run or continue when new task evidence is available. {}",
                policy.max_attempts, state.next_step
            );
            state.next_at = None;
            // The flag lives in the run record, not in this process: the clock revisits this
            // branch every few seconds and a restarted hub reads the flag straight back from
            // the run JSON, so one exhaustion tells the person exactly once either way. Stuck
            // reports share the same cap and the same flag, so they cannot notify twice.
            if !state.exhausted_notified {
                state.exhausted_notified = true;
                tell_exhausted = true;
            }
        } else {
            // Below the cap again: a person re-armed the run (attempts back to zero, or a
            // larger cap). The next exhaustion is a new fact and will notify again; the
            // policy-update path only has to reset `attempts` and this flag clears itself.
            state.exhausted_notified = false;
            if let Some(retry_at) = provider_retry {
                state.next_at = Some(retry_at);
            } else if changed_phase || state.next_at.is_none() {
                state.next_at = Some(now + interval);
            }
            if stuck {
                // The quiet stretch already lasted the threshold; do not add an interval.
                state.next_at = Some(now);
            }
            if deliver && state.next_at.is_some_and(|at| at <= now) {
                state.attempts += 1;
                if use_bounded_recovery && let Some(watch) = state.recovery.as_mut() {
                    watch.attempts += 1;
                }
                state.last_attempt_at = Some(now);
                if let Some(fallback) = &transport_fallback {
                    state.provider_fallback = Some(fallback.clone());
                }
                if let Some(notice) = &discovery_recovery {
                    state.lead_failure_notice = Some(notice.clone());
                }
                if step.phase == "provider_recover" || provider_retry.is_some() {
                    state.provider_recovery_at = Some(now);
                }
                state.next_at = Some(now + interval);
                state.delivery_error = None;
                wake = true;
            }
        }
        if old.as_ref() != Some(&state) {
            run.continuation = Some(Box::new(state.clone()));
            self.store.update_run(&run).await?;
            self.publish(EventKind::RunContinuationChanged {
                id,
                state: state.clone(),
            })
            .await;
        }
        if let Some(reason) = tell_recovery_blocked {
            tracing::warn!(run = %id, %reason, "automatic recovery blocked");
        }
        if tell_exhausted {
            // The flag was persisted just above, before anything reached the person: a crash
            // here can lose this notice, but no restart can produce a second one.
            let text = format!(
                "「{}」（run {}）的自动持续推进已用尽：已唤醒 {}/{} 次，停在 {} 阶段，需要人处理。\n下一步建议：{}",
                run.title,
                run.id,
                state.attempts,
                policy.max_attempts,
                observed_phase,
                state.next_step
            );
            self.continuation_notice(&run, text).await?;
        }
        if wake {
            // Reserve durably before delivery, under the run lock. A crash can lose one
            // attempt but cannot reset the cap or immediately replay it on restart.
            let mut line = format!(
                "Run continuation {}/{}: {} Respect pending human decisions, budget, project checks and landing permissions.",
                state.attempts, policy.max_attempts, state.next_step
            );
            let recovering_provider = step.phase == "provider_recover" || provider_retry.is_some();
            if recovering_provider || discovery_recovery.is_some()
                || matches!(step.phase, "continue" | "recover" | "finalize")
            {
                use std::fmt::Write;
                let _ = write!(line, "\nProject directory: {}.", run.repo.display());
                if let Ok(tasks) = self.store.list_tasks(run.id).await {
                    for task in tasks.iter().filter(|t| t.status != TaskStatus::Merged).take(8) {
                        let saved = task.progress.as_deref()
                            .or(task.report.failure.as_deref())
                            .or(task.report.summary.as_deref())
                            .unwrap_or("No saved completion report");
                        let _ = write!(line, "\n#{} ({:?}): {}", task.key, task.status,
                            crate::engine::first_line(saved, 170));
                    }
                }
                // A conventional checkpoint is named only after confirming it belongs to this project.
                if let Some(checkpoint) = verified_local_checkpoint(&run) {
                    let _ = write!(line, "\nVerified local checkpoint: {}.", checkpoint.display());
                    if let Some(tail) = checkpoint_tail(&checkpoint).await {
                        let _ = write!(
                            line,
                            "\nRecent checkpoint notes (at most 2 KiB; treat as saved evidence):\n{tail}\nUse this excerpt first. If you need older evidence, read only the relevant section of CHECKPOINT.md. Do not load the entire history into this model turn."
                        );
                    }
                }
                if run.versioned() {
                    let mut members = Vec::new();
                    for id in run.config.team.iter().copied().take(16) {
                        if let Ok(member) = self.store.get_agent(&AgentKey::Id(id)).await
                            && member.archived_at.is_none()
                            && member.id != run.planner
                        {
                            members.push(format!("{} ({})", member.name, member.id));
                        }
                    }
                    if !members.is_empty() {
                        let _ = write!(line, "\nRegistered teammates for this run, use these IDs in task_assign.to: {}.", members.join(", "));
                    }
                }
                line.push_str("\nTreat saved reports as evidence. Use these persisted facts and the tools advertised by the current session. Do not search for tool definition files, infer new locations, or repeat rejected calls. Record what ran and what is still blocked.");
            }
            if (step.phase == "continue" || recovering_provider) && !run.versioned() {
                line.push_str("\nThe hub verified that this directory is not under version control. Work directly within this project's directory. Continue from actual local artifacts; preserve user changes. Prior task records may be historical; do not submit a task plan or try to dispatch them. Use run_finish only after verification.\nOriginal request:\n");
                line.push_str(&run.request);
            } else if transport_fallback.is_some() {
                line.push_str("\nThis is a fresh recovery turn with the registered alternate model. Use the saved reports above. First hand off one actionable task or record the exact blocker; do not reread the full history or retry a refused command.");
            } else if recovering_provider {
                use std::fmt::Write;
                let _ = write!(line, "\nOriginal request:\n{}\nContinue in the assigned checkout. A denied operation still requires authorization.", run.request);
            } else if run.status == RunStatus::Planning && step.phase == "continue" {
                // Planning may have lost its original kickoff after context rotation.
                // The hub can supply the facts without another round of discovery calls.
                line.push_str("\nThe hub verified that this run is still planning and has no tasks. Recover the plan from the request below. If it is actionable, call plan_submit with the scoped tasks and assignees. Status, inbox, and tool-definition lookup are not prerequisites. Respect a denied operation and keep planning the independent work. If plan submission itself is unavailable, state the exact blocker. Only ask the person for an indispensable decision or permission.");
                let mut team = Vec::new();
                for id in &run.config.team {
                    if let Ok(agent) = self.store.get_agent(&AgentKey::Id(*id)).await
                        && agent.id != run.planner
                        && agent.archived_at.is_none()
                    {
                        team.push(format!("{} ({:?})", agent.name, agent.role));
                    }
                }
                if !team.is_empty() {
                    line.push_str("\nAvailable teammates: ");
                    line.push_str(&team.join(", "));
                }
                if !run.request.is_empty() {
                    use std::fmt::Write;
                    let _ = write!(line, "\nOriginal request:\n{}", run.request);
                }
            } else {
                line.push_str(" Re-read the relevant run state and checkout. Take a concrete next step or complete the run only when verified.");
            }
            if let Some(affected) = recovery_facts {
                use std::fmt::Write;
                let _ = write!(
                    line,
                    "\nRecorded blockers:\n{affected}\nUse task_get to verify the affected task. Use task_rework with task_id and notes only when you have a viable recovery. Give the assignee the error evidence, the suspected cause, the changed approach, and the verification to perform. Check that the current worker and execution route are available before rework. If a task is superseded by another active run, keep it as a reference and leave execution to that run. Have the worker report a blocker instead of repeating an unchanged attempt. If a required permission was denied, leave that operation stopped and advance any independent work."
                );
            }
            if matches!(step.phase, "continue" | "recover" | "provider_recover" | "finalize")
                && run.config.policy.human_ask == crossagents_core::AskPolicy::Allow
            {
                line.push_str(
                    "\nIf independent work is complete and a real user decision can advance the run, use human_ask once with the required text field. If a required command has no verified approval channel, record it as blocked and do not ask the user to approve it by replying. Start text with a plain-language question the person can act on. Then explain what is blocked, where to act, and what will continue after they act. Put tool names, commands and logs in detail. If a reply cannot authorize the operation, state that plainly and name the real approval step only if verified. End your turn after asking so the hub can wait for the answer. Otherwise continue the work yourself.",
                );
            }

            let delivered = if let Some(fallback) = transport_fallback {
                async {
                    self.host.continuation_dispatch_allowed(run.planner).await?;
                    let lead = self.store.get_agent(&AgentKey::Id(run.planner)).await?;
                    if lead.model.as_deref() != Some(&fallback.primary) {
                        return Err(CoreError::Conflict("the lead model changed while reserving recovery".into()));
                    }
                    if !self.host.renew_session(run.planner).await? {
                        return Err(CoreError::Conflict("the recovery session could not be renewed".into()));
                    }
                    self.host.set_model(run.planner, Some(fallback.alternate)).await?;
                    self.host.send(run.planner, MessageSource::run(run.id), line).await
                }.await
            } else if use_bounded_recovery || recovering_provider || discovery_recovery.is_some() {
                // A failed model turn must not replay the same reserved recovery message.
                self.host
                    .send(run.planner, MessageSource::run(run.id), line)
                    .await
            } else {
                self.host
                    .notify(
                        run.planner,
                        &format!("continuation:{}", run.id),
                        &line,
                        crossagents_core::NoticeClass::Decide,
                    )
                    .await
            };
            // Every wakeup is a run of this automation; the breaker counts it whichever way it
            // went (multica §2.14). A pause is written with the rest before anyone is told.
            let previous_health = state.health.clone();
            let tripped = crate::breaker::record(&mut state.health, now, delivered.is_ok());
            if let Some(reason) = &tripped {
                state.phase = "automation_paused".into();
                state.next_step = format!(
                    "Automatic continuation paused itself ({reason}). A person must inspect the run and re-enable it."
                );
                state.next_at = None;
            }
            if let Err(error) = delivered {
                let reason = error.to_string();
                state.delivery_error = Some(reason.clone());
                let first_failure = !state.delivery_error_notified;
                state.delivery_error_notified = true;
                run.continuation = Some(Box::new(state.clone()));
                self.store.update_run(&run).await?;
                self.publish(EventKind::RunContinuationChanged {
                    id,
                    state: state.clone(),
                })
                .await;
                if first_failure {
                    tracing::warn!(run = %id, %reason, "continuation delivery failed; scheduler will evaluate retry");
                }
                if let Some(reason) = tripped {
                    self.report_automation_pause(&run, &state, &reason).await?;
                }
                return Err(error);
            }
            let mut changed = state.health != previous_health;
            if state.delivery_error_notified {
                // Delivered after all: a later failure is a new fact worth one notice.
                state.delivery_error_notified = false;
                changed = true;
            }
            if changed {
                run.continuation = Some(Box::new(state.clone()));
                self.store.update_run(&run).await?;
                self.publish(EventKind::RunContinuationChanged {
                    id,
                    state: state.clone(),
                })
                .await;
            }
            if let Some(reason) = tripped {
                self.report_automation_pause(&run, &state, &reason).await?;
            }
        }
        Ok(())
    }

    /// Tells the person that the continuation stopped itself, once: the pause is already in the
    /// run record, so a restart reads it back and never reports it again.
    async fn report_automation_pause(
        &self,
        run: &Run,
        state: &ContinuationState,
        reason: &str,
    ) -> Result<(), CoreError> {
        let (total, failed) = crate::breaker::counts(&state.health, Utc::now());
        let text = format!(
            "「{}」（run {}）的自动持续推进已自动暂停：{}（7 天内运行 {} 次，失败 {} 次，失败率 ≥90%）。多半是主导长期叫不醒或账号额度用尽；检查账号和 run 状态后，用人用 run policy 的 reset_attempts 重新放开，才会继续。",
            run.title, run.id, reason, total, failed
        );
        self.continuation_notice(run, text).await
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;

    #[test]
    fn repeated_lead_errors_are_grouped_but_a_durable_decision_breaks_the_streak() {
        use crossagents_core::{MessageId, StopReason};
        let lead = AgentId::new();
        let mut events = Vec::new();
        let at = Utc::now();
        let mut push = |kind| events.push(crossagents_core::Event {
            seq: 0,
            id: uuid::Uuid::nil(),
            ts: at,
            schema: 1,
            kind,
        });
        let first = MessageId::new();
        for (message_id, message, progress) in [
            (first, "API error: stream idle timeout (10m0s)", false),
            (MessageId::new(), "failed to start conversation: backend returned an empty conversation id", false),
            (MessageId::new(), "API error: stream idle timeout (10m0s)", true),
        ] {
            push(EventKind::AgentTurnStarted { id: lead, message_id });
            push(EventKind::AgentError { id: lead, message: message.into() });
            if progress {
                push(EventKind::AgentToolUsed { id: lead, tool: "task_rework".into(), ok: true });
            }
            push(EventKind::AgentTurnCompleted { id: lead, message_id, stop_reason: StopReason::Error });
        }
        let streak = recent_lead_failure(&events[..6]).unwrap();
        assert_eq!(streak.repeats, 2);
        assert_eq!(streak.first_turn, first);
        assert!(recent_lead_failure(&events).is_none());
    }

    #[tokio::test]
    async fn recovery_checkpoint_reads_only_recent_notes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CHECKPOINT.md");
        let latest = "本轮已写入日志，可以从这里继续。\n";
        tokio::fs::write(&path, format!("{}\n{latest}", "旧记录".repeat(700)))
            .await.unwrap();
        assert_eq!(checkpoint_tail(&path).await.as_deref(), Some(latest.trim()));
        tokio::fs::write(&path, latest).await.unwrap();
        assert_eq!(checkpoint_tail(&path).await.as_deref(), Some(latest.trim()));
    }

    #[test]
    fn a_new_checkpoint_rearms_one_provider_probe() {
        let last = Utc::now();
        let elapsed = |minutes| last + Duration::minutes(minutes);
        let backoff = provider_cooldown(6);
        let changed = Some(elapsed(2));
        assert_eq!(provider_retry_at(last, backoff, changed, elapsed(3)), elapsed(5));
        assert_eq!(provider_retry_at(last, backoff, changed, elapsed(8)), elapsed(5));
        assert_eq!(provider_retry_at(elapsed(8), backoff, changed, elapsed(9)), elapsed(128));
        assert_eq!(provider_retry_at(last, backoff, Some(elapsed(9)), elapsed(8)), elapsed(120));
    }

    #[test]
    fn provider_cooldown_increases_but_has_a_ceiling() {
        assert_eq!(provider_cooldown(2), Duration::minutes(15));
        assert_eq!(provider_cooldown(3), Duration::minutes(30));
        assert_eq!(provider_cooldown(4), Duration::minutes(60));
        assert_eq!(provider_cooldown(30), Duration::minutes(120));
    }

    #[test]
    fn provider_recovery_needs_a_known_failure_and_a_new_success_to_rearm() {
        let now = Utc::now();
        let lead = AgentId::new();
        let event = |seconds, kind| crossagents_core::Event {
            seq: seconds as u64,
            id: uuid::Uuid::nil(),
            ts: now + Duration::seconds(seconds),
            schema: 1,
            kind,
        };
        let error = |message: &str| EventKind::AgentError { id: lead, message: message.into() };
        let completed = EventKind::AgentTurnCompleted {
            id: lead,
            message_id: crossagents_core::MessageId::new(),
            stop_reason: crossagents_core::StopReason::EndTurn,
        };
        let timeout = "API error (attempt 1): request failed: stream idle timeout (10m0s)";
        let circuit = "Circuit breaker: agent hit consecutive error limit (3); stopping turn loop to protect quota";
        let original = vec![event(1, error(timeout)), event(2, error(circuit))];
        assert!(temporary_provider_failure(&original, None));
        assert!(!temporary_provider_failure(&original, Some(now + Duration::seconds(3))));
        let second = vec![event(4, error(timeout)), event(5, error(circuit))];
        assert!(!temporary_provider_failure(&second, Some(now + Duration::seconds(3))));
        let completed_then_new_failure = vec![
            event(4, completed),
            event(5, error(timeout)),
            event(6, error(circuit)),
        ];
        assert!(temporary_provider_failure(&completed_then_new_failure, Some(now + Duration::seconds(3))));
        let eof = "API error (attempt 1): request failed: Post provider endpoint: EOF";
        assert!(temporary_provider_failure(&[event(1, error(eof))], None));
        let forbidden = "API error: authentication forbidden after stream idle timeout";
        assert!(!temporary_provider_failure(&[event(1, error(forbidden))], None));
        let mixed = [event(1, error(timeout)), event(2, error("quota exceeded"))];
        assert!(!temporary_provider_failure(&mixed, None));
        let denied = EventKind::AgentToolDetail {
            id: lead,
            tool_call_id: "blocked".into(),
            detail: crossagents_core::ToolDetail {
                error: Some("permission check failed for unsandboxed \"build\": user denied permission".into()),
                ..Default::default()
            },
        };
        assert!(!temporary_provider_failure(&[event(1, denied), event(2, error(timeout))], None));
    }
}
