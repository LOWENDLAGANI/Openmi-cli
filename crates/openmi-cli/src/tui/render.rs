use openmi_core::{PermissionMode, Role, TodoStatus};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::auth::store::now_unix_secs;
use crate::tui::app::{
    format_friendly_path, friendly_tool_title, AskUserModalState, FolderEntry, FolderPickerState,
    FullAccessModalState, ModelSelectorState, ResumePickerState, SettingsModalState,
    TranscriptKind, TuiApp,
};
use crate::tui::markdown::{render_markdown_lines, MarkdownTheme};
use crate::tui::slash::FULL_ACCESS_MODAL_WARNING;

// Openmi brand palette: Candy Pink (#FFB5D8), Lime Burst (#C6FF4D), Chrome Silver (#D9D9D9).
const ACCENT: Color = Color::Rgb(255, 181, 216); // Candy Pink (#FFB5D8)
const USER_COLOR: Color = Color::Rgb(198, 255, 77); // Lime Burst (#C6FF4D)
const SUCCESS: Color = Color::Rgb(198, 255, 77); // Lime Burst (#C6FF4D)
const CHROME: Color = Color::Rgb(217, 217, 217); // Chrome Silver (#D9D9D9)
const WARNING: Color = Color::Rgb(245, 194, 107); // Warm amber (Permissions, notices)
const DANGER: Color = Color::Rgb(242, 139, 130); // Soft coral (Errors, removals)
const PRO_BADGE: Color = Color::Rgb(255, 181, 216); // Candy Pink (Pro models)
const MUTED: Color = Color::Rgb(152, 152, 162); // Silver-slate (Hints, secondary text)
const BORDER_SUBTLE: Color = Color::Rgb(108, 108, 118); // Chrome-muted border / divider

/// Render the full TUI layout safely at any terminal size (including 80x24, Termux, and SSH).
pub fn draw(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = frame.area();
    if area.width < 10 || area.height < 6 {
        let msg = Paragraph::new("Terminal too small (min 10x6)").style(Style::default().fg(MUTED));
        frame.render_widget(msg, area);
        return;
    }

    let header_height: u16 = if area.height >= 10 { 2 } else { 1 };
    let activity_height: u16 = if app.is_busy() { 1 } else { 0 };
    let todo_height: u16 = if app.todos.is_empty() || area.height < 14 {
        0
    } else {
        (app.todos.len() as u16 + 2).min(6)
    };
    let popup_height: u16 = if !app.slash_suggestions.is_empty() {
        (app.slash_suggestions.len() as u16 + 2).min(8)
    } else if !app.mention_suggestions.is_empty() {
        (app.mention_suggestions.len() as u16 + 2).min(6)
    } else {
        0
    };
    let input_inner_width = area.width.saturating_sub(4).max(1);
    let wrapped_input_lines = if app.input.text().is_empty() {
        1
    } else {
        wrapped_text_rows(app.input.text(), input_inner_width).clamp(1, 5) as u16
    };
    let max_input_height = area
        .height
        .saturating_sub(header_height + activity_height + todo_height + popup_height + 3)
        .max(3);
    let input_height = (wrapped_input_lines + 2).min(max_input_height);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(2),
            Constraint::Length(activity_height),
            Constraint::Length(todo_height),
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
    if todo_height > 0 {
        render_todo_panel(frame, app, root[3]);
    }
    if popup_height > 0 {
        if !app.slash_suggestions.is_empty() {
            render_slash_suggestions(frame, app, root[4]);
        } else {
            render_mention_suggestions(frame, app, root[4]);
        }
    }
    render_input(frame, app, root[5]);
    render_footer(frame, app, root[6]);

    if let Some(pending) = &app.pending_approval {
        render_approval_popup(
            frame,
            app,
            &pending.request.tool_name,
            &pending.request.summary,
        );
    } else if let Some(ask_modal) = &app.ask_user_modal {
        render_ask_user_modal(frame, ask_modal);
    } else if let Some(modal) = &app.full_access_modal {
        render_full_access_modal(frame, modal);
    } else if let Some(selector) = &app.model_selector {
        render_model_selector_modal(frame, app, selector);
    } else if let Some(picker) = &app.folder_picker {
        render_folder_picker_modal(frame, picker);
    } else if let Some(resume) = &app.resume_picker {
        render_resume_picker_modal(frame, app, resume);
    } else if let Some(settings) = &app.settings_modal {
        render_settings_modal(frame, app, settings);
    }
}

