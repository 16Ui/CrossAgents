//! User-configured project-wide Codex reserve. A latched stop survives Hub restarts.
use crate::{accounts::Accounts, runtime::Runtime};
use chrono::{DateTime, Utc};
use crossagents_core::{
    AccountStatus, Agent, AgentKey, CoreError, EventBus, EventKind, Note, NoteScope, RunStatus,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone, Deserialize, Serialize)]
pub struct Rule {
    pub project: PathBuf,
    #[serde(default)]
    pub related_projects: Vec<PathBuf>,
    pub account: String,
    pub minimum_remaining_percent: f64,
    pub latched: bool,
    #[serde(default)]
    pub waiting_for_quota: bool,
    pub reason: Option<String>,
    pub checked_at: DateTime<Utc>,
    pub remaining_percent: Option<f64>,
}
fn path(home: &Path) -> PathBuf {
    home.join("project-quota-reserve.json")
}
fn normalized(p: &Path) -> String {
    p.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}
fn belongs(p: &Path, root: &Path) -> bool {
    let (p, root) = (normalized(p), normalized(root));
    p == root || p.starts_with(&(root + "/"))
}
impl Rule {
    fn covers(&self, path: &Path) -> bool {
        belongs(path, &self.project) || self.related_projects.iter().any(|root| belongs(path, root))
    }
}
fn load(home: &Path) -> Result<Option<Rule>, CoreError> {
    match std::fs::read(path(home)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| CoreError::Conflict(format!("Invalid quota reserve policy: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::Conflict(format!(
            "Cannot read quota reserve policy: {e}"
        ))),
    }
}
fn save(home: &Path, rule: &Rule) -> Result<(), CoreError> {
    let bytes = serde_json::to_vec_pretty(rule).map_err(|e| CoreError::Conflict(e.to_string()))?;
    let temp = home.join("project-quota-reserve.next");
    std::fs::write(&temp, bytes)
        .and_then(|_| std::fs::rename(&temp, path(home)))
        .map_err(|e| CoreError::Conflict(format!("Cannot persist quota stop: {e}")))
}
fn rule_gate(rule: &Rule, now: DateTime<Utc>) -> Result<(), CoreError> {
    if rule.latched || rule.waiting_for_quota {
        return Err(CoreError::Conflict(rule.reason.clone().unwrap_or_else(
            || "Codex reserve stop is latched; explicit user resume required".into(),
        )));
    }
    if !rule.minimum_remaining_percent.is_finite()
        || !(0.0..=100.0).contains(&rule.minimum_remaining_percent)
        || (now - rule.checked_at).num_seconds() > 15
        || rule.checked_at > now + chrono::Duration::seconds(5)
        || !rule
            .remaining_percent
            .is_some_and(|v| v.is_finite() && v > rule.minimum_remaining_percent && v <= 100.0)
    {
        return Err(CoreError::Conflict(
            "Codex reserve cannot be confirmed above its minimum; work is blocked".into(),
        ));
    }
    Ok(())
}
pub fn check(home: Option<&Path>, agent: &Agent) -> Result<(), CoreError> {
    let Some(home) = home else {
        return Ok(());
    };
    let Some(rule) = load(home)? else {
        return Ok(());
    };
    if rule.covers(&agent.home) || rule.covers(&agent.cwd) {
        rule_gate(&rule, Utc::now())?;
    }
    Ok(())
}
fn remaining(status: &AccountStatus, now: DateTime<Utc>) -> Result<f64, String> {
    if status.source != Some(crossagents_core::DataSource::Live)
        || !status.logged_in
        || status.as_of.is_none_or(|at| {
            (now - at).num_seconds() > 180 || at > now + chrono::Duration::seconds(5)
        })
    {
        return Err("Codex额度数据不可用或已过期，按严格规则停止".into());
    }
    if status.limit_reached {
        return Ok(0.0);
    }
    let mut lowest: Option<f64> = None;
    for w in status
        .rate_limits
        .iter()
        .chain(status.limits.iter().flat_map(|g| g.windows.iter()))
    {
        if w.resets_at.is_some_and(|at| at <= now) {
            continue;
        }
        if !w.used_percent.is_finite() || !(0.0..=100.0).contains(&w.used_percent) {
            return Err("Codex额度数值无效，按严格规则停止".into());
        }
        let value = 100.0 - w.used_percent;
        lowest = Some(lowest.map_or(value, |old| old.min(value)));
    }
    lowest.ok_or_else(|| "没有可确认的有效Codex额度窗口，按严格规则停止".into())
}
const QUOTA_WAIT: &str = "Gemini 周额度查询失败，暂停并继续轮询";

