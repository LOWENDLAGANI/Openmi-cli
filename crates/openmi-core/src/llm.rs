use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::AgentEvent;
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
    event_tx: &mpsc::Sender<AgentEvent>,
    cancel: &CancellationToken,
) -> Result<StepResponse, CoreError> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| CoreError::Llm(e.to_string()))?;

    let payload = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": messages,
    });

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
    if !(200..300).contains(&status) {
        let body = resp.text().await.unwrap_or_default();
        return Err(CoreError::Llm(format!("HTTP {status}: {body}")));
    }

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
