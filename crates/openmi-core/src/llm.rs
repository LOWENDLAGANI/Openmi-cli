use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::AgentEvent;
use crate::registry::DynamicToolSpec;
use crate::session::{CoreError, Message};

/// Configuration for the managed Openmi Supabase LLM proxy.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub endpoint: String,
    pub bearer_token: String,
}

/// A tool invocation requested by the model during an SSE stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments_json: String,
}

/// Result of a single streaming LLM step.
#[derive(Debug, Clone)]
pub struct StepResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SseChunk {
    TextDelta {
        text: String,
    },
    ToolCall {
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    Done {
        #[serde(default = "default_stop")]
        finish_reason: String,
    },
    Error {
        #[serde(default = "default_err_code")]
        code: String,
        message: String,
    },
}

fn default_stop() -> String {
    "stop".to_string()
}

fn default_err_code() -> String {
    "llm_error".to_string()
}

/// Stream a single model step over Server-Sent Events (`text/event-stream`).
pub async fn stream_step(
    config: &ProxyConfig,
    model: &str,
    messages: &[Message],
    extra_tools: &[DynamicToolSpec],
    allowed_tools: Option<&[String]>,
    event_tx: &mpsc::Sender<AgentEvent>,
    cancel: &CancellationToken,
) -> Result<StepResponse, CoreError> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| CoreError::Llm(e.to_string()))?;

    let wire_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            if m.attachments.is_empty() {
                serde_json::json!({
                    "role": m.role,
                    "content": m.content,
                })
            } else {
                let mut parts = vec![serde_json::json!({
                    "type": "text",
                    "text": m.content,
                })];
                for att in &m.attachments {
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{};base64,{}", att.mime_type, att.data_base64),
                        }
                    }));
                }
                serde_json::json!({
                    "role": m.role,
                    "content": parts,
                })
            }
        })
        .collect();

    let mut payload = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": wire_messages,
    });
    if let Some(allowed) = allowed_tools {
        payload["allowed_tools"] = serde_json::json!(allowed);
    }
    if !extra_tools.is_empty() {
        let tools_json: Vec<serde_json::Value> = extra_tools
            .iter()
            .map(|spec| {
                let params = if spec.input_schema.is_object() {
                    spec.input_schema.clone()
                } else {
                    serde_json::json!({ "type": "object", "properties": {} })
                };
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": spec.name,
                        "description": spec.description,
                        "parameters": params,
                    }
                })
            })
            .collect();
        payload["tools"] = serde_json::Value::Array(tools_json);
    }

    const MAX_RATE_LIMIT_RETRIES: u8 = 3;
    let mut attempt: u8 = 0;

    let resp = loop {
        let send_fut = client
            .post(&config.endpoint)
            .bearer_auth(&config.bearer_token)
            .header("Accept", "text/event-stream")
            .json(&payload)
            .send();

        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            res = send_fut => res.map_err(|e| CoreError::Llm(e.to_string()))?,
        };

        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(CoreError::Unauthorized);
        }
        if status == 402 {
            return Err(CoreError::OutOfCredits);
        }
        if status == 429 {
            let retry_after_hdr = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            let body = resp.text().await.unwrap_or_default();
            let parsed_json = serde_json::from_str::<serde_json::Value>(&body).ok();
            let err_code = parsed_json
                .as_ref()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()))
                .unwrap_or("rate_limited");
            let err_msg = parsed_json
                .as_ref()
                .and_then(|v| v.get("message").and_then(|m| m.as_str()))
                .map(str::to_string)
                .unwrap_or_else(|| format!("HTTP 429: {body}"));
            let retry_after_body = parsed_json
                .as_ref()
                .and_then(|v| v.get("retry_after").and_then(|r| r.as_u64()));

            let is_transient = matches!(
                err_code,
                "rate_limited_burst"
                    | "rate_limited_rpm"
                    | "rate_limited_tpm"
                    | "concurrency_limit"
                    | "rate_limited_ip"
                    | "global_rate_limited"
            );

            if is_transient && attempt < MAX_RATE_LIMIT_RETRIES {
                attempt += 1;
                let wait_secs = retry_after_hdr.or(retry_after_body).unwrap_or(3).clamp(1, 15);
                tokio::select! {
                    _ = cancel.cancelled() => return Err(CoreError::Cancelled),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(wait_secs)) => {}
                }
                continue;
            }
            return Err(CoreError::Llm(err_msg));
        }
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(msg) = parsed.get("message").and_then(|m| m.as_str()) {
                    return Err(CoreError::Llm(format!("HTTP {status}: {msg}")));
                }
            }
            return Err(CoreError::Llm(format!("HTTP {status}: {body}")));
        }
        break resp;
    };

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut finish_reason = "stop".to_string();

    loop {
        let next_chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            item = stream.next() => item,
        };

        let Some(chunk_res) = next_chunk else {
            break;
        };

        let bytes = chunk_res.map_err(|e| CoreError::Llm(e.to_string()))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(newline_idx) = buffer.find('\n') {
            let line = buffer[..newline_idx].trim_end_matches('\r').to_string();
            buffer.drain(..=newline_idx);

            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }

            let parsed: SseChunk = serde_json::from_str(data)
                .map_err(|e| CoreError::Llm(format!("invalid SSE JSON `{data}`: {e}")))?;

            match parsed {
                SseChunk::TextDelta { text: delta } => {
                    text.push_str(&delta);
                    let _ = event_tx.send(AgentEvent::TextDelta { text: delta }).await;
                }
                SseChunk::ToolCall {
                    call_id,
                    tool_name,
                    arguments,
                } => {
                    let arguments_json = arguments.to_string();
                    let _ = event_tx
                        .send(AgentEvent::ToolCallRequested {
                            call_id: call_id.clone(),
                            tool_name: tool_name.clone(),
                            arguments_json: arguments_json.clone(),
                        })
                        .await;
                    tool_calls.push(ToolCall {
                        call_id,
                        tool_name,
                        arguments_json,
                    });
                }
                SseChunk::Done {
                    finish_reason: reason,
                } => {
                    finish_reason = reason;
                }
                SseChunk::Error { code, message } => {
                    let _ = event_tx
                        .send(AgentEvent::Error {
                            code: code.clone(),
                            message: message.clone(),
                        })
                        .await;
                    return Err(CoreError::Llm(format!("{code}: {message}")));
                }
            }
        }
    }

    Ok(StepResponse {
        text,
        tool_calls,
        finish_reason,
    })
}
