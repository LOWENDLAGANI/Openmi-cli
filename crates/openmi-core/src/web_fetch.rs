use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::session::CoreError;

pub const WEB_FETCH_DEFAULT_MAX_CHARS: usize = 16 * 1024;
pub const WEB_FETCH_MAX_CHARS: usize = 16 * 1024;
const WEB_FETCH_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // 2 MiB
const WEB_FETCH_TIMEOUT_SECS: u64 = 20;

/// Execute the read-only `web_fetch(url, max_chars)` tool.
pub async fn exec_web_fetch(
    args: &serde_json::Value,
    cancel: &CancellationToken,
) -> Result<String, CoreError> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::Tool("web_fetch requires `url`".to_string()))?;

    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(CoreError::Tool(
            "web_fetch only supports http:// and https:// URLs".to_string(),
        ));
    }

    let max_chars = args
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(256, WEB_FETCH_MAX_CHARS))
        .unwrap_or(WEB_FETCH_DEFAULT_MAX_CHARS);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .user_agent("openmi-cli/0.1 (web_fetch)")
        .build()
        .map_err(|e| CoreError::Tool(format!("web_fetch client error: {e}")))?;

    let resp = tokio::select! {
        _ = cancel.cancelled() => return Err(CoreError::Cancelled),
        res = client.get(url).send() => {
            res.map_err(|e| CoreError::Tool(format!("web_fetch request failed: {e}")))?
        }
    };

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if !status.is_success() {
        return Err(CoreError::Tool(format!(
            "web_fetch HTTP {} for {}",
            status.as_u16(),
            url
        )));
    }

    let bytes = tokio::select! {
        _ = cancel.cancelled() => return Err(CoreError::Cancelled),
        res = resp.bytes() => {
            res.map_err(|e| CoreError::Tool(format!("web_fetch body read failed: {e}")))?
        }
    };

    let slice = if bytes.len() > WEB_FETCH_MAX_RESPONSE_BYTES {
        &bytes[..WEB_FETCH_MAX_RESPONSE_BYTES]
    } else {
        &bytes[..]
    };
    let raw = String::from_utf8_lossy(slice);

    let cleaned = if content_type.contains("html") || looks_like_html(&raw) {
        html_to_text(&raw)
    } else {
        raw.into_owned()
    };

    let total_chars = cleaned.chars().count();
    if total_chars > max_chars {
        let truncated: String = cleaned.chars().take(max_chars).collect();
        Ok(format!(
            "{truncated}\n\n... [truncated {} chars from {}]",
            total_chars - max_chars,
            url
        ))
    } else {
        Ok(cleaned)
    }
}

fn looks_like_html(s: &str) -> bool {
    let head: String = s.chars().take(512).collect();
    let lower = head.to_ascii_lowercase();
    lower.contains("<!doctype html") || lower.contains("<html") || lower.contains("<body")
}

/// Lightweight HTML-to-Markdown/text stripper: removes `<script>`, `<style>`, `<noscript>`,
/// comments, and HTML tags while preserving line breaks, headings, and list items.
pub fn html_to_text(html: &str) -> String {
    let stripped_blocks = strip_ignored_blocks(html);
    let mut out = String::with_capacity(stripped_blocks.len() / 2);
    let mut chars = stripped_blocks.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' {
            let mut tag = String::new();
            for tc in chars.by_ref() {
                if tc == '>' {
                    break;
                }
                if tag.len() < 64 {
                    tag.push(tc);
                }
            }
            let tag_lower = tag.trim().to_ascii_lowercase();
            let tag_name = tag_lower
                .trim_start_matches('/')
                .split(|ch: char| ch.is_whitespace() || ch == '/')
                .next()
                .unwrap_or("");
            let is_closing = tag_lower.starts_with('/');

            match tag_name {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    if !is_closing {
                        out.push_str("\n\n# ");
                    } else {
                        out.push('\n');
                    }
                }
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "tr"
                | "blockquote" | "pre" => {
                    out.push('\n');
                }
                "br" | "hr" => {
                    out.push('\n');
                }
                "li" => {
                    if !is_closing {
                        out.push_str("\n- ");
                    }
                }
                "td" | "th" => {
                    if !is_closing {
                        out.push(' ');
                    }
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }

    let decoded = decode_basic_entities(&out);
    normalize_whitespace(&decoded)
}

fn strip_ignored_blocks(html: &str) -> String {
    let mut s = html.to_string();
    // Strip HTML comments
    while let Some(start) = s.find("<!--") {
        if let Some(end_rel) = s[start + 4..].find("-->") {
            s.replace_range(start..start + 4 + end_rel + 3, "");
        } else {
            s.truncate(start);
            break;
        }
    }
    // Strip <script>, <style>, <noscript>, <svg>
    for tag in ["script", "style", "noscript", "svg"] {
        let open_pat = format!("<{tag}");
        let close_pat = format!("</{tag}>");
        loop {
            let lower = s.to_ascii_lowercase();
            let Some(start) = lower.find(&open_pat) else {
                break;
            };
            if let Some(end_rel) = lower[start..].find(&close_pat) {
                s.replace_range(start..start + end_rel + close_pat.len(), " ");
            } else {
                s.truncate(start);
                break;
            }
        }
    }
    s
}

fn decode_basic_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn normalize_whitespace(s: &str) -> String {
    let mut lines = Vec::new();
    let mut blank_run = 0usize;
    for raw_line in s.lines() {
        let compact = raw_line.split_whitespace().collect::<Vec<_>>().join(" ");
        if compact.is_empty() {
            blank_run += 1;
            if blank_run <= 1 && !lines.is_empty() {
                lines.push(String::new());
            }
        } else {
            blank_run = 0;
            lines.push(compact);
        }
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn html_to_text_strips_scripts_styles_and_preserves_structure() {
        let html = r#"
        <!DOCTYPE html>
        <html>
          <head>
            <style>body { color: red; }</style>
            <script>console.log("secret");</script>
          </head>
          <body>
            <!-- hidden comment -->
            <h1>Hello &amp; Welcome</h1>
            <p>First paragraph with <b>bold</b> text.</p>
            <ul><li>Item 1</li><li>Item 2</li></ul>
          </body>
        </html>
        "#;
        let text = html_to_text(html);
        assert!(text.contains("# Hello & Welcome"));
        assert!(text.contains("First paragraph with bold text."));
        assert!(text.contains("- Item 1"));
        assert!(text.contains("- Item 2"));
        assert!(!text.contains("console.log"));
        assert!(!text.contains("color: red"));
        assert!(!text.contains("hidden comment"));
    }

    #[tokio::test]
    async fn web_fetch_fetches_http_and_caps_output() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let body = format!("<html><body><h1>Title</h1><p>{}</p></body></html>", "x".repeat(1000));
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        let args = serde_json::json!({
            "url": format!("http://{addr}/docs"),
            "max_chars": 300
        });
        let out = exec_web_fetch(&args, &CancellationToken::new())
            .await
            .unwrap();
        assert!(out.contains("# Title"));
        assert!(out.contains("[truncated "));
    }

    #[tokio::test]
    async fn web_fetch_rejects_non_http_scheme() {
        let args = serde_json::json!({ "url": "file:///etc/passwd" });
        let err = exec_web_fetch(&args, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("http:// and https://"));
    }
}
