//! The ACP driver against the mock agent process.

use std::time::Duration;

use crossagents_core::{
    DriverSession, OutputStream, PermissionPolicy, SessionEvent, SessionEventKind, StopReason,
    ToolStatus,
};
use crossagents_drivers::acp::{AcpLaunch, AcpOptions, AcpSession};
use tokio::sync::mpsc;

fn launch() -> AcpLaunch {
    AcpLaunch {
        program: env!("CARGO_BIN_EXE_crossagents-mock-agent").into(),
        args: vec![],
        env: vec![],
        cwd: std::env::temp_dir(),
    }
}

async fn start(
    permission: PermissionPolicy,
    model: Option<&str>,
    instructions: Option<&str>,
) -> (AcpSession, mpsc::UnboundedReceiver<SessionEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let session = AcpSession::start(AcpOptions {
        launch: launch(),
        mcp_servers: vec![],
        permission,
        permissions: None,
        model: model.map(Into::into),
        effort: None,
        instructions: instructions.map(Into::into),
        events: tx,
    })
    .await
    .unwrap();
    (session, rx)
}

fn drain_text(rx: &mut mpsc::UnboundedReceiver<SessionEvent>) -> (String, Vec<SessionEventKind>) {
    let mut text = String::new();
    let mut others = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event.kind {
            SessionEventKind::Output {
                stream: OutputStream::Text,
                text: t,
            } => text.push_str(&t),
            other => others.push(other),
        }
    }
    (text, others)
}

#[tokio::test]
async fn prompt_selects_model_and_mode_and_streams_output() {
    let (session, mut rx) = start(
        PermissionPolicy::Bypass,
        Some("haiku"),
        Some(
            "You are w1.

About you: careful with tests.",
        ),
    )
    .await;
    assert_eq!(
        session.agent_info.as_ref().unwrap().name,
        "crossagents-mock-agent"
    );
    assert_eq!(session.session_id(), "mock-session");

    let outcome = session.prompt("hello").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(outcome.turn_id, None);
    let (text, others) = drain_text(&mut rx);
    assert_eq!(
        text,
        "[instructions received] echo(model=mock-haiku,mode=bypassPermissions): hello"
    );
    assert!(
        others
            .iter()
            .any(|e| matches!(e, SessionEventKind::Usage { used: 42, .. }))
    );
    assert!(others.iter().any(|e| matches!(
        e,
        SessionEventKind::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            ..
        }
    )));

    // Instructions are only sent once.
    session.prompt("again").await.unwrap();
    let (text, _) = drain_text(&mut rx);
    assert!(!text.contains("instructions"), "{text}");
    session.shutdown().await;
}

#[tokio::test]
async fn unknown_model_is_a_warning() {
    let (session, mut rx) = start(PermissionPolicy::Bypass, Some("gpt-9"), None).await;
    let (_, others) = drain_text(&mut rx);
    assert!(
        others.iter().any(
            |e| matches!(e, SessionEventKind::Warning { message } if message.contains("gpt-9"))
        ),
        "{others:?}"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn permissions_follow_policy() {
    let (session, mut rx) = start(PermissionPolicy::Bypass, None, None).await;
    session.prompt("[tool] go").await.unwrap();
    let (text, others) = drain_text(&mut rx);
    assert!(text.starts_with("permission=yes"), "{text}");
    assert!(others.iter().any(|e| matches!(
        e,
        SessionEventKind::ToolCallUpdated {
            status: Some(ToolStatus::Completed),
            ..
        }
    )));
    session.shutdown().await;

    let (session, mut rx) = start(PermissionPolicy::Deny, None, None).await;
    session.prompt("[tool] go").await.unwrap();
    let (text, _) = drain_text(&mut rx);
    assert!(text.starts_with("permission=no"), "{text}");
    session.shutdown().await;
}

#[tokio::test]
async fn a_guarded_acp_command_waits_for_the_host_approval() {
    use crossagents_core::{PermissionDecision, PermissionHandler, PermissionRequest};
    use std::sync::Arc;
    use tokio::sync::Notify;

    #[derive(Default)]
    struct HostApproval {
        requested: Notify,
        approved: Notify,
    }

    #[async_trait::async_trait]
    impl PermissionHandler for HostApproval {
        async fn decide(&self, request: &PermissionRequest) -> PermissionDecision {
            assert_eq!(request.kind, "acp_tool");
            self.requested.notify_one();
            self.approved.notified().await;
            PermissionDecision::Allow
        }
    }

    let approvals = Arc::new(HostApproval::default());
    let (events, mut rx) = mpsc::unbounded_channel();
    let session = Arc::new(AcpSession::start(AcpOptions {
        launch: launch(),
        mcp_servers: Vec::new(),
        permission: PermissionPolicy::Guard,
        permissions: Some(approvals.clone()),
        model: None,
        effort: None,
        instructions: None,
        events,
    }).await.unwrap());
    let turn = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("[tool] go").await }
    });
    tokio::time::timeout(Duration::from_secs(5), approvals.requested.notified()).await.unwrap();
    assert!(!turn.is_finished());
    assert!(!drain_text(&mut rx).0.contains("permission=yes"));

    approvals.approved.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(5), turn).await.unwrap().unwrap().unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    let (text, events) = drain_text(&mut rx);
    assert!(text.starts_with("permission=yes"), "{text}");
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEventKind::PermissionRequested { decision: PermissionDecision::Allow, .. }
    )));
    session.shutdown().await;
}

#[tokio::test]
async fn interrupt_stops_a_running_turn() {
    let (session, _rx) = start(PermissionPolicy::Bypass, None, None).await;
    let session = std::sync::Arc::new(session);
    let turn = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("[slow]").await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    session.interrupt().await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), turn)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    session.shutdown().await;
}

#[tokio::test]
async fn crash_is_reported() {
    let (session, mut rx) = start(PermissionPolicy::Bypass, None, None).await;
    assert!(session.prompt("[crash]").await.is_err());
    let exited = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(SessionEvent {
                kind: SessionEventKind::Exited { .. },
                ..
            }) = rx.recv().await
            {
                return;
            }
        }
    })
    .await;
    assert!(exited.is_ok(), "no Exited event");
}

#[tokio::test]
async fn shutdown_is_not_reported_as_exit() {
    let (session, mut rx) = start(PermissionPolicy::Bypass, None, None).await;
    session.shutdown().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, others) = drain_text(&mut rx);
    assert!(
        !others
            .iter()
            .any(|e| matches!(e, SessionEventKind::Exited { .. })),
        "{others:?}"
    );
}

#[tokio::test]
async fn optional_operations_are_unsupported() {
    let (session, _rx) = start(PermissionPolicy::Bypass, None, None).await;
    for result in [
        session.suspend().await,
        session.rollback("t", false).await,
        session.set_model(Some("m"), None).await,
    ] {
        assert!(
            matches!(result, Err(crossagents_core::CoreError::Unsupported(_))),
            "{result:?}"
        );
    }
    session.compact().await.unwrap();
    assert!(session.take_context_rotated());
    session.shutdown().await;
}
