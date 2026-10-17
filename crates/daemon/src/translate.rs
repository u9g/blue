//! Turns raw Agent SDK messages (JSON, as forwarded by the bridge) into Blue session events.
//!
//! The SDK owns these shapes and they change between releases, so parsing is lenient: fields we
//! don't use are ignored, and unknown message types produce no events.

use blue_protocol::events::v1::{
    AssistantText, SessionEvent, SessionStarted, ToolFinished, ToolStarted, TurnCompleted,
    TurnFailed, session_event::Kind,
};
use serde_json::Value;

const SUMMARY_MAX_CHARS: usize = 100;

#[derive(Default)]
pub struct Translator {
    claude_session_id: Option<String>,
    cwd: String,
}

impl Translator {
    pub fn claude_session_id(&self) -> Option<&str> {
        self.claude_session_id.as_deref()
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Translates one SDK message. `turn_id` is the turn in progress, if any.
    pub fn translate(&mut self, json: &str, turn_id: Option<&str>) -> Vec<SessionEvent> {
        let Ok(message) = serde_json::from_str::<Value>(json) else {
            tracing::warn!("bridge sent an SDK message that isn't JSON");
            return Vec::new();
        };
        let kinds = match str_field(&message, "type") {
            "system" if str_field(&message, "subtype") == "init" => self.init(&message),
            // Messages from subagents carry the Task tool call they belong to; only the main
            // conversation is reported.
            "assistant" if message["parent_tool_use_id"].is_null() => self.assistant(&message),
            "user" if message["parent_tool_use_id"].is_null() => tool_results(&message),
            "result" => self.result(&message, turn_id),
            _ => Vec::new(),
        };
        kinds
            .into_iter()
            .map(|kind| SessionEvent { kind: Some(kind) })
            .collect()
    }

    fn init(&mut self, message: &Value) -> Vec<Kind> {
        // Claude Code repeats init at the start of every turn; report it when it's new.
        let session_id = str_field(message, "session_id");
        if self.claude_session_id.as_deref() == Some(session_id) {
            return Vec::new();
        }
        self.claude_session_id = Some(session_id.to_owned());
        self.cwd = str_field(message, "cwd").to_owned();
        vec![Kind::SessionStarted(SessionStarted {
            claude_session_id: session_id.to_owned(),
            model: str_field(message, "model").to_owned(),
            cwd: self.cwd.clone(),
        })]
    }

    fn assistant(&self, message: &Value) -> Vec<Kind> {
        content_blocks(message)
            .filter_map(|block| match str_field(block, "type") {
                "text" => {
                    let text = str_field(block, "text").trim();
                    (!text.is_empty()).then(|| {
                        Kind::AssistantText(AssistantText {
                            text: text.to_owned(),
                        })
                    })
                }
                "tool_use" => {
                    let tool = str_field(block, "name");
                    Some(Kind::ToolStarted(ToolStarted {
                        tool_use_id: str_field(block, "id").to_owned(),
                        tool: tool.to_owned(),
                        summary: tool_summary(tool, &block["input"], &self.cwd),
                    }))
                }
                _ => None,
            })
            .collect()
    }

    fn result(&mut self, message: &Value, turn_id: Option<&str>) -> Vec<Kind> {
        let Some(turn_id) = turn_id.map(str::to_owned) else {
            tracing::warn!("SDK result arrived with no turn in progress");
            return Vec::new();
        };
        if str_field(message, "subtype") == "success" && message["is_error"] != true {
            return vec![Kind::TurnCompleted(TurnCompleted {
                turn_id,
                result_text: str_field(message, "result").to_owned(),
                duration_ms: message["duration_ms"].as_u64().unwrap_or_default(),
                num_turns: message["num_turns"].as_u64().unwrap_or_default() as u32,
            })];
        }
        let errors: Vec<&str> = message["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let reason = if !errors.is_empty() {
            errors.join("; ")
        } else if !str_field(message, "result").is_empty() {
            str_field(message, "result").to_owned()
        } else {
            str_field(message, "subtype").to_owned()
        };
        vec![Kind::TurnFailed(TurnFailed { turn_id, reason })]
    }
}

fn tool_results(message: &Value) -> Vec<Kind> {
    content_blocks(message)
        .filter(|block| str_field(block, "type") == "tool_result")
        .map(|block| {
            Kind::ToolFinished(ToolFinished {
                tool_use_id: str_field(block, "tool_use_id").to_owned(),
                is_error: block["is_error"] == true,
            })
        })
        .collect()
}

fn content_blocks(message: &Value) -> impl Iterator<Item = &Value> {
    message["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

/// A short, speakable description of a tool call, e.g. `Edit src/main.rs` or `Bash: Run tests`.
fn tool_summary(tool: &str, input: &Value, cwd: &str) -> String {
    let path = |key: &str| relative_to(str_field(input, key), cwd);
    let summary = match tool {
        "Read" | "Write" | "Edit" | "MultiEdit" => format!("{tool} {}", path("file_path")),
        "NotebookEdit" => format!("{tool} {}", path("notebook_path")),
        "Glob" | "Grep" => format!("{tool} {}", str_field(input, "pattern")),
        "Bash" => match str_field(input, "description") {
            "" => format!("Bash: {}", first_line(str_field(input, "command"))),
            description => format!("Bash: {description}"),
        },
        "WebFetch" => format!("WebFetch {}", str_field(input, "url")),
        "WebSearch" => format!("WebSearch {}", str_field(input, "query")),
        "Task" | "Agent" => format!("{tool}: {}", str_field(input, "description")),
        "TodoWrite" => "Update todo list".to_owned(),
        _ => match tool
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
        {
            Some((server, name)) => format!("{name} ({server})"),
            None => tool.to_owned(),
        },
    };
    truncate(summary.trim(), SUMMARY_MAX_CHARS)
}

fn relative_to<'a>(path: &'a str, cwd: &str) -> &'a str {
    if cwd.is_empty() {
        return path;
    }
    path.strip_prefix(cwd)
        .map(|rest| rest.trim_start_matches(['/', '\\']))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn truncate(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kinds(translator: &mut Translator, message: Value, turn: Option<&str>) -> Vec<Kind> {
        translator
            .translate(&message.to_string(), turn)
            .into_iter()
            .filter_map(|event| event.kind)
            .collect()
    }

    #[test]
    fn init_is_reported_once_per_claude_session() {
        let mut t = Translator::default();
        let init = json!({"type": "system", "subtype": "init", "session_id": "abc", "model": "m", "cwd": "/w"});
        assert_eq!(kinds(&mut t, init.clone(), None).len(), 1);
        assert!(kinds(&mut t, init, None).is_empty());
        assert_eq!(t.claude_session_id(), Some("abc"));
    }

    #[test]
    fn unsuccessful_result_fails_the_turn() {
        let mut t = Translator::default();
        let result = json!({"type": "result", "subtype": "error_max_turns", "is_error": true,
            "errors": ["hit max turns"]});
        assert!(matches!(
            &kinds(&mut t, result, Some("t1"))[..],
            [Kind::TurnFailed(failed)] if failed.reason == "hit max turns"
        ));
    }

    #[test]
    fn subagent_messages_are_skipped() {
        let mut t = Translator::default();
        let message = json!({"type": "assistant", "parent_tool_use_id": "toolu_1",
            "message": {"content": [{"type": "text", "text": "inner"}]}});
        assert!(kinds(&mut t, message, Some("t1")).is_empty());
    }

    #[test]
    fn summaries() {
        let cwd = "C:\\work\\repo";
        let s = |tool: &str, input: Value| tool_summary(tool, &input, cwd);
        assert_eq!(
            s("Edit", json!({"file_path": "C:\\work\\repo\\src\\main.rs"})),
            "Edit src\\main.rs"
        );
        assert_eq!(
            s("Read", json!({"file_path": "/etc/hosts"})),
            "Read /etc/hosts"
        );
        assert_eq!(
            s(
                "Bash",
                json!({"command": "cargo test\necho", "description": ""})
            ),
            "Bash: cargo test"
        );
        assert_eq!(
            s("Bash", json!({"command": "x", "description": "Run tests"})),
            "Bash: Run tests"
        );
        assert_eq!(
            s("mcp__blue__set_project_directory", json!({})),
            "set_project_directory (blue)"
        );
        assert_eq!(
            s("Bash", json!({"command": "y".repeat(200)}))
                .chars()
                .count(),
            101
        );
    }
}
