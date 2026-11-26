use openmi_core::PermissionMode;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::app::{
    format_friendly_path, friendly_tool_title, FolderEntry, FolderPickerState,
    FullAccessModalState, ModelSelectorState, TranscriptKind, TuiApp,
};
use crate::tui::markdown::{render_markdown_lines, MarkdownTheme};
use crate::tui::slash::FULL_ACCESS_MODAL_WARNING;

// Soft, beginner-friendly color palette (readable on dark and light terminals).
const ACCENT: Color = Color::Rgb(110, 195, 224); // Soft sky teal (Openmi, focus)
const USER_COLOR: Color = Color::Rgb(134, 219, 178); // Soft mint (You)
const SUCCESS: Color = Color::Rgb(126, 214, 150); // Gentle green (Active, additions)
const WARNING: Color = Color::Rgb(245, 194, 107); // Warm amber (Permissions, notices)
const DANGER: Color = Color::Rgb(242, 139, 130); // Soft coral (Errors, removals)
const PRO_BADGE: Color = Color::Rgb(196, 167, 231); // Soft lavender (Pro models)
const MUTED: Color = Color::Rgb(138, 148, 166); // Calm slate (Hints, secondary text)
const BORDER_SUBTLE: Color = Color::Rgb(78, 86, 102); // Quiet border / divider

/// Render the full TUI layout safely at any terminal size (including 80x24, Termux, and SSH).
pub fn draw(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = frame.area();
    if area.width < 10 || area.height < 6 {
        let msg = Paragraph::new("Terminal too small (min 10x6)")
            .style(Style::default().fg(MUTED));
        frame.render_widget(msg, area);
        return;
    }

    let header_height: u16 = if area.height >= 10 { 2 } else { 1 };
    let activity_height: u16 = if app.is_busy() { 1 } else { 0 };
    let popup_height: u16 = if !app.slash_suggestions.is_empty() {
        (app.slash_suggestions.len() as u16 + 2).min(8)
    } else if !app.mention_suggestions.is_empty() {
        (app.mention_suggestions.len() as u16 + 2).min(6)
    } else {
        0
    };
    let input_lines = app.input.text().lines().count().clamp(1, 5) as u16;
    let max_input_height = area
        .height
        .saturating_sub(header_height + activity_height + popup_height + 3)
        .max(3);
    let input_height = (input_lines + 2).min(max_input_height);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(2),
            Constraint::Length(activity_height),
            Constraint::Length(popup_height),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, app, root[0]);
    render_chat(frame, app, root[1]);
    if activity_height > 0 {
        render_activity_line(frame, app, root[2]);
    }
    if popup_height > 0 {
        if !app.slash_suggestions.is_empty() {
            render_slash_suggestions(frame, app, root[3]);
        } else {
            render_mention_suggestions(frame, app, root[3]);
        }
    }
    render_input(frame, app, root[4]);
    render_footer(frame, app, root[5]);

    if let Some(pending) = &app.pending_approval {
        render_approval_popup(frame, &pending.request.tool_name, &pending.request.summary);
    } else if let Some(modal) = &app.full_access_modal {
        render_full_access_modal(frame, modal);
    } else if let Some(selector) = &app.model_selector {
        render_model_selector_modal(frame, app, selector);
    } else if let Some(picker) = &app.folder_picker {
        render_folder_picker_modal(frame, picker);
    }
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        });

    let mut left_spans = vec![
        Span::styled(
            " ✦ Openmi ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("· ", Style::default().fg(MUTED)),
        Span::styled(app.model_display(), Style::default().fg(Color::Reset)),
        Span::styled("  ·  ", Style::default().fg(MUTED)),
        Span::styled(app.short_cwd(), Style::default().fg(USER_COLOR)),
    ];

    // Only show permission mode if the user changed it away from the default Ask mode.
    match app.mode {
        PermissionMode::Ask => {}
        PermissionMode::ReadOnly => {
            left_spans.push(Span::styled("  Read-only", Style::default().fg(MUTED)));
        }
        PermissionMode::Trusted => {
            left_spans.push(Span::styled("  Trusted", Style::default().fg(WARNING)));
        }
        PermissionMode::FullAccess => {
            left_spans.push(Span::styled(
                "  Full access",
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            ));
        }
    }

    let left = Paragraph::new(Line::from(left_spans));
    frame.render_widget(left, cols[0]);

    let mut right_spans: Vec<Span<'_>> = Vec::new();
    if let Some(cs) = &app.credit_status {
        if let Some((_, mins)) = cs.active_pass_remaining() {
            right_spans.push(Span::styled(
                format!("{mins}m pass  ·  "),
                Style::default().fg(SUCCESS),
            ));
        }
        let total = cs.balance + cs.wallet_credit;
        let label = if total == 1 { "credit" } else { "credits" };
        right_spans.push(Span::styled(
            format!("{total} {label} "),
            Style::default().fg(MUTED),
        ));
    } else if let Some(c) = app.credits {
        let label = if c == 1 { "credit" } else { "credits" };
        right_spans.push(Span::styled(
            format!("{c} {label} "),
            Style::default().fg(MUTED),
        ));
    }

    let right = Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right);
    frame.render_widget(right, cols[1]);

    if area.height >= 2 {
        let rule = "─".repeat(area.width as usize);
        let divider = Paragraph::new(rule).style(Style::default().fg(BORDER_SUBTLE));
        frame.render_widget(
            divider,
            Rect {
                x: area.x,
                y: area.y + 1,
                width: area.width,
                height: 1,
            },
        );
    }
}

