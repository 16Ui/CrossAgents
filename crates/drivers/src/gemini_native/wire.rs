//! Gemini CLI's headless `stream-json` (docs/research/gemini-cli.md).
//!
//! One JSON object per line: `init`, `message`, `tool_use`, `tool_result`, `error`,
//! `result`. Only assistant messages carry text, and only as deltas — the final
//! answer is not repeated. Thinking is not part of this stream.

use crossagents_core::{
    OutputStream, SessionEvent, SessionEventKind, StopReason, ToolDetail, ToolStatus,
};
use serde_json::Value;

pub struct TurnArgs<'a> {
    pub session_id: &'a str,
    /// The first turn of a new session opens it (`--session-id`); later turns resume it.
    pub first: bool,
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    /// `yolo`, `default`, `auto_edit` or `plan`.
    pub approval: &'a str,
}

/// The argv of one turn, without the program itself.
///
/// `--prompt=` rather than `-p <text>`: a prompt that begins with `-` would otherwise be
/// read as another flag. An empty value means "the prompt is on stdin" (the CLI appends
/// stdin to `--prompt`), which is how a message too long for a Windows command line is sent.
pub fn turn_args(args: &TurnArgs<'_>) -> Vec<String> {
    let mut out = vec![
        format!("--prompt={}", args.prompt),
        "-o".into(),
        "stream-json".into(),
        // Non-interactive: the folder-trust question would otherwise stop the turn.
        "--skip-trust".into(),
    ];
    if args.first {
        out.push(format!("--session-id={}", args.session_id));
    } else {
        out.push(format!("--resume={}", args.session_id));
    }
    if let Some(model) = args.model.filter(|m| !m.is_empty()) {
        out.push(format!("--model={model}"));
    }
    out.push(format!("--approval-mode={}", args.approval));
    out
}

#[derive(Debug, PartialEq)]
pub enum Line {
    Events(Vec<SessionEvent>),
    Text(String),
    Skip,
}

#[derive(Debug, Default)]
pub struct Turn {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub stop_reason: Option<StopReason>,
    /// `stats` of the terminal `result` event: the turn's token counts.
    pub stats: Option<Value>,
    /// An `error` event with `severity: "error"` was seen, so a failed `result` need
    /// not repeat its message.
    failed: bool,
    tool_kinds: std::collections::HashMap<String, &'static str>,
}

impl Turn {
    pub fn line(&mut self, line: &str) -> Line {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            return Line::Skip;
        }
        let Ok(raw) = serde_json::from_str::<Value>(line) else {
            return Line::Text(line.to_owned());
        };
        let mut events = Vec::new();
        match raw["type"].as_str().unwrap_or_default() {
            "init" => {
                self.session_id = text(&raw, "session_id");
                self.model = text(&raw, "model");
            }
            // The user message echoes the prompt we sent; only the answer is news.
            "message" if raw["role"] == "assistant" => {
                if let Some(text) = raw["content"].as_str().filter(|s| !s.is_empty()) {
                    events.push(SessionEventKind::Output {
                        stream: OutputStream::Text,
                        text: text.into(),
                    });
                }
            }
            "tool_use" => {
                if let Some(id) = text(&raw, "tool_id") {
                    let name = text(&raw, "tool_name").unwrap_or_else(|| "tool".into());
                    let kind = tool_kind(&name);
                    let command = command_of(&raw["parameters"]);
                    let title = tool_title(&name, kind, &raw["parameters"], command.as_deref());
                    self.tool_kinds.insert(id.clone(), kind);
                    events.push(SessionEventKind::ToolCall {
                        tool_call_id: id.clone(),
                        title,
                        kind: Some(kind.into()),
                        status: ToolStatus::InProgress,
                    });
                    events.push(SessionEventKind::ToolDetail {
                        tool_call_id: id,
                        detail: ToolDetail {
                            command,
                            arguments: if kind == "execute" {
                                None
                            } else {
                                raw.get("parameters")
                                    .filter(|p| !p.is_null())
                                    .map(json_text)
                            },
                            ..ToolDetail::default()
                        },
                    });
                }
            }
            "tool_result" => {
                if let Some(id) = text(&raw, "tool_id") {
                    let kind = self.tool_kinds.remove(&id).unwrap_or("other");
                    let failed = raw["status"] == "error";
                    let output = raw["output"].as_str().unwrap_or_default();
                    let error = raw["error"]["message"]
                        .as_str()
                        .filter(|m| !m.is_empty())
                        .map(str::to_owned);
                    if !output.is_empty() {
                        events.push(SessionEventKind::ToolOutput {
                            tool_call_id: id.clone(),
                            text: output.into(),
                        });
                    }
                    events.push(SessionEventKind::ToolDetail {
                        tool_call_id: id.clone(),
                        detail: ToolDetail {
                            output: (kind == "execute" && !output.is_empty())
                                .then(|| output.to_owned()),
                            result: (kind != "execute" && !output.is_empty())
                                .then(|| output.to_owned()),
                            error: error.clone(),
                            ..ToolDetail::default()
                        },
                    });
                    events.push(SessionEventKind::ToolCallUpdated {
                        tool_call_id: id,
                        title: None,
                        status: Some(if failed {
                            ToolStatus::Failed
                        } else {
                            ToolStatus::Completed
                        }),
                    });
                }
            }
            // Warnings and errors inside a turn; a failed `result` still follows.
            "error" => {
                if let Some(message) = text(&raw, "message") {
                    self.failed |= raw["severity"] == "error";
                    events.push(SessionEventKind::Warning { message });
                }
            }
            "result" => {
                let failed = raw["status"] == "error";
                self.stop_reason = Some(if failed {
                    StopReason::Error
                } else {
                    StopReason::EndTurn
                });
                if failed && !self.failed {
                    events.push(SessionEventKind::Error {
                        message: "Gemini reported the turn as failed".into(),
                    });
                }
                self.stats = raw.get("stats").filter(|s| s.is_object()).cloned();
            }
            _ => {}
        }
        if events.is_empty() {
            Line::Skip
        } else {
            Line::Events(
                events
                    .into_iter()
                    .map(|kind| SessionEvent::with_raw(kind, raw.clone()))
                    .collect(),
            )
        }
    }
}

