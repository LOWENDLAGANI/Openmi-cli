pub mod jsonl;
pub mod text;

use openmi_core::AgentEvent;

/// Trait implemented by headless output renderers (`text` and `jsonl`).
pub trait EventRenderer: Send {
    fn render_event(&mut self, event: &AgentEvent) -> std::io::Result<()>;
    fn finish(&mut self) -> std::io::Result<()>;
}