fn render_ask_user_modal(frame: &mut Frame<'_>, modal: &AskUserModalState) {
    let area = frame.area();
    let width = area.width.saturating_sub(6).clamp(36, 72);
    let rows = modal.total_rows() as u16;
    let height = (rows + 8).min(area.height.saturating_sub(2)).max(9);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };

    let mut lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            modal.pending.request.question.as_str(),
            Style::default()
                .fg(Color::Reset)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for (idx, opt) in modal.pending.request.options.iter().enumerate() {
        let is_sel = idx == modal.selected;
        let prefix = if is_sel { "▸ " } else { "  " };
        let style = if is_sel {
            Style::default().fg(USER_COLOR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(CHROME)
        };
        lines.push(Line::from(Span::styled(format!("{prefix}{opt}"), style)));
    }

    if modal.pending.request.allow_custom {
        let is_sel = modal.is_custom_row();
        let prefix = if is_sel { "▸ " } else { "  " };
        let display = if modal.custom_input.is_empty() {
            "Other (type custom answer...)".to_string()
        } else {
            format!("Other: {}_", modal.custom_input)
        };
        let style = if is_sel {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{display}"),
            style,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ Select  ·  Enter Confirm  ·  Esc Dismiss",
        Style::default().fg(MUTED),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " Question from Openmi ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::uniform(1));

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        popup,
    );
}

fn render_todo_panel(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let completed = app
        .todos
        .iter()
        .filter(|t| t.status == TodoStatus::Completed)
        .count();
    let title = format!(" Tasks ({completed}/{}) ", app.todos.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER_SUBTLE))
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::horizontal(1));

    let max_rows = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line<'_>> = Vec::new();
    for item in app.todos.iter().take(max_rows) {
        let (icon, style) = match item.status {
            TodoStatus::Completed => ("✓ ", Style::default().fg(SUCCESS)),
            TodoStatus::InProgress => (
                "◐ ",
                Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
            ),
            TodoStatus::Pending => ("○ ", Style::default().fg(MUTED)),
        };
        let text_style = match item.status {
            TodoStatus::Completed => Style::default().fg(MUTED),
            TodoStatus::InProgress => Style::default().fg(Color::Reset),
            TodoStatus::Pending => Style::default().fg(CHROME),
        };
        lines.push(Line::from(vec![
            Span::styled(icon, style),
            Span::styled(item.content.as_str(), text_style),
        ]));
    }

    let widget = Paragraph::new(lines).block(block);
    frame.render_widget(widget, area);
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let narrow = area.width < 60;

    let mut right_spans: Vec<Span<'_>> = Vec::new();
    let mut right_len: usize = 0;
    if let Some(cs) = &app.credit_status {
        if let Some((_, mins)) = cs.active_pass_remaining() {
            let s = if narrow {
                format!("{mins}m · ")
            } else {
                format!("{mins}m pass · ")
            };
            right_len += s.chars().count();
            right_spans.push(Span::styled(s, Style::default().fg(SUCCESS)));
        }
        let total = cs.balance + cs.wallet_credit;
        let s = if narrow {
            format!("{total} cr ")
        } else {
            let label = if total == 1 { "credit" } else { "credits" };
            format!("{total} {label} ")
        };
        right_len += s.chars().count();
        right_spans.push(Span::styled(s, Style::default().fg(MUTED)));
    } else if let Some(c) = app.credits {
        let s = if narrow {
            format!("{c} cr ")
        } else {
            let label = if c == 1 { "credit" } else { "credits" };
            format!("{c} {label} ")
        };
        right_len += s.chars().count();
        right_spans.push(Span::styled(s, Style::default().fg(MUTED)));
    }

    let right_width = (right_len as u16).min(area.width / 2);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(right_width)])
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
    ];
    if !narrow {
        left_spans.push(Span::styled(" · ", Style::default().fg(MUTED)));
        left_spans.push(Span::styled(
            app.short_cwd(),
            Style::default().fg(USER_COLOR),
        ));
    }

    // Show PLAN badge when Plan Mode is active, or permission mode when changed from default Trusted.
    if app.plan_mode {
        left_spans.push(Span::styled(
            "  PLAN",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    } else {
        match app.mode {
            PermissionMode::Trusted => {}
            PermissionMode::Ask => {
                left_spans.push(Span::styled("  Ask", Style::default().fg(WARNING)));
            }
            PermissionMode::ReadOnly => {
                left_spans.push(Span::styled("  Read-only", Style::default().fg(MUTED)));
            }
            PermissionMode::FullAccess => {
                left_spans.push(Span::styled(
                    "  Full access",
                    Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
                ));
            }
        }
    }

    let left = Paragraph::new(Line::from(left_spans));
    frame.render_widget(left, cols[0]);

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
                    Style::default().fg(USER_COLOR).add_modifier(Modifier::BOLD),
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

    let h_pad = if area.width < 60 { 1 } else { 2 };
    let inner_width = area.width.saturating_sub(h_pad * 2).max(1);
    let inner_height = area.height.saturating_sub(1) as usize;
    let total_visual_lines = wrapped_lines_total(&lines, inner_width);
    let max_scroll = total_visual_lines.saturating_sub(inner_height) as u16;
    let scroll_y = max_scroll.saturating_sub(app.scroll_from_bottom);

    let paragraph = Paragraph::new(lines)
        .block(Block::default().padding(Padding::new(h_pad, h_pad, 1, 0)))
        .wrap(Wrap { trim: false })
        .scroll((scroll_y, 0));
    frame.render_widget(paragraph, area);
}

/// Count visual rows produced when wrapping `lines` into `width` columns (matching `Wrap { trim: false }`).
fn wrapped_lines_total(lines: &[Line<'_>], width: u16) -> usize {
    lines
        .iter()
        .map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            wrapped_single_line_rows(&text, width)
        })
        .sum()
}

