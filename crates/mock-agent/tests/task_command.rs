use common::{TestHub, crossagents_bin, git_repo, wait_limit};
use crossagents_core::{EventKind, HumanItem, HumanItemStatus, Run, Task, TaskStatus};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

mod common;

#[tokio::test]
async fn only_live_approved_commands_start() {
    let repo = git_repo();
    let t = TestHub::start().await;
    let boss = t.add_agent_in("boss", None, "planner", repo.path()).await;
    t.add_agent_in("coder", None, "worker", repo.path()).await;
    let reply = t.post("/v1/runs", json!({
        "planner": "boss", "worker": "coder",
        "request": r#"[call plan_submit {"tasks":[{"key":"probe","title":"Command probe","spec":{"goal":"Wait [call task_complete {\"task\":\"probe\",\"summary\":\"ready\"}]"}}]}]"#,
        "policy": {"plan_approval":"auto", "human_ask":"timeout", "ask_timeout_minutes":0.05}
    })).await;
    assert_eq!(reply.status(), 201);
    let run: Run = reply.json().await.unwrap();
    let task: Task = tokio::time::timeout(wait_limit(), async {
        loop {
            if let Some(task) = t
                .hub
                .orchestrator
                .store()
                .list_tasks(run.id)
                .await
                .unwrap()
                .into_iter()
                .find(|task| task.status == TaskStatus::Passed && task.worktree.is_some())
            {
                break task;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("task has a real worktree and a completed report");
    let dir = task.worktree.as_ref().unwrap();
    let token = t.hub.agent_tokens.issue(boss.id).await;
    let url = format!("{}/v1/me/tools/task_command", t.base);
    let body = |name: &str| {
        json!({
            "task":"probe", "command":format!("echo ran> {name}"),
            "purpose":"Test marker in the isolated worktree; no GPU", "timeout_secs":10
        })
    };
    let request = |name: &str| t.client.post(&url).bearer_auth(&token).json(&body(name));

    let mut native = boss.clone();
    native.plugin = "antigravity".into();
    native.config = json!({"permission":"guard"});
    let names = t.hub.orchestrator.tools_for(&native).await;
    assert!(names.iter().any(|tool| tool.name == "task_command"));
    for legacy in ["workspace_run", "checks_run", "task_verify"] {
        assert!(!names.iter().any(|tool| tool.name == legacy));
        let refused = t
            .hub
            .orchestrator
            .call_tool(&native, legacy, body("legacy.txt"))
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("protected command execution"));
    }
    assert!(!dir.join("legacy.txt").exists());

    let mut watch = t.watch().await;
    let denied = tokio::spawn(request("denied.txt").send());
    let card = asked(&mut watch).await;
    assert!(
        card.detail
            .as_deref()
            .unwrap()
            .contains("echo ran> denied.txt")
    );
    assert!(card.detail.as_deref().unwrap().contains("no GPU"));
    let deny = t
        .post(
            &format!("/v1/questions/{}/answer", card.id),
            json!({"answer":card.options[1]}),
        )
        .await;
    assert_eq!(deny.status(), 200);
    assert_eq!(denied.await.unwrap().unwrap().status(), 409);
    assert!(!dir.join("denied.txt").exists());

    let expired = tokio::spawn(request("expired.txt").send());
    let card = asked(&mut watch).await;
    assert_eq!(expired.await.unwrap().unwrap().status(), 409);
    let late = t
        .post(
            &format!("/v1/questions/{}/answer", card.id),
            json!({"answer":card.options[0]}),
        )
        .await;
    assert_eq!(late.status(), 409);
    assert!(!dir.join("expired.txt").exists());

    // A real TCP client goes away mid-approval. Closing the client must drop
    // the hub's command waiter even though ordinary collaboration tools detach.
    let payload = body("disconnected.txt").to_string();
    let mut stream = TcpStream::connect(t.hub.addr).await.unwrap();
    let http = format!(
        "POST /v1/me/tools/task_command HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        t.hub.addr,
        token,
        payload.len(),
        payload
    );
    stream.write_all(http.as_bytes()).await.unwrap();
    let card = asked(&mut watch).await;
    drop(stream);
    disconnected(&t, &card).await;
    assert!(!dir.join("disconnected.txt").exists());

    let mut mcp = tokio::process::Command::new(crossagents_bin())
        .arg("mcp")
        .env("CROSSAGENTS_URL", &t.base)
        .env("CROSSAGENTS_AGENT_TOKEN", &token)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = mcp.stdin.take().unwrap();
    let mcp_request = |id, name| {
        format!(
            "{}\n",
            json!({
                "jsonrpc":"2.0", "id":id, "method":"tools/call",
                "params":{"name":"task_command", "arguments":body(name)}
            })
        )
    };
    stdin
        .write_all(mcp_request(1, "cancelled.txt").as_bytes())
        .await
        .unwrap();
    let card = asked(&mut watch).await;
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":1}}\n").await.unwrap();
    disconnected(&t, &card).await;
    assert!(!dir.join("cancelled.txt").exists());

    stdin
        .write_all(mcp_request(2, "stdio-closed.txt").as_bytes())
        .await
        .unwrap();
    let card = asked(&mut watch).await;
    drop(stdin);
    assert!(mcp.wait().await.unwrap().success());
    disconnected(&t, &card).await;
    assert!(!dir.join("stdio-closed.txt").exists());

    let allowed = tokio::spawn(request("allowed.txt").send());
    let card = asked(&mut watch).await;
    let reply = t
        .post(
            &format!("/v1/questions/{}/answer", card.id),
            json!({"answer":card.options[0]}),
        )
        .await;
    assert_eq!(reply.status(), 200);
    let reply: Value = allowed.await.unwrap().unwrap().json().await.unwrap();
    assert_eq!(reply["authorization"], "one_time_approval", "{reply}");
    assert_eq!(reply["exit_code"], 0, "{reply}");
    assert!(dir.join("allowed.txt").exists());

    // A scoped grant short-circuits the prompt. The executed command remains exact.
    std::fs::write(
        t.home.path().join("task-command-grants.json"),
        json!([{
            "agent": boss.id, "task": task.id, "directory": dir,
            "command": "echo ran> grant.txt", "timeout_secs": 10,
            "expires_at": chrono::Utc::now() + chrono::Duration::minutes(1)
        }])
        .to_string(),
    )
    .unwrap();
    let reply: Value = request("grant.txt")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reply["authorization"], "existing_grant", "{reply}");
    assert_eq!(reply["exit_code"], 0, "{reply}");
    assert!(dir.join("grant.txt").exists());
}

async fn asked(watch: &mut common::Watcher) -> HumanItem {
    let event = watch
        .until("command approval", |event| {
            matches!(&event.kind, EventKind::HumanAsked { item }
            if item.action == Some(crossagents_core::HumanAction::Approval))
        })
        .await;
    let EventKind::HumanAsked { item } = event.kind else {
        unreachable!()
    };
    item
}

async fn disconnected(t: &TestHub, card: &HumanItem) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            t.hub.orchestrator.expire_questions().await.unwrap();
            let stored = t
                .hub
                .orchestrator
                .store()
                .get_human_item(&card.id)
                .await
                .unwrap();
            if stored.status == HumanItemStatus::Dismissed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("disconnect cancels immediately, before the approval deadline");
    let late = t
        .post(
            &format!("/v1/questions/{}/answer", card.id),
            json!({"answer":card.options[0]}),
        )
        .await;
    assert_eq!(late.status(), 409);
}