fn gemini_weekly_remaining(status: &AccountStatus, now: DateTime<Utc>) -> Result<f64, String> {
    if status.source != Some(crossagents_core::DataSource::Live)
        || !status.logged_in
        || status.as_of.is_none_or(|at| {
            at < now - chrono::Duration::seconds(90) || at > now + chrono::Duration::seconds(5)
        })
    {
        return Err(QUOTA_WAIT.into());
    }
    // The provider exposes separate buckets for Gemini, Claude/GPT, and Argon.
    let grouped = status
        .limits
        .iter()
        .filter(|group| {
            group
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case("Gemini Models"))
        })
        .flat_map(|group| group.windows.iter())
        .filter(|window| {
            window.name.to_ascii_lowercase().contains("week") || window.name.contains("每周")
        });
    let labelled = status
        .rate_limits
        .iter()
        .filter(|window| window.name == "Gemini 每周");
    let mut remaining: Option<f64> = None;
    for window in grouped.chain(labelled) {
        if window.resets_at.is_some_and(|at| at <= now) {
            continue;
        }
        if !window.used_percent.is_finite() || !(0.0..=100.0).contains(&window.used_percent) {
            return Err(QUOTA_WAIT.into());
        }
        let value = 100.0 - window.used_percent;
        remaining = Some(remaining.map_or(value, |old| old.min(value)));
    }
    remaining.ok_or_else(|| QUOTA_WAIT.into())
}

fn resumable_status(note: Option<&str>) -> Option<RunStatus> {
    match note?.strip_prefix(QUOTA_WAIT)?.trim_start_matches(" · ") {
        "planning" => Some(RunStatus::Planning),
        "awaiting_approval" => Some(RunStatus::AwaitingApproval),
        "running" => Some(RunStatus::Running),
        _ => None,
    }
}