/// Count visual rows for a multi-line string wrapped into `width` columns.
fn wrapped_text_rows(text: &str, width: u16) -> usize {
    if text.is_empty() {
        return 1;
    }
    text.split('\n')
        .map(|line| wrapped_single_line_rows(line, width))
        .sum()
}

/// Count visual rows for a single logical line (no `\n`) wrapped into `width` columns.
fn wrapped_single_line_rows(line: &str, width: u16) -> usize {
    let w = width.max(1) as usize;
    if line.is_empty() {
        return 1;
    }
    let mut rows: usize = 1;
    let mut col: usize = 0;

    // Split into alternating whitespace / non-whitespace runs so word-wrapping matches ratatui.
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let is_ws = chars[i].is_whitespace();
        let start = i;
        while i < chars.len() && chars[i].is_whitespace() == is_ws {
            i += 1;
        }
        let seg_len = i - start;

        if is_ws {
            if col + seg_len <= w {
                col += seg_len;
            } else {
                // Trailing spaces wrap to next line only if more content follows.
                if i < chars.len() {
                    rows += 1;
                    col = 0;
                }
            }
        } else if col + seg_len <= w {
            col += seg_len;
        } else if seg_len <= w {
            if col > 0 {
                rows += 1;
            }
            col = seg_len;
        } else {
            // Very long unbroken token: wrap across multiple rows.
            let mut rem = seg_len;
            if col > 0 {
                let take = w.saturating_sub(col);
                rem = rem.saturating_sub(take);
                rows += 1;
            }
            while rem > w {
                rem -= w;
                rows += 1;
            }
            col = rem;
        }
    }
    rows
}