fn render_chat(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    if app.transcript.is_empty() {
        render_welcome(frame, app, area);
        return;
    }

    let md_theme = MarkdownTheme {
        accent: ACCENT,
        code: USER_COLOR,
        muted: MUTED,
        border: BORDER_SUBTLE,
    };

    let mut lines: Vec<Line<'_>> = Vec::new();
    for (idx_entry, entry) in app.transcript.iter().enumerate() {
        let next_is_tool = app
            .transcript
            .get(idx_entry + 1)
            .map(|e| matches!(e.kind, TranscriptKind::ToolOk | TranscriptKind::ToolErr))
            .unwrap_or(false);

        match entry.kind {
            TranscriptKind::User => {
                lines.push(Line::from(Span::styled(
                    "You",
                    Style::default()
                        .fg(USER_COLOR)
                        .add_modifier(Modifier::BOLD),
                )));
                for line_text in entry.text.lines() {
                    lines.push(Line::from(Span::raw(format!("  {line_text}"))));
                }
            }
            TranscriptKind::Assistant => {
                lines.push(Line::from(Span::styled(
                    "Openmi",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )));
                lines.extend(render_markdown_lines(&entry.text, "  ", md_theme));
            }
            TranscriptKind::ToolOk => {
                lines.push(Line::from(vec![
                    Span::styled("  ✓ ", Style::default().fg(SUCCESS)),
                    Span::styled(entry.text.clone(), Style::default().fg(MUTED)),
                ]));
            }
            TranscriptKind::ToolErr => {
                lines.push(Line::from(vec![
                    Span::styled("  ✗ ", Style::default().fg(DANGER)),
                    Span::styled(entry.text.clone(), Style::default().fg(DANGER)),
                ]));
            }
            TranscriptKind::System => {
                for (idx, line_text) in entry.text.lines().enumerate() {
                    let prefix = if idx == 0 { "• " } else { "  " };
                    lines.push(Line::from(Span::styled(
                        format!("{prefix}{line_text}"),
                        Style::default().fg(MUTED),
                    )));
                }
            }
            TranscriptKind::Error => {
                for (idx, line_text) in entry.text.lines().enumerate() {
                    let prefix = if idx == 0 { "! " } else { "  " };
                    lines.push(Line::from(Span::styled(
                        format!("{prefix}{line_text}"),
                        Style::default().fg(DANGER),
                    )));
                }
            }
            TranscriptKind::Diff => {
                for line_text in entry.text.lines() {
                    let diff_style = if line_text.starts_with('+') && !line_text.starts_with("+++")
                    {
                        Style::default().fg(SUCCESS)
                    } else if line_text.starts_with('-') && !line_text.starts_with("---") {
                        Style::default().fg(DANGER)
                    } else if line_text.starts_with("@@") {
                        Style::default().fg(ACCENT)
                    } else {
                        Style::default().fg(MUTED)
                    };
                    lines.push(Line::from(Span::styled(line_text.to_string(), diff_style)));
                }
            }
        }
        let is_tool = matches!(entry.kind, TranscriptKind::ToolOk | TranscriptKind::ToolErr);
        if !(is_tool && next_is_tool) {
            lines.push(Line::from(""));
        }
    }

    let inner_height = area.height.saturating_sub(1) as usize;
    let total_lines = lines.len();
    let max_scroll = total_lines.saturating_sub(inner_height) as u16;
    let scroll_y = max_scroll.saturating_sub(app.scroll_from_bottom);

    let widget = Paragraph::new(lines)
        .block(Block::default().padding(Padding::new(2, 2, 1, 0)))
        .wrap(Wrap { trim: false })
        .scroll((scroll_y, 0));
    frame.render_widget(widget, area);
}

