use std::io::Write;

use openmi_core::AgentEvent;
use serde::Serialize;

use crate::output::EventRenderer;

pub const JSONL_SCHEMA_VERSION: &str = "1";

#[derive(Debug, Serialize)]
struct VersionedEvent<'a> {
    schema_version: &'static str,
    event: &'a AgentEvent,
}

/// Stable, versioned JSONL renderer (one `AgentEvent` JSON object per line).
pub struct JsonlRenderer<W: Write + Send> {
    out: W,
}

impl<W: Write + Send> JsonlRenderer<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write + Send> EventRenderer for JsonlRenderer<W> {
    fn render_event(&mut self, event: &AgentEvent) -> std::io::Result<()> {
        let envelope = VersionedEvent {
            schema_version: JSONL_SCHEMA_VERSION,
            event,
        };
        let line = serde_json::to_string(&envelope)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writeln!(self.out, "{line}")?;
        self.out.flush()
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_renderer_snapshot_emits_versioned_lines() {
        let mut buf = Vec::new();
        {
            let mut renderer = JsonlRenderer::new(&mut buf);
            renderer
                .render_event(&AgentEvent::StepStarted { step: 1 })
                .unwrap();
            renderer
                .render_event(&AgentEvent::TextDelta { text: "hi".into() })
                .unwrap();
            renderer
                .render_event(&AgentEvent::TurnCompleted {
                    finish_reason: "stop".into(),
                })
                .unwrap();
            renderer.finish().unwrap();
        }

        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            r#"{"schema_version":"1","event":{"type":"step_started","step":1}}"#
        );
        assert_eq!(
            lines[1],
            r#"{"schema_version":"1","event":{"type":"text_delta","text":"hi"}}"#
        );
        assert_eq!(
            lines[2],
            r#"{"schema_version":"1","event":{"type":"turn_completed","finish_reason":"stop"}}"#
        );
    }
}
