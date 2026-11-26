use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::app::{
    FullAccessModalState, ModelSelectorState, ToolStatus, TranscriptKind, TuiApp,
};
use crate::tui::slash::FULL_ACCESS_MODAL_WARNING;

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
    let mention_height = if app.mention_suggestions.is_empty() {
        0
    } else {
        (app.mention_suggestions.len() as u16 + 2).min(7)
    };

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length(mention_height),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

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
    if mention_height > 0 {
        render_mention_suggestions(frame, app, root[1]);
    }
    render_input(frame, app, root[2]);
    render_status_bar(frame, app, root[3]);

    if let Some(pending) = &app.pending_approval {
        render_approval_popup(frame, &pending.request.tool_name, &pending.request.summary);
    } else if let Some(modal) = &app.full_access_modal {
        render_full_access_modal(frame, modal);
    } else if let Some(selector) = &app.model_selector {
        render_model_selector_modal(frame, app, selector);
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

fn render_mention_suggestions(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let lines: Vec<Line<'_>> = app
        .mention_suggestions
        .iter()
        .enumerate()
        .map(|(i, path)| {
            if i == 0 {
                Line::from(Span::styled(
                    format!("> @{path}  (Tab to complete)"),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::raw(format!("  @{path}")))
            }
        })
        .collect();

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" @path completions "),
    );
    frame.render_widget(widget, area);
}

fn render_input(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let title = format!(" Prompt ({}) ", app.status_hint);
    let widget = Paragraph::new(app.input.text())
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);

    if app.pending_approval.is_none()
        && app.full_access_modal.is_none()
        && app.model_selector.is_none()
    {
        let (row, col) = app.input.cursor_row_col();
        let max_x = area.x + area.width.saturating_sub(2);
        let max_y = area.y + area.height.saturating_sub(2);
        let cx = (area.x + 1 + col).min(max_x);
        let cy = (area.y + 1 + row).min(max_y);
        frame.set_cursor_position((cx, cy));
    }
}

fn render_status_bar(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let (tier_str, credits_str, pass_str) = if let Some(cs) = &app.credit_status {
        let tier = if cs.is_paid { "PAID" } else { "FREE" };
        let cap = cs.daily_limit.unwrap_or(if cs.is_paid { 15 } else { 6 });
        let cred = if cs.wallet_credit > 0 {
            format!("{}/{}+{}w cr", cs.balance, cap, cs.wallet_credit)
        } else {
            format!("{}/{} cr", cs.balance, cap)
        };
        let pass = match cs.active_pass_remaining() {
            Some((m, mins)) => format!(" | pass:{}({mins}m)", app.display_name_for(m)),
            None => String::new(),
        };
        (tier, cred, pass)
    } else {
        let cred = match app.credits {
            Some(c) => format!("{c} cr"),
            None => "- cr".to_string(),
        };
        ("-", cred, String::new())
    };
    let cwd_display = app.cwd.file_name().and_then(|s| s.to_str()).unwrap_or(".");

    let bar = format!(
        " [{tier_str}] model:{} | {credits_str}{pass_str} | mode:{:?} | {cwd_display} ({}) ",
        app.model_display(),
        app.mode,
        app.branch
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

fn render_full_access_modal(frame: &mut Frame<'_>, modal: &FullAccessModalState) {
    let popup_area = centered_rect(76, 50, frame.area());
    frame.render_widget(Clear, popup_area);

    let mut lines: Vec<Line<'_>> = FULL_ACCESS_MODAL_WARNING
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("> {}", modal.typed),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    if let Some(err) = &modal.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::Red),
        )));
    }

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red))
                .title(" Confirm FullAccess Mode "),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, popup_area);
}

fn render_model_selector_modal(frame: &mut Frame<'_>, app: &TuiApp, selector: &ModelSelectorState) {
    let popup_area = centered_rect(82, 62, frame.area());
    frame.render_widget(Clear, popup_area);

    let active_pass = app
        .credit_status
        .as_ref()
        .and_then(|cs| cs.active_pass_remaining());
    let is_paid_user = app
        .credit_status
        .as_ref()
        .map(|cs| cs.is_paid)
        .unwrap_or(false);

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Choose a model (1-hour pass starts when you send your first prompt):",
        Style::default().fg(Color::Gray),
    )));
    lines.push(Line::from(""));

    if selector.models.is_empty() {
        lines.push(Line::from("No models available."));
    } else {
        for (idx, m) in selector.models.iter().enumerate() {
            let is_sel = idx == selector.selected;
            let prefix = if is_sel { "▸ " } else { "  " };
            let mut spans = vec![
                Span::styled(
                    format!("{prefix}{}", m.name()),
                    if is_sel {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
                Span::styled(
                    format!("  {} cr/hr", m.credit_cost),
                    Style::default().fg(Color::Yellow),
                ),
            ];
            if let Some((active_id, mins)) = active_pass {
                if active_id == m.id {
                    spans.push(Span::styled(
                        format!("  [ACTIVE · {mins}m left]"),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            }
            if m.paid_only {
                let badge_color = if is_paid_user {
                    Color::Magenta
                } else {
                    Color::Red
                };
                spans.push(Span::styled(
                    "  [PAID]",
                    Style::default()
                        .fg(badge_color)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            lines.push(Line::from(spans));
            if is_sel {
                let mut detail = format!("    id: {}  ·  {}k ctx", m.id, m.context_window / 1000);
                if let Some(desc) = m.description.as_deref().filter(|d| !d.trim().is_empty()) {
                    detail.push_str(&format!("  ·  {}", desc.trim()));
                }
                lines.push(Line::from(Span::styled(
                    detail,
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Up/Down (or j/k): navigate   Enter: select   Esc: cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Select AI Model (/model) "),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, popup_area);
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
    use openmi_core::{ApprovalRequest, ModelInfo, PermissionMode};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use tokio::sync::oneshot;

    use crate::tui::approval::PendingApproval;

    #[test]
    fn renders_cleanly_at_80x24_and_120x40_with_modals_and_mentions() {
        let mut app = TuiApp::new(
            "openmi-default".into(),
            PermissionMode::Ask,
            PathBuf::from("."),
        );
        app.push_user("hello");
        app.push_diff("--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n-old\n+new\n");
        app.mention_suggestions = vec!["src/main.rs".into()];
        app.credits = Some(6);
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

        app.pending_approval = None;
        app.full_access_modal = Some(FullAccessModalState {
            typed: "I UNDERSTAND".into(),
            error: Some("Type exact phrase".into()),
        });
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();

        app.full_access_modal = None;
        app.model_selector = Some(ModelSelectorState::new(
            vec![
                ModelInfo {
                    id: "openmi-default".into(),
                    label: "Openmi Default".into(),
                    context_window: 128_000,
                    credit_cost: 3,
                    paid_only: false,
                    display_name: Some("Openmi".into()),
                    description: Some("Balanced everyday model".into()),
                },
                ModelInfo {
                    id: "anthropic/claude-3.5-sonnet".into(),
                    label: "Claude 3.5 Sonnet".into(),
                    context_window: 200_000,
                    credit_cost: 3,
                    paid_only: true,
                    display_name: None,
                    description: None,
                },
            ],
            "openmi-default",
        ));
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
    }
}
