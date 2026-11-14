//! Signing in to Gemini CLI through its ACP channel.
//!
//! Gemini CLI has no `login` subcommand: its Google sign-in lives in the interactive
//! first-run dialog, and without a terminal it exits with "Please set an Auth method".
//! Its ACP mode does take `authenticate`, and that is the one non-interactive way in:
//! the CLI opens the browser itself and finishes when the person is done.
//!
//! The hub runs this through `crossagents gemini-login` (a hidden subcommand), because a
//! login command is a process that the hub starts and watches, not a conversation.

use std::process::Stdio;
use std::time::Duration;

use crossagents_core::CoreError;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::process::{hide_console, resolve_program};

/// How long the agent gets to answer `initialize`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the person gets in the browser.
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
/// The method that opens a browser on the Google account page.
const OAUTH: &str = "oauth-personal";

/// Runs `command --acp`, asks it to authenticate with Google, and waits for the browser
/// round trip. Returns when the CLI says the sign-in is done.
pub async fn login(command: &[String]) -> Result<(), CoreError> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| CoreError::Agent("empty Gemini command".into()))?;
    let mut child = Command::new(resolve_program(program));
    child
        .args(args)
        .arg("--acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut child);
    let mut child = child
        .spawn()
        .map_err(|e| CoreError::Agent(format!("cannot start Gemini CLI: {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stderr = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            tail.push(line);
            if tail.len() > 40 {
                tail.remove(0);
            }
        }
        tail.join("\n")
    });
    let mut lines = BufReader::new(stdout).lines();

    // initialize: the auth methods the CLI offers, and the agent's own version.
    send(&mut stdin, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {}}})).await?;
    let ready = reply(&mut lines, &mut child, 1, HANDSHAKE_TIMEOUT).await?;
    let methods: Vec<String> = ready["authMethods"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|m| m["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if !methods.iter().any(|m| m == OAUTH) {
        return Err(CoreError::Agent(format!(
            "Gemini CLI does not offer a Google sign-in here (methods: {})",
            if methods.is_empty() {
                "none".to_owned()
            } else {
                methods.join(", ")
            }
        )));
    }

    // authenticate: the CLI opens the browser and answers when it is done.
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "authenticate", "params": {"methodId": OAUTH}}),
    )
    .await?;
    match reply(&mut lines, &mut child, 2, AUTH_TIMEOUT).await {
        Ok(_) => Ok(()),
        Err(err) => {
            // The browser step is where this fails in practice (closed window, another
            // account), and the CLI's stderr is the only thing that says why.
            let _ = child.kill().await;
            let tail = stderr.await.unwrap_or_default();
            let message = err.to_string();
            Err(CoreError::Agent(if tail.trim().is_empty() {
                message
            } else {
                format!("{message}: {}", tail.trim())
            }))
        }
    }
}

async fn send(stdin: &mut tokio::process::ChildStdin, value: Value) -> Result<(), CoreError> {
    let mut line = value.to_string();
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| CoreError::Agent(format!("cannot talk to Gemini CLI: {e}")))?;
    stdin
        .flush()
        .await
        .map_err(|e| CoreError::Agent(format!("cannot talk to Gemini CLI: {e}")))
}

/// Reads until the answer to `id` arrives. Other lines (notifications, the CLI's own
/// logging) are skipped.
async fn reply(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    child: &mut tokio::process::Child,
    id: u64,
    within: Duration,
) -> Result<Value, CoreError> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let line = match tokio::time::timeout_at(deadline, lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => {
                return Err(CoreError::Agent(format!(
                    "Gemini CLI stopped before answering request {id}{}",
                    exit_note(child).await
                )));
            }
            Ok(Err(e)) => return Err(CoreError::Agent(format!("Gemini CLI output: {e}"))),
            Err(_) => {
                let _ = child.kill().await;
                return Err(CoreError::Agent(format!(
                    "Gemini CLI did not answer within {} s",
                    within.as_secs()
                )));
            }
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value["id"].as_u64() != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let message = error["message"]
                .as_str()
                .unwrap_or("the CLI refused the request");
            return Err(CoreError::Agent(message.to_owned()));
        }
        return Ok(value["result"].clone());
    }
}

async fn exit_note(child: &mut tokio::process::Child) -> String {
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => format!(" ({status})"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for `gemini --acp`: answers initialize with the auth methods, then
    /// authenticate as the CLI does.
    fn fake_cli(answer: &'static str) -> (Vec<String>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-gemini.mjs");
        std::fs::write(&script, answer).unwrap();
        (vec!["node".into(), script.display().to_string()], dir)
    }

    const HAPPY: &str = r#"
import { createInterface } from "node:readline";
const rl = createInterface({ input: process.stdin });
const send = (o) => process.stdout.write(JSON.stringify(o) + "\n");
rl.on("line", (line) => {
  const req = JSON.parse(line);
  if (req.method === "initialize") {
    send({ jsonrpc: "2.0", id: req.id, result: { authMethods: [{ id: "oauth-personal" }, { id: "gemini-api-key" }] } });
  } else if (req.method === "authenticate") {
    send({ jsonrpc: "2.0", id: req.id, result: {} });
  }
});
"#;

    const REFUSES: &str = r#"
import { createInterface } from "node:readline";
const rl = createInterface({ input: process.stdin });
const send = (o) => process.stdout.write(JSON.stringify(o) + "\n");
rl.on("line", (line) => {
  const req = JSON.parse(line);
  if (req.method === "initialize") {
    send({ jsonrpc: "2.0", id: req.id, result: { authMethods: [{ id: "gemini-api-key" }] } });
  }
});
"#;

    const NO_GOOGLE: &str = r#"
import { createInterface } from "node:readline";
const rl = createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const req = JSON.parse(line);
  if (req.method === "initialize") {
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: req.id, result: { authMethods: [{ id: "oauth-personal" }] } }) + "\n");
  } else if (req.method === "authenticate") {
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: req.id, error: { code: -32000, message: "sign-in cancelled" } }) + "\n");
    process.exit(0);
  }
});
"#;

    #[tokio::test]
    async fn the_browser_step_is_done_when_the_cli_answers() {
        let (command, _dir) = fake_cli(HAPPY);
        login(&command).await.expect("signed in");
    }

    #[tokio::test]
    async fn a_cli_without_google_sign_in_is_refused_before_anything_opens() {
        let (command, _dir) = fake_cli(REFUSES);
        let err = login(&command).await.unwrap_err().to_string();
        assert!(err.contains("does not offer a Google sign-in"), "{err}");
        assert!(err.contains("gemini-api-key"), "{err}");
    }

    #[tokio::test]
    async fn the_clis_own_refusal_is_what_the_person_sees() {
        let (command, _dir) = fake_cli(NO_GOOGLE);
        let err = login(&command).await.unwrap_err().to_string();
        assert!(err.contains("sign-in cancelled"), "{err}");
    }
}
