use std::io::Write;

use openmi_core::AgentEvent;
use serde::Serialize;

use crate::output::EventRenderer;

#[derive(Debug, Clone, Serialize)]
struct ToolCallSummary {
    call_id: String,
    tool_name: String,
    ok: bool,
    output: String,
}

#[derive(Debug, Serialize)]
struct FinalJsonOutput {
    ok: bool,
    text: String,
    finish_reason: String,
    tool_calls: Vec<ToolCallSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Buffers a headless turn and emits a single structured JSON object on `finish()`.
pub struct SingleJsonRenderer<W: Write + Send> {
    out: W,
    text: String,
    finish_reason: String,
    tool_calls: Vec<ToolCallSummary>,
    error: Option<String>,
}

impl<W: Write + Send> SingleJsonRenderer<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            text: String::new(),
            finish_reason: "stop".to_string(),
            tool_calls: Vec::new(),
            error: None,
        }
    }
}

impl<W: Write + Send> EventRenderer for SingleJsonRenderer<W> {
    fn render_event(&mut self, event: &AgentEvent) -> std::io::Result<()> {
        match event {
            AgentEvent::TextDelta { text } => self.text.push_str(text),
            AgentEvent::ToolCompleted {
                call_id,
                tool_name,
                ok,
                output,
            } => {
                self.tool_calls.push(ToolCallSummary {
                    call_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    ok: *ok,
                    output: output.clone(),
                });
            }
            AgentEvent::TurnCompleted { finish_reason } => {
                self.finish_reason = finish_reason.clone();
            }
            AgentEvent::Error { code, message } => {
                self.error = Some(format!("{code}: {message}"));
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        let payload = FinalJsonOutput {
            ok: self.error.is_none(),
            text: self.text.clone(),
            finish_reason: self.finish_reason.clone(),
            tool_calls: self.tool_calls.clone(),
            error: self.error.clone(),
        };
        let encoded = serde_json::to_string_pretty(&payload)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writeln!(self.out, "{encoded}")?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_json_renderer_emits_one_final_json_document() {
        let mut buf = Vec::new();
        {
            let mut r = SingleJsonRenderer::new(&mut buf);
            r.render_event(&AgentEvent::TextDelta {
                text: "Hello ".into(),
            })
            .unwrap();
            r.render_event(&AgentEvent::TextDelta {
                text: "CI".into(),
            })
            .unwrap();
            r.render_event(&AgentEvent::TurnCompleted {
                finish_reason: "stop".into(),
            })
            .unwrap();
            r.finish().unwrap();
        }
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["text"], "Hello CI");
        assert_eq!(parsed["finish_reason"], "stop");
    }
}