fn render_welcome(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let pink = Style::default().fg(ACCENT);
    let lime = Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD);
    let eye = Style::default().fg(Color::Rgb(20, 20, 23)).bg(ACCENT);
    let chrome = Style::default().fg(CHROME);
    let muted = Style::default().fg(MUTED);

    let plan_label = match app.credit_status.as_ref() {
        Some(cs) if cs.is_paid => "Openmi Paid",
        Some(_) => "Openmi Free",
        None => "Openmi CLI",
    };

    let version = env!("CARGO_PKG_VERSION");
    let compact = area.height < 14 || area.width < 46;

    let mut lines: Vec<Line<'_>> = Vec::new();
    if !compact {
        lines.push(Line::from(""));
    }

    // Openmi pixel mascot (Candy Pink body + Lime Burst sparks)
    lines.push(Line::from(vec![
        Span::styled("✦", lime),
        Span::raw("   "),
        Span::styled("✦", lime),
    ]));
    lines.push(Line::from(Span::styled("█████████", pink)));
    lines.push(Line::from(vec![
        Span::styled("████", pink),
        Span::styled("█", eye),
        Span::styled("███", pink),
        Span::styled("█", eye),
        Span::styled("████", pink),
    ]));
    lines.push(Line::from(Span::styled("█████████", pink)));
    lines.push(Line::from(Span::styled("█ █ █ █", pink)));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(app.model_display(), chrome.add_modifier(Modifier::BOLD)),
        Span::styled(" · ", muted),
        Span::styled(plan_label, lime),
    ]));
    lines.push(Line::from(Span::styled(app.friendly_cwd(), muted)));

    if !compact {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("/plan", pink),
            Span::styled(" · ", muted),
            Span::styled("/compact", pink),
            Span::styled(" · ", muted),
            Span::styled("/model", pink),
            Span::styled(" · ", muted),
            Span::styled("/folder", pink),
            Span::styled(" · ", muted),
            Span::styled("/help", pink),
        ]));
    }

    let box_width = area.width.saturating_sub(4).clamp(28, 66);
    let box_height = (lines.len() as u16 + 2).min(area.height.saturating_sub(1).max(6));
    let x = area.x + area.width.saturating_sub(box_width) / 2;
    let y = area.y + area.height.saturating_sub(box_height) / 2;
    let card_area = Rect {
        x,
        y,
        width: box_width.min(area.width),
        height: box_height.min(area.height),
    };

    let title = format!(" Openmi v{version} ");
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .padding(Padding::new(2, 2, 0, 0));

    let widget = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, card_area);
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
            let item = &app.slash_suggestions[i];
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
                    item.description.as_str(),
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
                Line::from(Span::styled(
                    format!("  @{path}"),
                    Style::default().fg(MUTED),
                ))
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

    let inner_width = area.width.saturating_sub(4).max(1);
    let visible_rows = area.height.saturating_sub(2).max(1);
    let (vis_row, vis_col) =
        wrapped_cursor_pos(app.input.text(), app.input.cursor_byte(), inner_width);
    let input_scroll_y = vis_row.saturating_sub(visible_rows.saturating_sub(1));

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
        .wrap(Wrap { trim: false })
        .scroll((input_scroll_y, 0));
    frame.render_widget(widget, area);

    if app.pending_approval.is_none()
        && app.full_access_modal.is_none()
        && app.model_selector.is_none()
        && app.folder_picker.is_none()
        && app.resume_picker.is_none()
        && app.settings_modal.is_none()
    {
        let max_x = area.x + area.width.saturating_sub(2);
        let max_y = area.y + area.height.saturating_sub(2);
        let cx = (area.x + 2 + vis_col).min(max_x);
        let cy = (area.y + 1 + vis_row.saturating_sub(input_scroll_y)).min(max_y);
        frame.set_cursor_position((cx, cy));
    }
}

/// Compute the wrapped visual `(row, col)` of `cursor_byte` inside a box of width `inner_width`.
fn wrapped_cursor_pos(text: &str, cursor_byte: usize, inner_width: u16) -> (u16, u16) {
    let w = inner_width.max(1) as usize;
    let clamped = cursor_byte.min(text.len());
    let prefix = &text[..clamped];
    let mut row: usize = 0;
    let mut col: usize = 0;
    for ch in prefix.chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
            if col >= w {
                row += 1;
                col = 0;
            }
        }
    }
    (row as u16, col as u16)
}

fn render_footer(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let line = if app.armed_quit {
        Line::from(Span::styled(
            " Press Esc again to quit",
            Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
        ))
    } else if app.scroll_from_bottom > 0 {
        Line::from(Span::styled(
            " ↑ Scrolled up  ·  PageDown to return",
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
            Span::styled(" /", Style::default().fg(ACCENT)),
            Span::styled(" commands  ·  ", Style::default().fg(MUTED)),
            Span::styled("Shift+Tab", Style::default().fg(ACCENT)),
            Span::styled(" mode  ·  ", Style::default().fg(MUTED)),
            Span::styled("/folder", Style::default().fg(ACCENT)),
            Span::styled(" project  ·  ", Style::default().fg(MUTED)),
            Span::styled("Esc ×2", Style::default().fg(ACCENT)),
            Span::styled(" quit", Style::default().fg(MUTED)),
        ])
    };

    frame.render_widget(Paragraph::new(line), area);
}