async fn enforce(
    home: &Path,
    bus: &Arc<EventBus>,
    runtime: &Arc<Runtime>,
    accounts: &Arc<Accounts>,
) -> Result<(), CoreError> {
    let Some(mut rule) = load(home)? else {
        return Ok(());
    };
    if !rule.latched {
        let now = Utc::now();
        let result = match tokio::time::timeout(
            Duration::from_secs(8),
            accounts.status(&rule.account),
        )
        .await
        {
            Ok(Ok(status)) if rule.account == "agy" => gemini_weekly_remaining(&status, Utc::now()),
            Ok(Ok(status)) => remaining(&status, Utc::now()),
            _ if rule.account == "agy" => Err(QUOTA_WAIT.into()),
            _ => Err("无法确认Codex剩余额度，按严格规则停止".into()),
        };
        rule.checked_at = now;
        match result {
            Ok(value) => {
                rule.remaining_percent = Some(value);
                rule.waiting_for_quota = false;
                rule.reason = None;
                if !rule.minimum_remaining_percent.is_finite()
                    || value <= rule.minimum_remaining_percent
                {
                    rule.latched = true;
                    rule.reason = Some(if rule.account == "agy" {
                        format!("Gemini 周额度已用 {:.1}%，EF 已停止", 100.0 - value)
                    } else {
                        format!(
                            "Codex剩余{value:.1}% ≤ 保留线{:.1}%，本项目全团队已停止；必须由用户明确恢复",
                            rule.minimum_remaining_percent
                        )
                    });
                }
            }
            Err(why) => {
                rule.waiting_for_quota = rule.account == "agy";
                rule.latched = !rule.waiting_for_quota;
                rule.remaining_percent = None;
                rule.reason = Some(why);
            }
        }
        // Persist the latch BEFORE stopping or publishing anything. All runtime gates
        // see it immediately, including other providers and new execution sessions.
        save(home, &rule)?;
    }
    let store = bus.store();
    if !rule.latched && !rule.waiting_for_quota {
        if rule.account == "agy" {
            for mut run in store.list_runs().await? {
                if !rule.covers(&run.repo) || run.status != RunStatus::Paused {
                    continue;
                }
                let Some(resumed) = resumable_status(run.note.as_deref()) else {
                    continue;
                };
                run.status = resumed;
                run.note = None;
                store.update_run(&run).await?;
                bus.publish(EventKind::RunStatusChanged {
                    id: run.id,
                    from: RunStatus::Paused,
                    to: resumed,
                    note: None,
                })
                .await?;
                // Managed runs go back through continuation, which also checks
                // unresolved questions and the lead's provider circuit breaker.
                if resumed != RunStatus::AwaitingApproval
                    && run.config.policy.continuation.is_none()
                    && store.get_agent(&AgentKey::Id(run.planner)).await
                        .is_ok_and(|lead| lead.status != crossagents_core::AgentStatus::Error)
                {
                    let _ = runtime.send(&AgentKey::Id(run.planner), "Gemini 周额度查询已恢复，且低于停止线。先检查原计划、任务归属和检查点，再接续被暂停的工作。若计划尚未提交，先完成计划。".into(), crossagents_core::MessageSource::run(run.id)).await;
                }
            }
        }
        return Ok(());
    }
    for mut run in store.list_runs().await? {
        if rule.covers(&run.repo)
            && matches!(
                run.status,
                RunStatus::Planning | RunStatus::Running | RunStatus::AwaitingApproval
            )
        {
            let checkpoint = serde_json::json!({"run_id":run.id,"work_id":run.work_id,"repo":run.repo,"reason":rule.reason,"at":Utc::now(),"tasks":store.list_tasks(run.id).await?});
            let bytes = serde_json::to_vec_pretty(&checkpoint)
                .map_err(|e| CoreError::Conflict(e.to_string()))?;
            crate::paths::write_atomic(
                &home.join(format!("quota-stop-checkpoint-{}.json", run.id)),
                &bytes,
            )
            .map_err(|e| CoreError::Conflict(format!("Cannot save quota checkpoint: {e}")))?;
            let from = run.status;
            run.status = RunStatus::Paused;
            run.note = if rule.waiting_for_quota {
                let previous = match from {
                    RunStatus::Planning => "planning",
                    RunStatus::AwaitingApproval => "awaiting_approval",
                    _ => "running",
                };
                Some(format!("{QUOTA_WAIT} · {previous}"))
            } else {
                rule.reason.clone()
            };
            if rule.latched {
                run.config.policy.continuation = None;
            }
            store.update_run(&run).await?;
            store
                .put_note(&Note {
                    scope: NoteScope::Run,
                    run_id: Some(run.id),
                    key: "quota-reserve/stop".into(),
                    text: format!(
                        "{}\n保留代码、现有工件与任务状态；不自动续接，不切快模型继续。",
                        rule.reason.as_deref().unwrap_or("Quota reserve stopped")
                    ),
                    author: run.planner,
                    updated_at: Utc::now(),
                })
                .await?;
            bus.publish(EventKind::RunStatusChanged {
                id: run.id,
                from,
                to: RunStatus::Paused,
                note: run.note.clone(),
            })
            .await?;
        }
    }
    let mut stops = tokio::task::JoinSet::new();
    for agent in store.list_agents().await? {
        // The error breaker already stopped the turn loop. Keeping its status
        // carries the reason and its recovery budget across an account pause.
        if !matches!(agent.status, crossagents_core::AgentStatus::Stopped | crossagents_core::AgentStatus::Error)
            && (rule.covers(&agent.home) || rule.covers(&agent.cwd))
        {
            let runtime = runtime.clone();
            stops.spawn(async move { runtime.stop(&AgentKey::Id(agent.id)).await });
        }
    }
    while let Some(result) = stops.join_next().await {
        if let Ok(Err(err)) = result {
            tracing::warn!(error=%err,"quota stop of session failed; latch still blocks dispatch");
        }
    }
    Ok(())
}
pub async fn start(
    home: PathBuf,
    bus: Arc<EventBus>,
    runtime: Arc<Runtime>,
    accounts: Arc<Accounts>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    if let Err(err) = enforce(&home, &bus, &runtime, &accounts).await {
        tracing::error!(error=%err,"quota guard initialization failed; runtime gates remain fail closed");
    }
    tokio::spawn(async move {
        let mut events = bus.subscribe();
        let mut timer = tokio::time::interval(Duration::from_secs(3));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = timer.tick() => {},
                event = events.recv() => {
                    match event { Ok(e) if matches!(e.kind,EventKind::AgentRateLimits{..}|EventKind::AccountQuotaUpdated{..}) => {},
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        _ => continue,
                    }
                }
            }
            if let Err(err) = enforce(&home, &bus, &runtime, &accounts).await {
                tracing::error!(error=%err,"quota enforcement failed; stale heartbeat blocks new work");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserve_boundary_is_inclusive_and_latched_stop_never_resumes() {
        let now = Utc::now();
        let mut r = Rule {
            project: PathBuf::from("D:/games/Emberframe-Engine"),
            related_projects: vec![PathBuf::from("D:/games/Emberframe-UE-GI")],
            account: "codex".into(),
            minimum_remaining_percent: 50.0,
            latched: false,
            waiting_for_quota: false,
            reason: None,
            checked_at: now,
            remaining_percent: Some(50.1),
        };
        assert!(rule_gate(&r, now).is_ok());
        assert!(r.covers(Path::new("D:/games/Emberframe-UE-GI/Plugins")));
        assert!(!r.covers(Path::new("D:/games/Emberframe-UE-GI-Other")));
        for value in [50.0, 49.9, 0.0, f64::NAN] {
            r.remaining_percent = Some(value);
            assert!(rule_gate(&r, now).is_err());
        }
        r.remaining_percent = Some(82.0);
        r.latched = true;
        assert!(rule_gate(&r, now).is_err());
        r.latched = false;
        assert!(rule_gate(&r, now + chrono::Duration::seconds(16)).is_err());
        assert!(belongs(
            Path::new("d:\\games\\Emberframe-Engine\\sub"),
            &r.project
        ));
        assert!(!belongs(
            Path::new("D:/games/Emberframe-Engine-Other"),
            &r.project
        ));
    }
    #[test]
    fn durable_gate_denies_all_models_and_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let r = Rule {
            project: PathBuf::from("D:/games/Emberframe-Engine"),
            related_projects: vec![PathBuf::from("D:/games/Emberframe-UE-GI")],
            account: "codex".into(),
            minimum_remaining_percent: 50.0,
            latched: true,
            waiting_for_quota: false,
            reason: Some("test stop".into()),
            checked_at: now,
            remaining_percent: Some(50.0),
        };
        save(dir.path(), &r).unwrap();
        assert!(rule_gate(&load(dir.path()).unwrap().unwrap(), now).is_err());
        assert_eq!(
            load(dir.path()).unwrap().unwrap().remaining_percent,
            Some(50.0)
        );
        let mut fast = Agent {
            node: None,
            id: crossagents_core::AgentId::new(),
            name_source: crossagents_core::NameSource::Chosen,
            topic: None,
            archived_at: None,
            home: r.project.clone(),
            name: "fast".into(),
            plugin: "tokenworks".into(),
            driver: None,
            model: Some("glm-5.3-flash".into()),
            role: crossagents_core::Role::Worker,
            cwd: r.project.clone(),
            config: serde_json::json!({"account":"tokenworks"}),
            status: crossagents_core::AgentStatus::Stopped,
            created_at: now,
            profile: Default::default(),
        };
        assert!(
            check(Some(dir.path()), &fast).is_err(),
            "fast models cannot bypass the Codex reserve"
        );
        fast.home = PathBuf::from("D:/other");
        fast.cwd = fast.home.clone();
        assert!(
            check(Some(dir.path()), &fast).is_ok(),
            "unrelated projects are not blocked"
        );
    }
    #[test]
    fn uses_lowest_active_window_and_refuses_unknown_or_stale_data() {
        let now = Utc::now();
        let mut status:AccountStatus=serde_json::from_value(serde_json::json!({
            "logged_in":true,"auth_mode":"chatgpt","email":null,"plan":"prolite","subscription_until":null,
            "rate_limits":[{"name":"weekly","used_percent":50.0,"window_seconds":604800,"resets_at":now+chrono::Duration::days(1)}],
            "limit_reached":false,"credits":null,"as_of":now,"source":"live","note":null
        })).unwrap();
        assert_eq!(remaining(&status, now).unwrap(), 50.0);
        let mut second = status.rate_limits[0].clone();
        second.name = "short".into();
        second.used_percent = 60.0;
        status.rate_limits.push(second);
        assert_eq!(remaining(&status, now).unwrap(), 40.0);
        status.rate_limits[1].resets_at = Some(now - chrono::Duration::seconds(1));
        assert_eq!(remaining(&status, now).unwrap(), 50.0);
        status.as_of = Some(now - chrono::Duration::seconds(181));
        assert!(remaining(&status, now).is_err());
        status.as_of = Some(now);
        status.rate_limits.clear();
        assert!(remaining(&status, now).is_err());
    }
}

#[cfg(test)]
mod gemini_tests {
    use super::*;

    #[test]
    fn only_gemini_weekly_counts_and_unknown_does_not_authorize_work() {
        let now = Utc::now();
        let expires = now + chrono::Duration::days(2);
        let mut status: AccountStatus = serde_json::from_value(serde_json::json!({
            "logged_in": true, "limit_reached": false, "source": "live", "as_of": now,
            "rate_limits": [
                {"name": "Gemini 5h", "used_percent": 99.0, "resets_at": expires},
                {"name": "Claude/GPT 每周", "used_percent": 99.0, "resets_at": expires},
                {"name": "Gemini 每周", "used_percent": 49.0, "resets_at": expires}
            ]
        }))
        .unwrap();
        assert_eq!(gemini_weekly_remaining(&status, now), Ok(51.0));
        status.rate_limits[2].used_percent = 50.0;
        assert_eq!(gemini_weekly_remaining(&status, now), Ok(50.0));
        status.rate_limits.pop();
        assert!(gemini_weekly_remaining(&status, now).is_err());
        assert_eq!(resumable_status(Some("手动暂停")), None);
        assert_eq!(
            resumable_status(Some(&format!("{QUOTA_WAIT} · running"))),
            Some(RunStatus::Running)
        );
    }
}
