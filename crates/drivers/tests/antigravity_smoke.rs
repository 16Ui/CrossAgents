//! Smoke tests against the real Antigravity CLI. All tests are ignored by default.
//! Requires an installed, signed-in `agy` (run it once and sign in with Google); the model
//! test also needs two model choices on the account. These tests spend quota and leave
//! conversations in the CLI's history. Install the CLI only with the user's approval;
//! compilation needs no CLI.
//!
//! ```text
//! CROSSAGENTS_SMOKE_AGY=<launch command>   optional, default agy
//! CROSSAGENTS_SMOKE_MODEL=<model>          optional, default gemini-3.8-flash
//! CROSSAGENTS_SMOKE_NEXT_MODEL=<model>     optional model-switch target
//! cargo test -p crossagents-drivers --test antigravity_smoke -- --ignored --test-threads=1
//! ```
//!
//! Launch commands are split on whitespace, as in cursor_smoke.rs (no shell quoting).
//! Compile only: `cargo test -p crossagents-drivers --test antigravity_smoke --no-run`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use crossagents_core::{
    Agent, AgentId, AgentStatus, Driver, DriverSession, OutputStream, PermissionPolicy, Role,
    SessionEvent, SessionEventKind, SessionSpec, StopReason, TurnOutcome,
};
use crossagents_drivers::antigravity_native::AntigravityDriver;
use crossagents_drivers::locate::Cli;
use serde_json::json;
use tokio::sync::mpsc;

const TURN_TIMEOUT: Duration = Duration::from_secs(300);

fn driver() -> AntigravityDriver {
    let command = std::env::var("CROSSAGENTS_SMOKE_AGY")
        .unwrap_or_else(|_| "agy".into())
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    AntigravityDriver::new("antigravity", Cli::at(command), vec![])
}

fn spec(cwd: &Path) -> (SessionSpec, mpsc::UnboundedReceiver<SessionEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let spec = SessionSpec {
        agent: Agent {
            node: None,
            id: AgentId::new(),
            name_source: crossagents_core::NameSource::Chosen,
            topic: None,
            archived_at: None,
            home: PathBuf::new(),
            name: "smoke".into(),
            plugin: "antigravity".into(),
            driver: Some("antigravity-native".into()),
            model: Some(
                std::env::var("CROSSAGENTS_SMOKE_MODEL")
                    .unwrap_or_else(|_| "gemini-3.8-flash".into()),
            ),
            role: Role::Worker,
            cwd: cwd.to_path_buf(),
            config: json!({}),
            status: AgentStatus::Stopped,
            created_at: Utc::now(),
            profile: Default::default(),
        },
        mcp_servers: vec![],
        permission: PermissionPolicy::Bypass,
        permissions: None,
        command: None,
        instructions: Some(
            "You are a test fixture. Answer as briefly as possible. When asked for the \
             fixture word, answer BLUEBERRY."
                .into(),
        ),
        env: vec![],
        events: tx,
    };
    (spec, rx)
}

fn drain(rx: &mut mpsc::UnboundedReceiver<SessionEvent>) -> (String, Vec<SessionEventKind>) {
    let mut streamed = String::new();
    let mut others = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event.kind {
            SessionEventKind::Output {
                stream: OutputStream::Text,
                text,
            } => streamed.push_str(&text),
            other => others.push(other),
        }
    }
    (streamed, others)
}

async fn turn(
    session: &dyn DriverSession,
    rx: &mut mpsc::UnboundedReceiver<SessionEvent>,
    text: &str,
) -> (TurnOutcome, String, Vec<SessionEventKind>) {
    let outcome = tokio::time::timeout(TURN_TIMEOUT, session.prompt(text))
        .await
        .expect("turn timed out")
        .expect("turn failed");
    let (streamed, others) = drain(rx);
    assert_eq!(
        outcome.stop_reason,
        StopReason::EndTurn,
        "{streamed} {others:?}"
    );
    (outcome, streamed, others)
}

