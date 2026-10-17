//! Records raw SDK messages to a JSON-lines file, for translation test fixtures and for replaying
//! a session through the bridge without Claude (`BLUE_BRIDGE_REPLAY`).
//!
//! Recordings are scrubbed because fixtures are committed to a public repo: the home directory
//! becomes `<HOME>`, `init` keeps only the fields Blue uses (dropping the machine's tool, MCP
//! server, plugin and agent lists), and `rate_limit_event` (account usage) is dropped.
//!
//! Besides SDK messages, a recording marks where the daemon interrupted a turn with a
//! `{"type":"blue_interrupt"}` line, so tests know the `result` that follows means "interrupted".
//! The translator ignores it like any unknown message type.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use serde_json::{Map, Value};

const INIT_FIELDS: &[&str] = &[
    "type",
    "subtype",
    "session_id",
    "uuid",
    "model",
    "cwd",
    "permissionMode",
    "claude_code_version",
];

pub const INTERRUPT_MARKER: &str = r#"{"type":"blue_interrupt"}"#;

pub struct Recorder {
    out: BufWriter<File>,
    home: Option<String>,
}

impl Recorder {
    pub fn create(path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            out: BufWriter::new(File::create(path)?),
            home: std::env::home_dir().and_then(|home| home.into_os_string().into_string().ok()),
        })
    }

    pub fn record(&mut self, json: &str) {
        if let Some(line) = scrub(json, self.home.as_deref()) {
            self.write(&line);
        }
    }

    pub fn record_interrupt(&mut self) {
        self.write(INTERRUPT_MARKER);
    }

    fn write(&mut self, line: &str) {
        if let Err(err) = writeln!(self.out, "{line}").and_then(|()| self.out.flush()) {
            tracing::warn!("recording failed: {err}");
        }
    }
}

/// Returns the scrubbed message as one JSON line, or `None` if it shouldn't be recorded.
pub fn scrub(json: &str, home: Option<&str>) -> Option<String> {
    let mut message: Value = serde_json::from_str(json).ok()?;
    match message["type"].as_str() {
        Some("rate_limit_event") => return None,
        Some("system") if message["subtype"] == "init" => {
            let kept: Map<String, Value> = message
                .as_object()?
                .iter()
                .filter(|(key, _)| INIT_FIELDS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            message = Value::Object(kept);
        }
        _ => {}
    }
    let mut line = message.to_string();
    if let Some(home) = home.filter(|home| !home.is_empty()) {
        // Paths appear raw, JSON-escaped (Windows backslashes doubled) and with forward slashes.
        let escaped = home.replace('\\', "\\\\");
        let forward = home.replace('\\', "/");
        for variant in [escaped, forward, home.to_owned()] {
            line = replace_case_insensitive(&line, &variant, "<HOME>");
        }
    }
    Some(line)
}

/// Windows paths are case-insensitive and Claude Code sometimes lowercases them.
fn replace_case_insensitive(text: &str, needle: &str, replacement: &str) -> String {
    let (lower_text, lower_needle) = (text.to_lowercase(), needle.to_lowercase());
    if lower_text.len() != text.len() {
        return text.replace(needle, replacement);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, _) in lower_text.match_indices(&lower_needle) {
        out.push_str(&text[last..start]);
        out.push_str(replacement);
        last = start + needle.len();
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_home_and_init() {
        let home = r"C:\Users\Someone";
        let init = r#"{"type":"system","subtype":"init","session_id":"s","cwd":"C:\\Users\\Someone\\code","mcp_servers":[{"name":"private"}],"plugins":["x"]}"#;
        let scrubbed = scrub(init, Some(home)).unwrap();
        assert_eq!(
            scrubbed,
            r#"{"cwd":"<HOME>\\code","session_id":"s","subtype":"init","type":"system"}"#
        );
        let text = r#"{"type":"assistant","text":"see c:/users/someone/x"}"#;
        assert_eq!(
            scrub(text, Some(home)).unwrap(),
            r#"{"text":"see <HOME>/x","type":"assistant"}"#
        );
        assert!(scrub(r#"{"type":"rate_limit_event"}"#, Some(home)).is_none());
    }
}
