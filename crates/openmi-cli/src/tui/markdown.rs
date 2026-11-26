use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Color palette passed into the markdown renderer so it matches the TUI theme.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownTheme {
    pub accent: Color,
    pub code: Color,
    pub muted: Color,
    pub border: Color,
}

/// Convert markdown text (including streaming/partial markdown) into styled ratatui `Line`s.
pub fn render_markdown_lines(text: &str, indent: &str, theme: MarkdownTheme) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;

    for raw_line in text.lines() {
        let trimmed_start = raw_line.trim_start();

        // Fenced code block toggle (``` or ```lang)
        if let Some(rest) = trimmed_start.strip_prefix("```") {
            if !in_code_block {
                in_code_block = true;
                let lang = rest.trim();
                if !lang.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::styled(format!("┌─ {lang}"), Style::default().fg(theme.border)),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::styled("┌───", Style::default().fg(theme.border)),
                    ]));
                }
            } else {
                in_code_block = false;
                lines.push(Line::from(vec![
                    Span::raw(indent.to_string()),
                    Span::styled("└───", Style::default().fg(theme.border)),
                ]));
            }
            continue;
        }

        if in_code_block {
            lines.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled("│ ", Style::default().fg(theme.border)),
                Span::styled(raw_line.to_string(), Style::default().fg(theme.code)),
            ]));
            continue;
        }

        // Empty line
        if raw_line.trim().is_empty() {
            lines.push(Line::from(""));
            continue;
        }

        // Headings (#, ##, ###, ...)
        if let Some((level, heading)) = parse_heading(trimmed_start) {
            let style = if level <= 2 {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            lines.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(heading.to_string(), style),
            ]));
            continue;
        }

        // Blockquote (> text)
        if let Some(quoted) = trimmed_start.strip_prefix("> ") {
            let mut spans = vec![
                Span::raw(indent.to_string()),
                Span::styled("│ ", Style::default().fg(theme.border)),
            ];
            spans.extend(parse_inline_spans(
                quoted,
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::ITALIC),
                theme,
            ));
            lines.push(Line::from(spans));
            continue;
        }

        // Unordered bullet list (- item or * item)
        if let Some((leading_ws, item)) = parse_bullet(raw_line) {
            let mut spans = vec![
                Span::raw(format!("{indent}{leading_ws}")),
                Span::styled("• ", Style::default().fg(theme.accent)),
            ];
            spans.extend(parse_inline_spans(item, Style::default(), theme));
            lines.push(Line::from(spans));
            continue;
        }

        // Ordered list (1. item)
        if let Some((leading_ws, num, item)) = parse_numbered(raw_line) {
            let mut spans = vec![
                Span::raw(format!("{indent}{leading_ws}")),
                Span::styled(format!("{num}. "), Style::default().fg(theme.accent)),
            ];
            spans.extend(parse_inline_spans(item, Style::default(), theme));
            lines.push(Line::from(spans));
            continue;
        }

        // Normal paragraph line with inline markdown
        let mut spans = vec![Span::raw(indent.to_string())];
        spans.extend(parse_inline_spans(raw_line, Style::default(), theme));
        lines.push(Line::from(spans));
    }

    lines
}

fn parse_heading(trimmed: &str) -> Option<(usize, &str)> {
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed[hashes..].strip_prefix(' ')?;
    Some((hashes, rest.trim()))
}

fn parse_bullet(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let ws_len = line.len() - trimmed.len();
    let rest = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))?;
    Some((&line[..ws_len], rest))
}

fn parse_numbered(line: &str) -> Option<(&str, &str, &str)> {
    let trimmed = line.trim_start();
    let ws_len = line.len() - trimmed.len();
    let digits = trimmed.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || digits > 4 {
        return None;
    }
    let after_digits = &trimmed[digits..];
    let rest = after_digits.strip_prefix(". ")?;
    Some((&line[..ws_len], &trimmed[..digits], rest))
}

/// Parse inline markdown spans: `` `code` ``, `**bold**`, and `*italic*`.
/// Unclosed delimiters fall back gracefully to plain text (safe during streaming).
fn parse_inline_spans(text: &str, base: Style, theme: MarkdownTheme) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), style));
        }
    };

    while i < chars.len() {
        // Inline code: `...`
        if chars[i] == '`' {
            if let Some(end) = find_closing_char(&chars, i + 1, '`') {
                flush(&mut buf, &mut spans, base);
                let code_text: String = chars[i + 1..end].iter().collect();
                spans.push(Span::styled(code_text, Style::default().fg(theme.code)));
                i = end + 1;
                continue;
            }
        }

        // Bold: **...**
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(end) = find_closing_double_star(&chars, i + 2) {
                flush(&mut buf, &mut spans, base);
                let bold_text: String = chars[i + 2..end].iter().collect();
                spans.push(Span::styled(bold_text, base.add_modifier(Modifier::BOLD)));
                i = end + 2;
                continue;
            }
        }

        // Italic: *...* (single star, not followed by space)
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] != ' ' && chars[i + 1] != '*' {
            if let Some(end) = find_closing_char(&chars, i + 1, '*') {
                if end > i + 1 && chars[end - 1] != ' ' {
                    flush(&mut buf, &mut spans, base);
                    let italic_text: String = chars[i + 1..end].iter().collect();
                    spans.push(Span::styled(
                        italic_text,
                        base.add_modifier(Modifier::ITALIC),
                    ));
                    i = end + 1;
                    continue;
                }
            }
        }

        buf.push(chars[i]);
        i += 1;
    }

    flush(&mut buf, &mut spans, base);
    spans
}

fn find_closing_char(chars: &[char], start: usize, target: char) -> Option<usize> {
    (start..chars.len()).find(|&idx| chars[idx] == target)
}

fn find_closing_double_star(chars: &[char], start: usize) -> Option<usize> {
    let mut idx = start;
    while idx + 1 < chars.len() {
        if chars[idx] == '*' && chars[idx + 1] == '*' {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> MarkdownTheme {
        MarkdownTheme {
            accent: Color::Cyan,
            code: Color::Green,
            muted: Color::Gray,
            border: Color::DarkGray,
        }
    }

    #[test]
    fn renders_headings_bullets_inline_styles_and_code_blocks() {
        let md = "\
# Summary
Here is **bold**, *italic*, and `cargo test`.
- First item
1. Step one
```rust
fn main() {}
```";
        let lines = render_markdown_lines(md, "  ", theme());
        assert_eq!(lines.len(), 7);
        assert!(lines[0].to_string().contains("Summary"));
        assert!(!lines[0].to_string().contains('#'));
        assert!(lines[1].to_string().contains("bold"));
        assert!(!lines[1].to_string().contains("**"));
        assert!(lines[2].to_string().contains("• First item"));
        assert!(lines[3].to_string().contains("1. Step one"));
        assert!(lines[4].to_string().contains("rust"));
        assert!(lines[5].to_string().contains("fn main() {}"));
    }

    #[test]
    fn handles_unclosed_streaming_markdown_safely() {
        let partial = "Thinking about **something and `unclosed\n```rs\nlet x = 1;";
        let lines = render_markdown_lines(partial, "  ", theme());
        assert_eq!(lines.len(), 3);
        assert!(lines[0].to_string().contains("**something"));
        assert!(lines[2].to_string().contains("let x = 1;"));
    }
}