/// The conversation id the CLI reported, which is what a later `resume` takes.
fn conversation(rx: &mut mpsc::UnboundedReceiver<SessionEvent>) -> String {
    while let Ok(event) = rx.try_recv() {
        if let SessionEventKind::Started { session_id } = event.kind {
            return session_id;
        }
    }
    panic!("the CLI never reported a conversation id");
}

#[tokio::test]
#[ignore = "needs an installed, signed-in Antigravity CLI"]
async fn smoke_text_and_opening_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let (spec, mut rx) = spec(dir.path());
    let session = driver().open(spec).await.unwrap();
    let (_, text, _) = turn(&*session, &mut rx, "What is the fixture word?").await;
    assert!(text.contains("BLUEBERRY"), "{text}");
    session.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an installed, signed-in Antigravity CLI"]
async fn smoke_resume_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    let driver = driver();
    let (spec_a, mut rx) = spec(dir.path());
    let session = driver.open(spec_a).await.unwrap();
    turn(
        &*session,
        &mut rx,
        "Remember the code word OKAPI-31. Reply with exactly: OK",
    )
    .await;
    let id = conversation(&mut rx);
    drop(session);

    let (mut spec_b, mut rx) = spec(dir.path());
    spec_b.instructions = None;
    let resumed = driver.resume(&id, spec_b).await.unwrap();
    assert_eq!(resumed.session_id(), id);
    let (_, text, _) = turn(
        &*resumed,
        &mut rx,
        "What was the code word? Reply with it only.",
    )
    .await;
    assert!(text.contains("OKAPI-31"), "{text}");
    resumed.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an installed, signed-in Antigravity CLI and two model choices"]
async fn smoke_model_switch() {
    let dir = tempfile::tempdir().unwrap();
    let (spec, mut rx) = spec(dir.path());
    let initial = spec.agent.model.clone().unwrap();
    let next = std::env::var("CROSSAGENTS_SMOKE_NEXT_MODEL").unwrap_or_else(|_| {
        if initial == "gemini-3.1-pro" {
            "gemini-3.8-flash"
        } else {
            "gemini-3.1-pro"
        }
        .into()
    });
    assert_ne!(initial, next, "choose a different model for the switch");
    let session = driver().open(spec).await.unwrap();
    turn(&*session, &mut rx, "Reply with exactly: READY").await;
    let id = conversation(&mut rx);
    session.set_model(Some(&next), None).await.unwrap();
    let (_, text, _) = turn(&*session, &mut rx, "What is the fixture word?").await;
    assert!(text.contains("BLUEBERRY"), "{text}");
    // The process was restarted with the new model, in the same conversation.
    assert_eq!(session.session_id(), id);
    session.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an installed, signed-in Antigravity CLI"]
async fn smoke_models_are_asked_of_the_cli() {
    let driver = driver();
    assert!(driver.installed(), "agy is not on this machine");
    driver.probe_cli().await;
    let models = driver.models();
    assert!(!models.is_empty(), "no models: {models:?}");
    assert!(
        models.iter().any(|m| m.contains("gemini")),
        "the list does not look like the CLI's: {models:?}"
    );
}

#[tokio::test]
#[ignore = "needs an installed, signed-in Antigravity CLI"]
async fn smoke_interrupt_kills_process() {
    let dir = tempfile::tempdir().unwrap();
    let (spec, mut rx) = spec(dir.path());
    let session: std::sync::Arc<dyn DriverSession> = driver().open(spec).await.unwrap().into();
    let running = tokio::spawn({
        let session = session.clone();
        async move {
            session
                .prompt("Count slowly from one to five hundred, one number per line.")
                .await
        }
    });
    tokio::time::sleep(Duration::from_secs(3)).await;
    session.interrupt().await.unwrap();
    let outcome = tokio::time::timeout(TURN_TIMEOUT, running)
        .await
        .expect("turn did not end after interrupt")
        .unwrap()
        .unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    let _ = drain(&mut rx);
    session.shutdown().await;
}
