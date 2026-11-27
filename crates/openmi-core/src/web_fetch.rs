use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::session::CoreError;

pub const WEB_FETCH_DEFAULT_MAX_CHARS: usize = 16 * 1024;
pub const WEB_FETCH_MAX_CHARS: usize = 16 * 1024;
const WEB_FETCH_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // 2 MiB
const WEB_FETCH_TIMEOUT_SECS: u64 = 20;
const WEB_FETCH_MAX_REDIRECTS: usize = 5;

/// Returns `true` if `ip` is loopback, private, link-local, multicast, or unspecified.
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // Carrier-grade NAT 100.64.0.0/10
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0b1100_0000) == 64)
                // Benchmarking 198.18.0.0/15
                || (v4.octets()[0] == 198 && (v4.octets()[1] & 0b1111_1110) == 18)
                || v4 == Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local fc00::/7
                || (seg0 & 0xfe00) == 0xfc00
                // Link-local fe80::/10
                || (seg0 & 0xffc0) == 0xfe80
                || v6 == Ipv6Addr::LOCALHOST
        }
    }
}

/// Validate URL scheme and reject private/loopback/metadata hosts (SSRF protection).
fn validate_url_static(parsed: &reqwest::Url, allow_private: bool) -> Result<(), CoreError> {
    match parsed.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(CoreError::Tool(
                "web_fetch only supports http:// and https:// URLs".to_string(),
            ));
        }
    }
    let host = parsed
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| CoreError::Tool("web_fetch URL is missing a host".to_string()))?;

    if allow_private {
        return Ok(());
    }

    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower == "metadata.google.internal"
    {
        return Err(CoreError::Tool(format!(
            "web_fetch blocked private/local host `{host}`"
        )));
    }

    let ip_candidate = lower.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = ip_candidate.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(CoreError::Tool(format!(
                "web_fetch blocked private/loopback IP `{ip}`"
            )));
        }
    }

    Ok(())
}

/// Resolve DNS for `parsed` and ensure every resolved address is a public IP.
async fn validate_url_dns(parsed: &reqwest::Url, allow_private: bool) -> Result<(), CoreError> {
    validate_url_static(parsed, allow_private)?;
    if allow_private {
        return Ok(());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| CoreError::Tool("web_fetch URL is missing a host".to_string()))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    if let Ok(addrs) = tokio::net::lookup_host((host, port)).await {
        for sock in addrs {
            if is_blocked_ip(sock.ip()) {
                return Err(CoreError::Tool(format!(
                    "web_fetch blocked host `{host}` resolving to private/loopback IP `{}`",
                    sock.ip()
                )));
            }
        }
    }
    Ok(())
}

/// Execute the read-only `web_fetch(url, max_chars)` tool.
pub async fn exec_web_fetch(
    args: &serde_json::Value,
    cancel: &CancellationToken,
) -> Result<String, CoreError> {
    exec_web_fetch_inner(args, cancel, false).await
}

async fn exec_web_fetch_inner(
    args: &serde_json::Value,
    cancel: &CancellationToken,
    allow_private: bool,
) -> Result<String, CoreError> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::Tool("web_fetch requires `url`".to_string()))?;

    let parsed = reqwest::Url::parse(url).map_err(|_| {
        CoreError::Tool("web_fetch only supports http:// and https:// URLs".to_string())
    })?;
    validate_url_dns(&parsed, allow_private).await?;

    let max_chars = args
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(256, WEB_FETCH_MAX_CHARS))
        .unwrap_or(WEB_FETCH_DEFAULT_MAX_CHARS);

    let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= WEB_FETCH_MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        if let Err(err) = validate_url_static(attempt.url(), allow_private) {
            return attempt.error(err.to_string());
        }
        attempt.follow()
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .redirect(redirect_policy)
        .user_agent(concat!(
            "openmi-cli/",
            env!("CARGO_PKG_VERSION"),
            " (web_fetch)"
        ))
        .build()
        .map_err(|e| CoreError::Tool(format!("web_fetch client error: {e}")))?;

    let mut resp = tokio::select! {
        _ = cancel.cancelled() => return Err(CoreError::Cancelled),
        res = client.get(parsed).send() => {
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

    // Stream response chunks up to WEB_FETCH_MAX_RESPONSE_BYTES to prevent OOM on huge responses.
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            res = resp.chunk() => {
                res.map_err(|e| CoreError::Tool(format!("web_fetch body read failed: {e}")))?
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        let remaining = WEB_FETCH_MAX_RESPONSE_BYTES.saturating_sub(bytes.len());
        if remaining == 0 {
            break;
        }
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            break;
        }
        bytes.extend_from_slice(&chunk);
    }

    let raw = String::from_utf8_lossy(&bytes);

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
                | "blockquote" | "pre" | "br" | "hr" => {
                    out.push('\n');
                }
                "li" if !is_closing => {
                    out.push_str("\n- ");
                }
                "td" | "th" if !is_closing => {
                    out.push(' ');
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
    let mut current = strip_delimited(html, "<!--", "-->", "");
    for tag in ["script", "style", "noscript", "svg"] {
        let open_pat = format!("<{tag}");
        let close_pat = format!("</{tag}>");
        current = strip_delimited_ascii_ci(&current, &open_pat, &close_pat, " ");
    }
    current
}

fn strip_delimited(input: &str, open_pat: &str, close_pat: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(rel_start) = input[cursor..].find(open_pat) {
        let start = cursor + rel_start;
        out.push_str(&input[cursor..start]);
        let after_open = start + open_pat.len();
        if let Some(rel_end) = input[after_open..].find(close_pat) {
            out.push_str(replacement);
            cursor = after_open + rel_end + close_pat.len();
        } else {
            return out;
        }
    }
    out.push_str(&input[cursor..]);
    out
}

fn strip_delimited_ascii_ci(
    input: &str,
    open_pat_lower: &str,
    close_pat_lower: &str,
    replacement: &str,
) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(rel_start) = lower[cursor..].find(open_pat_lower) {
        let start = cursor + rel_start;
        out.push_str(&input[cursor..start]);
        if let Some(rel_end) = lower[start..].find(close_pat_lower) {
            out.push_str(replacement);
            cursor = start + rel_end + close_pat_lower.len();
        } else {
            return out;
        }
    }
    out.push_str(&input[cursor..]);
    out
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
                let body = format!(
                    "<html><body><h1>Title</h1><p>{}</p></body></html>",
                    "x".repeat(1000)
                );
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
        let out = exec_web_fetch_inner(&args, &CancellationToken::new(), true)
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

    #[tokio::test]
    async fn web_fetch_blocks_ssrf_private_and_metadata_targets() {
        for blocked in [
            "http://127.0.0.1:8080/admin",
            "http://localhost/secret",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.1/internal",
            "http://192.168.1.1/",
            "http://[::1]:3000/",
            "http://metadata.google.internal/computeMetadata/v1/",
        ] {
            let args = serde_json::json!({ "url": blocked });
            let err = exec_web_fetch(&args, &CancellationToken::new())
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("blocked"),
                "expected SSRF block for {blocked}, got: {err}"
            );
        }
    }
}