fn render_welcome(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "✦ Welcome to Openmi",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Project folder: ", Style::default().fg(MUTED)),
            Span::styled(app.friendly_cwd(), Style::default().fg(USER_COLOR)),
        ]),
        Line::from(""),
        Line::from(Span::raw(
            "Type what you need help with below and press Enter.",
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  /folder  ", Style::default().fg(ACCENT)),
            Span::styled("Pick or change project folder", Style::default().fg(MUTED)),
        ]),
        Line::from(vec![
            Span::styled("  /model   ", Style::default().fg(ACCENT)),
            Span::styled("Switch AI model", Style::default().fg(MUTED)),
        ]),
        Line::from(vec![
            Span::styled("  /help    ", Style::default().fg(ACCENT)),
            Span::styled("See tips & commands", Style::default().fg(MUTED)),
        ]),
        Line::from(vec![
            Span::styled("  /quit    ", Style::default().fg(ACCENT)),
            Span::styled("Close Openmi", Style::default().fg(MUTED)),
        ]),
    ];

    let widget = Paragraph::new(lines)
        .block(Block::default().padding(Padding::new(2, 2, 1, 0)))
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

fn render_activity_line(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let spinner = frames[(app.tick as usize) % frames.len()];
    let label = app
        .active_activity_label()
        .unwrap_or_else(|| "Thinking...".to_string());

    let line = Line::from(vec![
        Span::styled(format!("  {spinner} "), Style::default().fg(ACCENT)),
        Span::styled(label, Style::default().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_slash_suggestions(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let inner_rows = area.height.saturating_sub(2).max(1) as usize;
    let total = app.slash_suggestions.len();
    let start = if app.slash_selected >= inner_rows {
        (app.slash_selected + 1).saturating_sub(inner_rows)
    } else {
        0
    };
    let end = (start + inner_rows).min(total);

    let lines: Vec<Line<'_>> = (start..end)
        .map(|i| {
            let item = app.slash_suggestions[i];
            let is_sel = i == app.slash_selected;
            let prefix = if is_sel { "▸ " } else { "  " };
            Line::from(vec![
                Span::styled(
                    format!("{prefix}{:<10}", item.command),
                    if is_sel {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Reset)
                    },
                ),
                Span::styled(
                    item.description,
                    if is_sel {
                        Style::default().fg(Color::Reset)
                    } else {
                        Style::default().fg(MUTED)
                    },
                ),
            ])
        })
        .collect();

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(BORDER_SUBTLE))
            .padding(Padding::horizontal(1))
            .title(Span::styled(
                " Commands (↑/↓ · Enter to run) ",
                Style::default().fg(MUTED),
            )),
    );
    frame.render_widget(widget, area);
}

fn render_mention_suggestions(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let lines: Vec<Line<'_>> = app
        .mention_suggestions
        .iter()
        .enumerate()
        .map(|(i, path)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(
                        format!("▸ @{path}"),
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  (Tab to pick)", Style::default().fg(MUTED)),
                ])
            } else {
                Line::from(Span::styled(format!("  @{path}"), Style::default().fg(MUTED)))
            }
        })
        .collect();

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(BORDER_SUBTLE))
            .padding(Padding::horizontal(1))
            .title(Span::styled(" Files ", Style::default().fg(MUTED))),
    );
    frame.render_widget(widget, area);
}

fn render_input(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let is_empty = app.input.text().is_empty();
    let border_color = if app.is_busy() { BORDER_SUBTLE } else { ACCENT };
    let title_text = if app.is_busy() {
        " Working... "
    } else {
        " Message "
    };

    let content = if is_empty && !app.is_busy() {
        Paragraph::new(Span::styled(
            "Type a message and press Enter...",
            Style::default().fg(MUTED),
        ))
    } else {
        Paragraph::new(app.input.text())
    };

    let widget = content
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border_color))
                .padding(Padding::horizontal(1))
                .title(Span::styled(
                    title_text,
                    Style::default().fg(if app.is_busy() { MUTED } else { ACCENT }),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);

    if app.pending_approval.is_none()
        && app.full_access_modal.is_none()
        && app.model_selector.is_none()
        && app.folder_picker.is_none()
    {
        let (row, col) = app.input.cursor_row_col();
        let max_x = area.x + area.width.saturating_sub(2);
        let max_y = area.y + area.height.saturating_sub(2);
        let cx = (area.x + 2 + col).min(max_x);
        let cy = (area.y + 1 + row).min(max_y);
        frame.set_cursor_position((cx, cy));
    }
}

