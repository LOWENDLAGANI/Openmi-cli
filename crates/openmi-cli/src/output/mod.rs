pub mod json;
pub mod jsonl;
pub mod text;

use std::str::FromStr;

use openmi_core::AgentEvent;

/// Headless output format (`text`, `json`, or `stream-json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    StreamJson,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "stream-json" | "stream_json" | "jsonl" => Ok(Self::StreamJson),
            other => Err(format!(
                "invalid --output-format `{other}` (expected: text, json, stream-json)"
            )),
        }
    }
}

/// Trait implemented by headless output renderers (`text`, `json`, and `stream-json`).
pub trait EventRenderer: Send {
    fn render_event(&mut self, event: &AgentEvent) -> std::io::Result<()>;
    fn finish(&mut self) -> std::io::Result<()>;
}
