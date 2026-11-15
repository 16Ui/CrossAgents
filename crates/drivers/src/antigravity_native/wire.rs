//! Antigravity CLI's `--output-format stream-json` (docs/research/antigravity-cli.md).
//!
//! One JSON object per line: `init` once, then `step_update` per step, then `result` when
//! the turn ends. `step_update` carries the streamed answer in `text_delta` fragments and
//! tool calls in `tool_info`; `result` repeats the answer and the session's cumulative
//! token counts.

use serde_json::Value;

/// One line of the stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Init(Init),
    Step(Box<Step>),
    Result(Box<Result>),
    /// A line this driver does not use (a newer CLI's event, a warning object).
    Other,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Init {
    pub conversation_id: Option<String>,
    pub model: Option<String>,
}

/// One step of the run: the answer being written, a tool call, a checkpoint.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    pub conversation_id: Option<String>,
    pub step_index: Option<u64>,
    /// `ACTIVE` while it runs, `DONE` when it ends.
    pub state: String,
    /// `user_input`, `agent_response`, `tool`, `checkpoint`.
    pub step_type: String,
    pub tool_name: Option<String>,
    pub text_delta: Option<String>,
    pub tool_info: Option<ToolInfo>,
    pub usage: Option<Usage>,
}

impl Step {
    pub fn running(&self) -> bool {
        self.state.eq_ignore_ascii_case("ACTIVE")
    }

    pub fn is_answer(&self) -> bool {
        self.step_type == "agent_response"
    }

    pub fn is_tool(&self) -> bool {
        self.step_type == "tool"
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolInfo {
    pub name: Option<String>,
    /// The call's arguments, as the CLI printed them.
    pub parameters: Option<Value>,
    pub output: Option<String>,
    /// `error.message`, when the call failed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Result {
    pub conversation_id: Option<String>,
    /// `SUCCESS`, `ERROR`, `CANCELED`, `INTERRUPTED`, `INVALID`, `WAITING`, `RUNNING`.
    pub status: String,
    pub response: String,
    pub error: Option<String>,
    pub duration_seconds: Option<f64>,
    pub num_turns: Option<u64>,
    /// Cumulative over the session, not this turn alone.
    pub usage: Option<Usage>,
}

/// Token counts. In `result` they are the session's running totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    /// Part of `output` spent thinking.
    pub thinking: u64,
    /// Part of `input` served from the prompt cache.
    pub cache_read: u64,
    pub total: u64,
}

impl Usage {
    fn from_json(value: &Value) -> Option<Self> {
        value.as_object()?;
        let num = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
        Some(Self {
            input: num("input_tokens"),
            output: num("output_tokens"),
            thinking: num("thinking_tokens"),
            cache_read: num("cache_read_tokens"),
            total: num("total_tokens"),
        })
    }
}

/// The event a line carries, or `None` when it is not JSON this driver knows.
pub fn parse(line: &str) -> Option<Event> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    Some(match value.get("event").and_then(Value::as_str)? {
        "init" => {
            let init = value.get("init").cloned().unwrap_or(Value::Null);
            Event::Init(Init {
                conversation_id: text(&value, "conversation_id")
                    .or_else(|| text(&init, "conversation_id")),
                model: text(&init, "model"),
            })
        }
        "step_update" => Event::Step(Box::new(step(value.get("step_update")?))),
        "result" => Event::Result(Box::new(result(value.get("result")?))),
        _ => Event::Other,
    })
}

fn step(value: &Value) -> Step {
    let info = value.get("tool_info").filter(|v| v.is_object());
    Step {
        conversation_id: text(value, "conversation_id"),
        step_index: value.get("step_index").and_then(Value::as_u64),
        state: text(value, "state").unwrap_or_default(),
        step_type: text(value, "step_type").unwrap_or_default(),
        tool_name: text(value, "tool_name"),
        text_delta: text(value, "text_delta"),
        tool_info: info.map(|info| ToolInfo {
            name: text(info, "name"),
            parameters: info.get("parameters").filter(|v| !v.is_null()).cloned(),
            output: text(info, "output"),
            error: info
                .get("error")
                .and_then(|error| text(error, "message").or_else(|| text(error, "type"))),
        }),
        usage: value.get("usage").and_then(Usage::from_json),
    }
}