fn render_footer(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let line = if app.armed_quit {
        Line::from(Span::styled(
            " Press Esc again to quit",
            Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
        ))
    } else if app.scroll_from_bottom > 0 {
        Line::from(Span::styled(
            " ↑ Scrolled up  ·  PageDown to return to bottom",
            Style::default().fg(ACCENT),
        ))
    } else if app.is_busy() {
        Line::from(Span::styled(
            " Esc to stop  ·  Esc twice to quit",
            Style::default().fg(MUTED),
        ))
    } else if !app.status_hint.is_empty() {
        Line::from(Span::styled(
            format!(" {}", app.status_hint),
            Style::default().fg(MUTED),
        ))
    } else {
        Line::from(vec![
            Span::styled(" Enter", Style::default().fg(ACCENT)),
            Span::styled(" send  ·  ", Style::default().fg(MUTED)),
            Span::styled("/folder", Style::default().fg(ACCENT)),
            Span::styled(" project  ·  ", Style::default().fg(MUTED)),
            Span::styled("/model", Style::default().fg(ACCENT)),
            Span::styled(" AI  ·  ", Style::default().fg(MUTED)),
            Span::styled("/help", Style::default().fg(ACCENT)),
            Span::styled(" tips  ·  ", Style::default().fg(MUTED)),
            Span::styled("Esc ×2", Style::default().fg(ACCENT)),
            Span::styled(" quit", Style::default().fg(MUTED)),
        ])
    };

    frame.render_widget(Paragraph::new(line), area);
}

fn render_approval_popup(frame: &mut Frame<'_>, tool_name: &str, summary: &str) {
    let popup_area = centered_rect(72, 42, frame.area());
    frame.render_widget(Clear, popup_area);

    let action_title = friendly_tool_title(tool_name);
    let lines = vec![
        Line::from(Span::styled(
            format!("Openmi wants to: {action_title}"),
            Style::default()
                .fg(WARNING)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::raw(summary.to_string())),
        Line::from(""),
        Line::from(vec![
            Span::styled("[y]", Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)),
            Span::styled(" Allow once   ", Style::default().fg(Color::Reset)),
            Span::styled("[a]", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" Always allow   ", Style::default().fg(Color::Reset)),
            Span::styled("[n]", Style::default().fg(DANGER).add_modifier(Modifier::BOLD)),
            Span::styled(" Cancel", Style::default().fg(Color::Reset)),
        ]),
    ];

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(WARNING))
                .padding(Padding::uniform(1))
                .title(Span::styled(
                    " Permission needed ",
                    Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(modal, popup_area);
}

fn render_full_access_modal(frame: &mut Frame<'_>, modal: &FullAccessModalState) {
    let popup_area = centered_rect(76, 52, frame.area());
    frame.render_widget(Clear, popup_area);

    let mut lines: Vec<Line<'_>> = FULL_ACCESS_MODAL_WARNING
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("> {}", modal.typed),
        Style::default()
            .fg(WARNING)
            .add_modifier(Modifier::BOLD),
    )));
    if let Some(err) = &modal.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(DANGER),
        )));
    }

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(DANGER))
                .padding(Padding::uniform(1))
                .title(Span::styled(
                    " Confirm Full Access ",
                    Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, popup_area);
}

