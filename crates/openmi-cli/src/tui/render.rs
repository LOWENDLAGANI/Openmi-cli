use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::app::{ToolStatus, TranscriptKind, TuiApp};

/// Render the full TUI layout safely at any terminal size (including 80x24 and SSH).
pub fn draw(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = frame.area();
    if area.width < 10 || area.height < 6 {
        let msg = Paragraph::new("Terminal too small (min 10x6)");
        frame.render_widget(msg, area);
        return;
    }

    let input_lines = app.input.text().lines().count().clamp(1, 5) as u16;
    let input_height = (input_lines + 2).min(area.height.saturating_sub(4));

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

    // Split body into transcript + tool timeline when width >= 72, otherwise stack compactly.
    let (transcript_area, tools_area) = if root[0].width >= 72 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(root[0]);
        (cols[0], Some(cols[1]))
    } else {
        (root[0], None)
    };

    render_transcript(frame, app, transcript_area);
    if let Some(t_area) = tools_area {
        render_tool_timeline(frame, app, t_area);
    }
    render_input(frame, app, root[1]);
    render_status_bar(frame, app, root[2]);

    if let Some(pending) = &app.pending_approval {
        render_approval_popup(frame, &pending.request.tool_name, &pending.request.summary);
    }
}

fn render_transcript(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    for entry in &app.transcript {
        let (prefix, style) = match entry.kind {
            TranscriptKind::User => (
                "you> ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            TranscriptKind::Assistant => ("openmi> ", Style::default().fg(Color::White)),
            TranscriptKind::System => ("info> ", Style::default().fg(Color::DarkGray)),
            TranscriptKind::Error => (
                "err> ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            TranscriptKind::Diff => ("", Style::default().fg(Color::White)),
        };

        for (idx, line_text) in entry.text.lines().enumerate() {
            if entry.kind == TranscriptKind::Diff {
                let diff_style = if line_text.starts_with('+') && !line_text.starts_with("+++") {
                    Style::default().fg(Color::Green)
                } else if line_text.starts_with('-') && !line_text.starts_with("---") {
                    Style::default().fg(Color::Red)
                } else if line_text.starts_with("@@") {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Gray)
                };
                lines.push(Line::from(Span::styled(line_text.to_string(), diff_style)));
            } else if idx == 0 {
                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::raw(line_text.to_string()),
                ]));
            } else {
                lines.push(Line::from(Span::raw(format!("  {line_text}"))));
            }
        }
        lines.push(Line::from(""));
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(inner_height) as u16;
    let scroll_y = max_scroll.saturating_sub(app.scroll_from_bottom);

    let title = if app.is_busy() {
        let spinner = ["|", "/", "-", "\\"][(app.tick as usize) % 4];
        format!(" Transcript [{spinner}] ")
    } else {
        " Transcript ".to_string()
    };

    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((scroll_y, 0));
    frame.render_widget(widget, area);
}

fn render_tool_timeline(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    if app.tools.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no tool calls yet)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for item in app.tools.iter().rev().take(20) {
            let (badge, color) = match item.status {
                ToolStatus::Requested => ("REQ", Color::Gray),
                ToolStatus::WaitingApproval => ("ASK", Color::Yellow),
                ToolStatus::Running => ("RUN", Color::Cyan),
                ToolStatus::Succeeded => ("OK ", Color::Green),
                ToolStatus::Failed => ("ERR", Color::Red),
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{badge}] "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(item.tool_name.clone()),
            ]));
            if !item.summary.is_empty() {
                let truncated: String = item.summary.chars().take(48).collect();
                lines.push(Line::from(Span::styled(
                    format!("  {truncated}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Tools "))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn render_input(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let title = format!(" Prompt ({}) ", app.status_hint);
    let widget = Paragraph::new(app.input.text())
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);

    if app.pending_approval.is_none() {
        let (row, col) = app.input.cursor_row_col();
        let max_x = area.x + area.width.saturating_sub(2);
        let max_y = area.y + area.height.saturating_sub(2);
        let cx = (area.x + 1 + col).min(max_x);
        let cy = (area.y + 1 + row).min(max_y);
        frame.set_cursor_position((cx, cy));
    }
}

fn render_status_bar(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let credits_str = match app.credits {
        Some(c) => format!("{c} cr"),
        None => "- cr".to_string(),
    };
    let cwd_display = app.cwd.file_name().and_then(|s| s.to_str()).unwrap_or(".");

    let bar = format!(
        " model:{} | mode:{:?} | credits:{} | cwd:{} | git:{} ",
        app.model, app.mode, credits_str, cwd_display, app.branch
    );
    let widget = Paragraph::new(bar).style(Style::default().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(widget, area);
}

fn render_approval_popup(frame: &mut Frame<'_>, tool_name: &str, summary: &str) {
    let popup_area = centered_rect(70, 40, frame.area());
    frame.render_widget(Clear, popup_area);

    let lines = vec![
        Line::from(Span::styled(
            format!("Tool: {tool_name}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(summary.to_string()),
        Line::from(""),
        Line::from(Span::styled(
            "[y] Approve once   [a] Always this session   [n] Deny",
            Style::default().fg(Color::Cyan),
        )),
    ];

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Approval Required "),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(modal, popup_area);
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmi_core::{ApprovalRequest, PermissionMode};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use tokio::sync::oneshot;

    use crate::tui::approval::PendingApproval;

    #[test]
    fn renders_cleanly_at_80x24_and_120x40_with_approval_modal() {
        let mut app = TuiApp::new(
            "openmi-default".into(),
            PermissionMode::Ask,
            PathBuf::from("."),
        );
        app.push_user("hello");
        app.credits = Some(250);
        let (tx, _rx) = oneshot::channel();
        app.pending_approval = Some(PendingApproval {
            request: ApprovalRequest {
                call_id: "c1".into(),
                tool_name: "write_file".into(),
                summary: "write_file hello.txt".into(),
            },
            responder: tx,
        });

        for (w, h) in [(80, 24), (120, 40), (40, 12)] {
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();
        }
    }
}