fn result(value: &Value) -> Result {
    Result {
        conversation_id: text(value, "conversation_id"),
        status: text(value, "status").unwrap_or_default(),
        response: text(value, "response").unwrap_or_default(),
        error: text(value, "error"),
        duration_seconds: value.get("duration_seconds").and_then(Value::as_f64),
        num_turns: value.get("num_turns").and_then(Value::as_u64),
        usage: value.get("usage").and_then(Usage::from_json),
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The model slugs `agy models` lists, in order and without repeats.
///
/// The command's layout is not documented field by field, so this takes every token that
/// reads like a slug (`gemini-3.8-flash-high`, `claude-sonnet-4-5`, `auto`): lowercase
/// letters and digits with at least one `-`/`.` and a digit, or the bare word `auto`.
/// Titles ("Gemini 3.6 Flash (High)") and counters are left out.
pub fn parse_models(text: &str) -> Vec<String> {
    let mut models = Vec::new();
    for line in text.lines() {
        for token in line.split_whitespace() {
            let token = token.trim_matches(|c: char| matches!(c, ',' | ';' | '(' | ')' | '*'));
            if !looks_like_model(token) || models.iter().any(|m| m == token) {
                continue;
            }
            models.push(token.to_owned());
        }
    }
    models
}

fn looks_like_model(token: &str) -> bool {
    if token == "auto" {
        return true;
    }
    let mut chars = token.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    let mut digit = false;
    let mut separator = false;
    for c in token.chars() {
        if c.is_ascii_digit() {
            digit = true;
        } else if matches!(c, '-' | '.' | '_') {
            separator = true;
        } else if !c.is_ascii_lowercase() {
            return false;
        }
    }
    digit && separator
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_streams_in_fragments() {
        let line = r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"Git rebase "}}"#;
        let Event::Step(step) = parse(line).unwrap() else {
            panic!("not a step");
        };
        assert!(step.running());
        assert!(step.is_answer());
        assert_eq!(step.text_delta.as_deref(), Some("Git rebase "));
        assert_eq!(step.conversation_id.as_deref(), Some("c1"));
    }

    #[test]
    fn a_tool_step_carries_its_command_and_output() {
        let line = r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":4,"state":"DONE","step_type":"tool","tool_name":"run_command","duration_seconds":0.07,"tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hello"},"output":"hello\r\n"}}}"#;
        let Event::Step(step) = parse(line).unwrap() else {
            panic!("not a step");
        };
        assert!(step.is_tool());
        assert!(!step.running());
        let info = step.tool_info.expect("tool info");
        assert_eq!(info.name.as_deref(), Some("run_command"));
        assert_eq!(
            info.parameters.as_ref().unwrap()["CommandLine"],
            "echo hello"
        );
        assert_eq!(info.output.as_deref(), Some("hello\r\n"));
        assert_eq!(info.error, None);
    }

    #[test]
    fn a_failed_tool_step_reports_the_error_message() {
        let line = r#"{"event":"step_update","step_update":{"step_index":5,"state":"DONE","step_type":"tool","tool_name":"write_to_file","tool_info":{"name":"write_to_file","error":{"type":"PERMISSION_DENIED","message":"needs approval"}}}}"#;
        let Event::Step(step) = parse(line).unwrap() else {
            panic!("not a step");
        };
        let info = step.tool_info.expect("tool info");
        assert_eq!(info.error.as_deref(), Some("needs approval"));
    }

    #[test]
    fn init_names_the_conversation_and_the_model() {
        let line = r#"{"event":"init","conversation_id":"c3b6","init":{"cwd":"/w","tools":["run_command"],"permission_mode":"request-review","model":"gemini-3.1-pro"}}"#;
        let Event::Init(init) = parse(line).unwrap() else {
            panic!("not init");
        };
        assert_eq!(init.conversation_id.as_deref(), Some("c3b6"));
        assert_eq!(init.model.as_deref(), Some("gemini-3.1-pro"));
    }

    #[test]
    fn the_result_carries_the_answer_and_the_totals() {
        let line = r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"apple\n","duration_seconds":1.4,"num_turns":1,"usage":{"input_tokens":30384,"output_tokens":4,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":30388}}}"#;
        let Event::Result(result) = parse(line).unwrap() else {
            panic!("not a result");
        };
        assert_eq!(result.status, "SUCCESS");
        assert_eq!(result.response, "apple\n");
        let usage = result.usage.expect("usage");
        assert_eq!(usage.input, 30384);
        assert_eq!(usage.total, 30388);
    }

    #[test]
    fn a_failed_result_says_why() {
        let line = r#"{"event":"result","result":{"conversation_id":"","status":"ERROR","response":"","error":"invalid model selection","num_turns":0}}"#;
        let Event::Result(result) = parse(line).unwrap() else {
            panic!("not a result");
        };
        assert_eq!(result.status, "ERROR");
        assert_eq!(result.error.as_deref(), Some("invalid model selection"));
        assert!(result.usage.is_none());
    }

    #[test]
    fn a_line_that_is_not_json_is_ignored() {
        assert_eq!(
            parse("warning: ignoring unsupported stream input message"),
            None
        );
        assert_eq!(parse(r#"{"event":"future_thing"}"#), Some(Event::Other));
    }

    #[test]
    fn models_are_the_slugs_a_listing_names() {
        let listing = "Gemini 3.8 Flash (High)   gemini-3.8-flash-high\n\
                       Gemini 3.1 Pro (High)     gemini-3.1-pro-high\n\
                       Gemini 3.6 Flash (Medium) gemini-3.6-flash-medium\n\
                       gemini-3.1-pro-high\n\
                       auto\n\
                       No models are available for your account.\n";
        assert_eq!(
            parse_models(listing),
            vec![
                "gemini-3.8-flash-high",
                "gemini-3.1-pro-high",
                "gemini-3.6-flash-medium",
                "auto"
            ]
        );
    }

    #[test]
    fn a_bare_listing_works_too() {
        assert_eq!(
            parse_models("gemini-3.8-flash\ngemini-3.1-pro\n"),
            vec!["gemini-3.8-flash", "gemini-3.1-pro"]
        );
        assert!(parse_models("Please sign in to view available models.").is_empty());
    }
}