fn render_model_selector_modal(frame: &mut Frame<'_>, app: &TuiApp, selector: &ModelSelectorState) {
    let popup_area = centered_rect(78, 60, frame.area());
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
        "Pick an AI model (1-hour pass starts on your first message):",
        Style::default().fg(MUTED),
    )));
    lines.push(Line::from(""));

    if selector.models.is_empty() {
        lines.push(Line::from("No models available."));
    } else {
        for (idx, m) in selector.models.iter().enumerate() {
            let is_sel = idx == selector.selected;
            let prefix = if is_sel { "▸ " } else { "  " };
            let credit_label = if m.credit_cost == 1 {
                "1 credit/hr".to_string()
            } else {
                format!("{} credits/hr", m.credit_cost)
            };
            let mut spans = vec![
                Span::styled(
                    format!("{prefix}{}", m.name()),
                    if is_sel {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Reset)
                    },
                ),
                Span::styled(format!("  ·  {credit_label}"), Style::default().fg(MUTED)),
            ];
            if let Some((active_id, mins)) = active_pass {
                if active_id == m.id {
                    spans.push(Span::styled(
                        format!("  Active ({mins}m left)"),
                        Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD),
                    ));
                }
            }
            if m.paid_only {
                let badge_color = if is_paid_user { PRO_BADGE } else { WARNING };
                spans.push(Span::styled(
                    "  [Pro]",
                    Style::default()
                        .fg(badge_color)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            lines.push(Line::from(spans));
            if is_sel {
                if let Some(desc) = m.description.as_deref().filter(|d| !d.trim().is_empty()) {
                    lines.push(Line::from(Span::styled(
                        format!("    {}", desc.trim()),
                        Style::default().fg(MUTED),
                    )));
                }
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("↑/↓", Style::default().fg(ACCENT)),
        Span::styled(" Move  ·  ", Style::default().fg(MUTED)),
        Span::styled("Enter", Style::default().fg(ACCENT)),
        Span::styled(" Select  ·  ", Style::default().fg(MUTED)),
        Span::styled("Esc", Style::default().fg(ACCENT)),
        Span::styled(" Close", Style::default().fg(MUTED)),
    ]));

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(ACCENT))
                .padding(Padding::uniform(1))
                .title(Span::styled(
                    " Choose AI Model ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, popup_area);
}

fn render_folder_picker_modal(frame: &mut Frame<'_>, picker: &FolderPickerState) {
    let popup_area = centered_rect(80, 66, frame.area());
    frame.render_widget(Clear, popup_area);

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Current: ", Style::default().fg(MUTED)),
        Span::styled(
            format_friendly_path(&picker.current_dir),
            Style::default()
                .fg(USER_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    if let Some(err) = &picker.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(DANGER),
        )));
    }

    // Keep the highlighted entry visible even in folders with many subfolders.
    let inner_rows = popup_area.height.saturating_sub(8) as usize;
    let max_visible = inner_rows.max(3);
    let total = picker.entries.len();
    let start = if picker.selected >= max_visible {
        (picker.selected + 1).saturating_sub(max_visible)
    } else {
        0
    };
    let end = (start + max_visible).min(total);

    for idx in start..end {
        let is_sel = idx == picker.selected;
        let prefix = if is_sel { "▸ " } else { "  " };
        let line = match &picker.entries[idx] {
            FolderEntry::UseCurrent => Line::from(Span::styled(
                format!("{prefix}✓ Use this folder"),
                if is_sel {
                    Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(SUCCESS)
                },
            )),
            FolderEntry::Parent(_) => Line::from(Span::styled(
                format!("{prefix}↩ ..  (go up a folder)"),
                if is_sel {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(MUTED)
                },
            )),
            FolderEntry::Child { name, .. } => Line::from(Span::styled(
                format!("{prefix}{name}/"),
                if is_sel {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Reset)
                },
            )),
        };
        lines.push(line);
    }

    if total > max_visible {
        lines.push(Line::from(Span::styled(
            format!("  ({} folders — use ↑/↓ to scroll)", total.saturating_sub(2)),
            Style::default().fg(MUTED),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("↑/↓", Style::default().fg(ACCENT)),
        Span::styled(" Move  ·  ", Style::default().fg(MUTED)),
        Span::styled("Enter", Style::default().fg(ACCENT)),
        Span::styled(" Open/Use  ·  ", Style::default().fg(MUTED)),
        Span::styled("Space", Style::default().fg(ACCENT)),
        Span::styled(" Pick  ·  ", Style::default().fg(MUTED)),
        Span::styled("Backspace", Style::default().fg(ACCENT)),
        Span::styled(" Up  ·  ", Style::default().fg(MUTED)),
        Span::styled("Esc", Style::default().fg(ACCENT)),
        Span::styled(" Close", Style::default().fg(MUTED)),
    ]));

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(ACCENT))
                .padding(Padding::uniform(1))
                .title(Span::styled(
                    " Choose Project Folder ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
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
    use openmi_core::{ApprovalRequest, ControlState, ModelInfo, PermissionMode};
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

        // Render empty welcome state first.
        for (w, h) in [(80, 24), (120, 40), (40, 12)] {
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();
        }

        // Render active conversation + busy spinner + mentions + approval popup.
        app.push_user("hello");
        app.push_diff("--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n-old\n+new\n");
        app.control_state = ControlState::Running;
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

        app.model_selector = None;
        app.folder_picker = Some(FolderPickerState::new(PathBuf::from(".")));
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
    }
}