fn render_approval_popup(frame: &mut Frame<'_>, app: &TuiApp, tool_name: &str, summary: &str) {
    let preview = app.approval_preview_diff();
    let height_pct = if preview.is_some() { 68 } else { 44 };
    let popup_area = centered_rect(78, height_pct, frame.area());
    frame.render_widget(Clear, popup_area);

    let action_title = friendly_tool_title(tool_name);
    let mut lines = vec![
        Line::from(Span::styled(
            format!("Openmi wants to: {action_title}"),
            Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            summary.to_string(),
            Style::default().fg(MUTED),
        )),
        Line::from(""),
    ];

    if let Some(diff_txt) = preview {
        lines.push(Line::from(Span::styled(
            "Preview of changes:",
            Style::default().fg(MUTED),
        )));
        for dline in diff_txt.lines() {
            let style = if dline.starts_with('+') {
                Style::default().fg(SUCCESS)
            } else if dline.starts_with('-') {
                Style::default().fg(DANGER)
            } else {
                Style::default().fg(MUTED)
            };
            lines.push(Line::from(Span::styled(format!("  {dline}"), style)));
        }
        lines.push(Line::from(""));
    }

    let choices = [
        ("[y]", "Allow once", SUCCESS),
        ("[a]", "Always allow this session", ACCENT),
        ("[n]", "Cancel", DANGER),
    ];
    for (idx, (key_lbl, text_lbl, color)) in choices.iter().enumerate() {
        let is_sel = idx == app.approval_selected;
        let prefix = if is_sel { "▸ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{prefix}{key_lbl} "),
                Style::default().fg(*color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                *text_lbl,
                if is_sel {
                    Style::default()
                        .fg(Color::Reset)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(MUTED)
                },
            ),
        ]));
    }

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(WARNING))
                .padding(Padding::uniform(1))
                .title(Span::styled(
                    " Permission needed (↑/↓ · Enter · y/a/n) ",
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
        Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
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
            Style::default().fg(USER_COLOR).add_modifier(Modifier::BOLD),
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
            format!(
                "  ({} folders — use ↑/↓ to scroll)",
                total.saturating_sub(2)
            ),
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

fn render_resume_picker_modal(frame: &mut Frame<'_>, app: &TuiApp, picker: &ResumePickerState) {
    let popup_area = centered_rect(82, 62, frame.area());
    frame.render_widget(Clear, popup_area);

    let now = now_unix_secs();
    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Pick a previous conversation to continue:",
        Style::default().fg(MUTED),
    )));
    lines.push(Line::from(""));

    if picker.sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No saved chats yet. Your chats are saved automatically as you talk.",
            Style::default().fg(MUTED),
        )));
    } else {
        let max_visible = (popup_area.height.saturating_sub(8) / 2).max(2) as usize;
        let total = picker.sessions.len();
        let start = if picker.selected >= max_visible {
            (picker.selected + 1).saturating_sub(max_visible)
        } else {
            0
        };
        let end = (start + max_visible).min(total);

        for idx in start..end {
            let sess = &picker.sessions[idx];
            let is_sel = idx == picker.selected;
            let prefix = if is_sel { "▸ " } else { "  " };
            let preview_raw = sess
                .messages
                .iter()
                .find(|m| m.role == Role::User)
                .map(|m| m.content.lines().next().unwrap_or("").trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("(empty chat)");
            let preview: String = preview_raw.chars().take(52).collect();
            let ellipsis = if preview_raw.chars().count() > 52 {
                "…"
            } else {
                ""
            };
            let age = format_relative_age(now, sess.updated_at);
            let folder_note = if sess.cwd == app.cwd {
                "this folder".to_string()
            } else {
                format_friendly_path(&sess.cwd)
            };

            lines.push(Line::from(vec![Span::styled(
                format!("{prefix}{preview}{ellipsis}"),
                if is_sel {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Reset)
                },
            )]));
            lines.push(Line::from(Span::styled(
                format!(
                    "    {age}  ·  {} msgs  ·  {folder_note}",
                    sess.messages.len()
                ),
                Style::default().fg(MUTED),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("↑/↓", Style::default().fg(ACCENT)),
        Span::styled(" Move  ·  ", Style::default().fg(MUTED)),
        Span::styled("Enter", Style::default().fg(ACCENT)),
        Span::styled(" Resume  ·  ", Style::default().fg(MUTED)),
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
                    " Resume a Previous Chat ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, popup_area);
}

fn format_relative_age(now: u64, then: u64) -> String {
    let diff = now.saturating_sub(then);
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86_400)
    }
}

fn render_settings_modal(frame: &mut Frame<'_>, app: &TuiApp, modal: &SettingsModalState) {
    let popup_area = centered_rect(78, 54, frame.area());
    frame.render_widget(Clear, popup_area);

    let items = [
        (
            "Auto-approve edits & commands",
            app.settings.auto_approve && !app.settings.read_only,
            "Run file edits and commands automatically without asking",
        ),
        (
            "Read-only mode",
            app.settings.read_only,
            "Only read files; block all file edits and shell commands",
        ),
        (
            "Show tool activity in chat",
            app.settings.show_tool_activity,
            "Show short inline lines like '✓ Read main.rs' in the chat",
        ),
    ];

    let mut lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            "Preferences are saved automatically:",
            Style::default().fg(MUTED),
        )),
        Line::from(""),
    ];

    for (idx, (label, enabled, desc)) in items.iter().enumerate() {
        let is_sel = idx == modal.selected;
        let prefix = if is_sel { "▸ " } else { "  " };
        let (badge, badge_color) = if *enabled {
            ("[ ON ]", SUCCESS)
        } else {
            ("[ OFF ]", MUTED)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{prefix}{label:<34} "),
                if is_sel {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Reset)
                },
            ),
            Span::styled(
                badge,
                Style::default()
                    .fg(badge_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        if is_sel {
            lines.push(Line::from(Span::styled(
                format!("    {desc}"),
                Style::default().fg(MUTED),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("↑/↓", Style::default().fg(ACCENT)),
        Span::styled(" Move  ·  ", Style::default().fg(MUTED)),
        Span::styled("Enter / Space", Style::default().fg(ACCENT)),
        Span::styled(" Toggle  ·  ", Style::default().fg(MUTED)),
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
                    " Settings ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, popup_area);
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let px = if r.width < 60 {
        percent_x.clamp(94, 100)
    } else {
        percent_x.min(100)
    };
    let py = if r.height < 20 {
        percent_y.clamp(88, 100)
    } else {
        percent_y.min(100)
    };

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - py) / 2),
            Constraint::Percentage(py),
            Constraint::Percentage((100 - py) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - px) / 2),
            Constraint::Percentage(px),
            Constraint::Percentage((100 - px) / 2),
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

        app.folder_picker = None;
        app.settings_modal = Some(SettingsModalState::default());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
    }

    #[test]
    fn long_wrapped_output_and_prompt_stay_visible_on_narrow_screen() {
        let mut app = TuiApp::new(
            "openmi-default".into(),
            PermissionMode::Trusted,
            PathBuf::from("."),
        );
        // Long multi-paragraph assistant reply that wraps heavily at 42 cols.
        let mut long_reply = String::new();
        for i in 1..=15 {
            long_reply.push_str(&format!(
                "Paragraph {i}: This is a very long sentence designed to wrap across multiple lines on a phone screen.\n\n"
            ));
        }
        long_reply.push_str("FINAL_VISIBLE_MARKER");
        app.transcript.push(crate::tui::app::TranscriptEntry {
            kind: TranscriptKind::Assistant,
            text: long_reply,
        });

        // Long single-line prompt that wraps across > 5 rows at 42 cols.
        let long_prompt = format!("{}TAIL_PROMPT_OK", "word ".repeat(60));
        for ch in long_prompt.chars() {
            app.input.insert_char(ch);
        }

        let backend = TestBackend::new(42, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            rendered.contains("FINAL_VISIBLE_MARKER"),
            "Expected bottom of long wrapped output to be visible, got:\n{rendered}"
        );
        assert!(
            rendered.contains("TAIL_PROMPT_OK"),
            "Expected tail of long wrapped prompt to be visible, got:\n{rendered}"
        );
    }
}
