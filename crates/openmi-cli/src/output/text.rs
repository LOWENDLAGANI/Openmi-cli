use std::io::Write;

use openmi_core::AgentEvent;

use crate::output::EventRenderer;

/// Human-readable streaming text renderer for headless `openmi run`.
pub struct TextRenderer<W: Write + Send> {
    out: W,
    in_text_line: bool,
}

impl<W: Write + Send> TextRenderer<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            in_text_line: false,
        }
    }

    fn break_text_line(&mut self) -> std::io::Result<()> {
        if self.in_text_line {
            writeln!(self.out)?;
            self.in_text_line = false;
        }
        Ok(())
    }
}

impl<W: Write + Send> EventRenderer for TextRenderer<W> {
    fn render_event(&mut self, event: &AgentEvent) -> std::io::Result<()> {
        match event {
            AgentEvent::TextDelta { text } => {
                write!(self.out, "{text}")?;
                self.out.flush()?;
                self.in_text_line = !text.ends_with('\n');
            }
            AgentEvent::ToolCallRequested {
                tool_name,
                arguments_json,
                ..
            } => {
                self.break_text_line()?;
                writeln!(self.out, "[tool:request] {tool_name} {arguments_json}")?;
            }
            AgentEvent::ApprovalRequired {
                tool_name, summary, ..
            } => {
                self.break_text_line()?;
                writeln!(self.out, "[approval:required] {tool_name}: {summary}")?;
            }
            AgentEvent::ToolStarted { tool_name, .. } => {
                self.break_text_line()?;
                writeln!(self.out, "[tool:start] {tool_name}")?;
            }
            AgentEvent::ToolCompleted {
                tool_name,
                ok,
                output,
                ..
            } => {
                self.break_text_line()?;
                let status = if *ok { "ok" } else { "err" };
                let first_line = output.lines().next().unwrap_or("");
                writeln!(self.out, "[tool:{status}] {tool_name}: {first_line}")?;
            }
            AgentEvent::Error { code, message } => {
                self.break_text_line()?;
                writeln!(self.out, "[error:{code}] {message}")?;
            }
            AgentEvent::StepStarted { .. }
            | AgentEvent::ControlStateChanged { .. }
            | AgentEvent::TurnCompleted { .. } => {}
        }
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.break_text_line()?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_renderer_snapshot_formats_deltas_and_tools() {
        let mut buf = Vec::new();
        {
            let mut renderer = TextRenderer::new(&mut buf);
            renderer
                .render_event(&AgentEvent::TextDelta {
                    text: "Hello ".into(),
                })
                .unwrap();
            renderer
                .render_event(&AgentEvent::TextDelta {
                    text: "world".into(),
                })
                .unwrap();
            renderer
                .render_event(&AgentEvent::ToolStarted {
                    call_id: "c1".into(),
                    tool_name: "read_file".into(),
                })
                .unwrap();
            renderer
                .render_event(&AgentEvent::ToolCompleted {
                    call_id: "c1".into(),
                    tool_name: "read_file".into(),
                    ok: true,
                    output: "first line\nsecond line".into(),
                })
                .unwrap();
            renderer.finish().unwrap();
        }

        let rendered = String::from_utf8(buf).unwrap();
        assert_eq!(
            rendered,
            "Hello world\n[tool:start] read_file\n[tool:ok] read_file: first line\n"
        );
    }
}