/// The kind of tool, in the hub's words, for the GUI's icons.
fn tool_kind(name: &str) -> &'static str {
    match name {
        "read_file" | "read_many_files" | "list_directory" | "glob" => "read",
        "write_file" | "replace" => "edit",
        "run_shell_command" => "execute",
        "google_web_search" => "search",
        "web_fetch" => "fetch",
        _ => "other",
    }
}

/// A shell tool's command line, for the tool card.
fn command_of(parameters: &Value) -> Option<String> {
    parameters["command"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn tool_title(name: &str, kind: &str, parameters: &Value, command: Option<&str>) -> String {
    if kind == "execute"
        && let Some(cmd) = command
    {
        return cmd.lines().next().unwrap_or(cmd).trim().to_owned();
    }
    if let Some(path) = ["file_path", "path", "dir_path"]
        .iter()
        .find_map(|k| parameters.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match kind {
            "read" => return format!("Read {path}"),
            "edit" => return format!("Edit {path}"),
            _ => {}
        }
    }
    name.to_owned()
}

/// The ids `gemini --list-models` prints, one `id (Display Name)` line each.
pub fn parse_models(text: &str) -> Vec<String> {
    let mut models = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((id, rest)) = line.split_once(' ') else {
            continue;
        };
        if rest.starts_with('(') && rest.ends_with(')') && !id.is_empty() {
            models.push(id.to_owned());
        }
    }
    models
}

fn text(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

fn json_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs(line: Line) -> String {
        match line {
            Line::Events(events) => events
                .into_iter()
                .filter_map(|e| match e.kind {
                    SessionEventKind::Output {
                        stream: OutputStream::Text,
                        text,
                    } => Some(text),
                    _ => None,
                })
                .collect(),
            _ => String::new(),
        }
    }

    #[test]
    fn argv_opens_then_resumes_and_keeps_a_prompt_that_looks_like_a_flag() {
        let first = turn_args(&TurnArgs {
            session_id: "s-1",
            first: true,
            prompt: "--not-a-flag",
            model: Some("gemini-2.5-pro"),
            approval: "yolo",
        });
        assert_eq!(
            first,
            [
                "--prompt=--not-a-flag",
                "-o",
                "stream-json",
                "--skip-trust",
                "--session-id=s-1",
                "--model=gemini-2.5-pro",
                "--approval-mode=yolo"
            ]
        );
        let later = turn_args(&TurnArgs {
            session_id: "s-1",
            first: false,
            prompt: "",
            model: None,
            approval: "default",
        });
        assert!(later.contains(&"--resume=s-1".to_string()));
        assert!(later.contains(&"--approval-mode=default".to_string()));
        assert!(!later.iter().any(|a| a.starts_with("--model=")));
    }

    #[test]
    fn init_and_result_carry_the_session_and_the_stats() {
        let mut turn = Turn::default();
        assert_eq!(
            turn.line(
                r#"{"type":"init","timestamp":"t","session_id":"abc","model":"gemini-2.5-pro"}"#
            ),
            Line::Skip
        );
        assert_eq!(turn.session_id.as_deref(), Some("abc"));
        assert_eq!(turn.model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(
            turn.line(r#"{"type":"result","timestamp":"t","status":"success","stats":{"total_tokens":15,"input_tokens":10,"output_tokens":5}}"#),
            Line::Skip
        );
        assert_eq!(turn.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(turn.stats.unwrap()["total_tokens"], 15);
    }

    #[test]
    fn assistant_deltas_are_the_answer_and_the_echo_is_ignored() {
        let mut turn = Turn::default();
        assert_eq!(
            turn.line(r#"{"type":"message","role":"user","content":"hi"}"#),
            Line::Skip
        );
        assert_eq!(
            outputs(
                turn.line(r#"{"type":"message","role":"assistant","content":"he","delta":true}"#)
            ),
            "he"
        );
        assert_eq!(
            outputs(
                turn.line(r#"{"type":"message","role":"assistant","content":"llo","delta":true}"#)
            ),
            "llo"
        );
        assert_eq!(
            turn.line(r#"{"type":"message","role":"assistant","content":""}"#),
            Line::Skip
        );
    }

    #[test]
    fn tools_keep_their_arguments_output_and_status() {
        let mut turn = Turn::default();
        let Line::Events(started) = turn.line(
            r#"{"type":"tool_use","tool_name":"run_shell_command","tool_id":"t1","parameters":{"command":"ls -la"}}"#,
        ) else {
            panic!("tool_use");
        };
        assert!(matches!(
            &started[0].kind,
            SessionEventKind::ToolCall { tool_call_id, title, kind: Some(kind), status: ToolStatus::InProgress }
                if tool_call_id == "t1" && title == "ls -la" && kind == "execute"
        ));
        let SessionEventKind::ToolDetail { detail, .. } = &started[1].kind else {
            panic!("detail");
        };
        assert_eq!(detail.command.as_deref(), Some("ls -la"));
        assert!(detail.arguments.is_none());

        let Line::Events(done) = turn.line(
            r#"{"type":"tool_result","tool_id":"t1","status":"error","output":"","error":{"type":"X","message":"boom"}}"#,
        ) else {
            panic!("tool_result");
        };
        assert!(matches!(
            &done[0].kind,
            SessionEventKind::ToolDetail { detail, .. } if detail.error.as_deref() == Some("boom")
        ));
        assert!(matches!(
            done[1].kind,
            SessionEventKind::ToolCallUpdated {
                status: Some(ToolStatus::Failed),
                ..
            }
        ));
    }

    #[test]
    fn an_error_event_is_a_warning_and_a_failed_result_does_not_repeat_it() {
        let mut turn = Turn::default();
        let Line::Events(events) =
            turn.line(r#"{"type":"error","severity":"error","message":"quota exceeded"}"#)
        else {
            panic!("error");
        };
        assert!(matches!(
            &events[0].kind,
            SessionEventKind::Warning { message } if message == "quota exceeded"
        ));
        let line = turn.line(r#"{"type":"result","status":"error","stats":{"total_tokens":1}}"#);
        assert_eq!(line, Line::Skip, "the message was already sent");
        assert_eq!(turn.stop_reason, Some(StopReason::Error));

        let mut quiet = Turn::default();
        let Line::Events(events) =
            quiet.line(r#"{"type":"result","status":"error","stats":{"total_tokens":1}}"#)
        else {
            panic!("result");
        };
        assert!(matches!(events[0].kind, SessionEventKind::Error { .. }));
    }

    #[test]
    fn models_are_the_ids_the_list_names() {
        let listed = "Available Gemini Models:\n\nauto (Auto)\n  Description:   Let Gemini CLI decide\n  Context limit: 1,048,576 input tokens\n\ngemini-2.5-pro (gemini-2.5-pro)\n  Context limit: 1,048,576 input tokens\n";
        assert_eq!(parse_models(listed), ["auto", "gemini-2.5-pro"]);
        assert!(parse_models("no models here").is_empty());
    }
}
