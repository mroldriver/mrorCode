//! Anthropic Messages API 流式 provider（M5-b）。
//!
//! 与 OpenAI 兼容的关键差异：
//! - `system` 是顶层字符串字段（从首条 System 消息提取）；
//! - 消息 content 是 ContentBlock 数组（text / tool_use / tool_result）；
//! - 工具定义为 `{name, description, input_schema}`（非 function 包裹）；
//! - SSE 事件类型为 `message_start / content_block_start / content_block_delta /
//!   content_block_stop / message_delta / message_stop`。
//!
//! 复用手写 SSE 解码器（见 [`super::sse`]）。

use std::pin::Pin;
use std::time::Duration;

use async_stream::stream;
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use mrorcode_kernel::message::{Message, Role};
use mrorcode_kernel::provider::{
    ChatOptions, LlmProvider, ProviderError, ProviderResult, StreamEvent,
};
use mrorcode_kernel::tool::ToolDef;

use super::retry::{is_retryable_status, parse_retry_after, RetryPolicy};
use super::sse::SseDecoder;

/// 配置。
#[derive(Clone, Debug)]
pub struct AnthropicConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub context_window: u32,
    pub timeout: Duration,
    /// Anthropic 必填 max_tokens，默认 4096。
    pub max_tokens: u32,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".to_string(),
            api_key: String::new(),
            model: "claude-sonnet-4-5".to_string(),
            context_window: 200_000,
            timeout: Duration::from_secs(300),
            max_tokens: 4096,
        }
    }
}

/// Anthropic provider。
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    context_window: u32,
    max_tokens: u32,
    retry: RetryPolicy,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent(concat!("mrorcode/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key,
            model: config.model,
            context_window: config.context_window,
            max_tokens: config.max_tokens,
            retry: RetryPolicy::default(),
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn context_window(&self) -> u32 {
        self.context_window
    }

    fn chat_stream<'a>(
        &'a self,
        messages: &'a [Message],
        tools: &'a [ToolDef],
        options: &'a ChatOptions,
        cancel: &'a CancellationToken,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = ProviderResult<BoxStream<'a, StreamEvent>>>
                + Send
                + 'a,
        >,
    > {
        let model = options.model.clone().unwrap_or_else(|| self.model.clone());
        let body = WireRequest::from_parts(&model, self.max_tokens, messages, tools, options);
        Box::pin(async move { self.start(body, cancel).await })
    }
}

impl AnthropicProvider {
    async fn start<'a>(
        &'a self,
        body: WireRequest,
        cancel: &'a CancellationToken,
    ) -> ProviderResult<BoxStream<'a, StreamEvent>> {
        let mut attempts = 0u32;
        let response = loop {
            let req_fut = self
                .client
                .post(self.endpoint())
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body);

            let send_result = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return Err(ProviderError::new("request cancelled"));
                }
                r = req_fut.send() => r,
            };

            match send_result {
                Ok(resp) if resp.status().is_success() => break resp,
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after = parse_retry_after(
                        resp.headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|v| v.to_str().ok()),
                    );
                    let text = resp.text().await.unwrap_or_default();
                    if self.retry.can_retry(attempts) && is_retryable_status(status) {
                        attempts += 1;
                        let delay = self.retry.backoff(attempts, retry_after);
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                return Err(ProviderError::new("request cancelled"));
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                        continue;
                    }
                    return Err(ProviderError {
                        message: format!("HTTP {status}: {}", truncate_body(&text)),
                        retryable: is_retryable_status(status),
                        http_status: Some(status),
                        code: None,
                    });
                }
                Err(e) => {
                    if self.retry.can_retry(attempts)
                        && (e.is_connect() || e.is_timeout() || e.is_request())
                    {
                        attempts += 1;
                        let delay = self.retry.backoff(attempts, None);
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                return Err(ProviderError::new("request cancelled"));
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                        continue;
                    }
                    return Err(map_reqwest_error(e));
                }
            }
        };

        Ok(Box::pin(self.event_stream(response, cancel)))
    }

    fn event_stream<'a>(
        &'a self,
        response: reqwest::Response,
        cancel: &'a CancellationToken,
    ) -> impl futures::Stream<Item = StreamEvent> + Send + 'a {
        let mut byte_stream = response.bytes_stream();
        let mut decoder = SseDecoder::new();
        let mut tracker = AnthropicEventTracker::default();

        stream! {
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    item = byte_stream.next() => item,
                };
                match chunk {
                    None => break,
                    Some(Err(e)) => {
                        yield StreamEvent::Error(map_reqwest_error(e));
                        break;
                    }
                    Some(Ok(bytes)) => {
                        for payload in decoder.feed(&bytes) {
                            for ev in tracker.ingest(&payload) {
                                yield ev;
                            }
                        }
                    }
                }
            }
            if decoder.has_pending() {
                for payload in decoder.feed(b"\n") {
                    for ev in tracker.ingest(&payload) {
                        yield ev;
                    }
                }
            }
        }
    }
}

fn truncate_body(s: &str) -> String {
    const MAX: usize = 500;
    if s.len() <= MAX {
        return s.to_string();
    }
    let mut idx = MAX;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    format!("{}...", &s[..idx])
}

fn map_reqwest_error(e: reqwest::Error) -> ProviderError {
    let retryable = e.is_connect() || e.is_timeout() || e.is_request();
    ProviderError {
        message: e.to_string(),
        retryable,
        http_status: e.status().map(|s| s.as_u16()),
        code: None,
    }
}

// ---------------- SSE 事件追踪 ----------------

/// Anthropic SSE 事件追踪：把 content_block_* 流映射为中立 StreamEvent。
#[derive(Default)]
struct AnthropicEventTracker {
    /// 当前 tool_use block 的 id（content_block_start 时记录）。
    current_tool_id: Option<String>,
    /// 已发出 ToolCallStarted 的 block index 集合（避免重复）。
    started_blocks: std::collections::HashSet<usize>,
}

impl AnthropicEventTracker {
    fn ingest(&mut self, payload: &str) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        // Anthropic SSE 每条 data 是一个 JSON，带 "type" 字段标识事件类型。
        let event: AnthropicSseEvent = match serde_json::from_str(payload) {
            Ok(e) => e,
            Err(_) => return out,
        };
        match event {
            AnthropicSseEvent::MessageStart {
                message:
                    StartMessage {
                        usage: Some(StartUsage { input_tokens }),
                    },
            } if input_tokens > 0 => {
                out.push(StreamEvent::Usage {
                    input_tokens,
                    output_tokens: 0,
                });
            }
            AnthropicSseEvent::ContentBlockStart {
                index,
                block: AnthropicBlock::ToolUse { id, name, .. },
            } => {
                self.current_tool_id = Some(id.clone());
                if self.started_blocks.insert(index) {
                    out.push(StreamEvent::ToolCallStarted { id, name });
                }
            }
            AnthropicSseEvent::ContentBlockDelta {
                delta: AnthropicDelta::TextDelta { text },
                ..
            } if !text.is_empty() => {
                out.push(StreamEvent::TextDelta(text));
            }
            AnthropicSseEvent::ContentBlockDelta {
                delta: AnthropicDelta::InputJsonDelta { partial_json },
                ..
            } if !partial_json.is_empty() => {
                if let Some(id) = &self.current_tool_id {
                    out.push(StreamEvent::ToolCallDelta {
                        id: id.clone(),
                        arguments: partial_json,
                    });
                }
            }
            AnthropicSseEvent::ContentBlockDelta {
                delta: AnthropicDelta::ThinkingDelta { thinking },
                ..
            } if !thinking.is_empty() => {
                out.push(StreamEvent::Reasoning(thinking));
            }
            AnthropicSseEvent::ContentBlockStop {} => {
                self.current_tool_id = None;
            }
            AnthropicSseEvent::MessageDelta {
                usage: Some(DeltaUsage { output_tokens }),
            } if output_tokens > 0 => {
                out.push(StreamEvent::Usage {
                    input_tokens: 0,
                    output_tokens,
                });
            }
            AnthropicSseEvent::Error { error } => {
                let retryable = error.is_retryable();
                out.push(StreamEvent::Error(ProviderError {
                    message: error.message,
                    retryable,
                    http_status: None,
                    code: Some(error.error_type),
                }));
            }
            _ => {}
        }
        out
    }
}

// ---------------- 线格式类型（请求 + 响应） ----------------

#[derive(Clone, Serialize)]
struct WireRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<WireTool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Clone, Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<WireBlock>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Clone, Serialize)]
struct WireTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

impl WireRequest {
    fn from_parts(
        model: &str,
        max_tokens: u32,
        messages: &[Message],
        tools: &[ToolDef],
        options: &ChatOptions,
    ) -> Self {
        // 提取首条 System 消息为顶层 system 字段；剩余消息转为 Anthropic 消息序列。
        let mut system_text = String::new();
        let mut convo_msgs: Vec<&Message> = Vec::new();
        for m in messages {
            if m.role == Role::System && system_text.is_empty() {
                system_text = m.text.clone();
            } else {
                convo_msgs.push(m);
            }
        }

        let wire_messages = merge_tool_results_into_user(&convo_msgs);

        WireRequest {
            model: model.to_string(),
            max_tokens,
            system: (!system_text.is_empty()).then_some(system_text),
            messages: wire_messages,
            tools: if tools.is_empty() {
                None
            } else {
                Some(
                    tools
                        .iter()
                        .map(|t| WireTool {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            input_schema: t.parameters.clone(),
                        })
                        .collect(),
                )
            },
            stream: true,
            temperature: options.temperature,
        }
    }
}

/// 把内核消息序列转为 Anthropic 消息序列。
///
/// 关键：Anthropic 的 tool_result 必须包在 user 消息里；
/// 连续的 assistant 消息（tool_calls 后的 assistant 文本）与 user/tool 消息按角色分组合并。
fn merge_tool_results_into_user(msgs: &[&Message]) -> Vec<WireMessage> {
    let mut out: Vec<WireMessage> = Vec::new();
    for m in msgs {
        match m.role {
            Role::System => {} // 已提取到顶层 system
            Role::User => {
                let blocks = vec![WireBlock::Text {
                    text: m.text.clone(),
                }];
                out.push(WireMessage {
                    role: "user",
                    content: blocks,
                });
            }
            Role::Assistant => {
                let mut blocks = Vec::new();
                if !m.text.is_empty() {
                    blocks.push(WireBlock::Text {
                        text: m.text.clone(),
                    });
                }
                for tc in &m.tool_calls {
                    let input: serde_json::Value = serde_json::from_str(&tc.arguments)
                        .unwrap_or(serde_json::Value::Object(Default::default()));
                    blocks.push(WireBlock::ToolUse {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input,
                    });
                }
                if blocks.is_empty() {
                    continue;
                }
                out.push(WireMessage {
                    role: "assistant",
                    content: blocks,
                });
            }
            Role::Tool => {
                // tool_result 包进最近一条 user 消息（或新建一条 user 消息）
                let block = WireBlock::ToolResult {
                    tool_use_id: m.tool_call_id.clone().unwrap_or_default(),
                    content: m.text.clone(),
                    is_error: m.is_error,
                };
                match out.last_mut() {
                    Some(last) if last.role == "user" => last.content.push(block),
                    _ => out.push(WireMessage {
                        role: "user",
                        content: vec![block],
                    }),
                }
            }
        }
    }
    out
}

// ---------------- SSE 事件反序列化 ----------------

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicSseEvent {
    MessageStart {
        message: StartMessage,
    },
    ContentBlockStart {
        index: usize,
        block: AnthropicBlock,
    },
    ContentBlockDelta {
        delta: AnthropicDelta,
    },
    ContentBlockStop {},
    MessageDelta {
        usage: Option<DeltaUsage>,
    },
    MessageStop {},
    Ping {},
    Error {
        error: AnthropicErrorBody,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct StartMessage {
    #[serde(default)]
    usage: Option<StartUsage>,
}

#[derive(Deserialize)]
struct StartUsage {
    #[serde(default)]
    input_tokens: u64,
}

#[derive(Deserialize)]
struct DeltaUsage {
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicBlock {
    Text,
    ToolUse {
        id: String,
        name: String,
    },
    Thinking,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct AnthropicErrorBody {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

impl AnthropicErrorBody {
    fn is_retryable(&self) -> bool {
        matches!(
            self.error_type.as_str(),
            "overloaded_error" | "rate_limit_error" | "api_error"
        )
    }
}

// ---------------- 测试 ----------------

#[cfg(test)]
mod tests {
    use super::*;
    use mrorcode_kernel::message::ToolCall;

    #[test]
    fn system_extracted_to_top_level() {
        let msgs = [Message::system("You are helpful."), Message::user("hi")];
        let req = WireRequest::from_parts("claude", 4096, &msgs, &[], &ChatOptions::default());
        assert_eq!(req.system.as_deref(), Some("You are helpful."));
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
    }

    #[test]
    fn tool_result_merged_into_user() {
        let msgs = [
            Message::user("read file"),
            Message::assistant_tool_calls(vec![ToolCall::new("c1", "read_file")]),
            Message::tool_result(&mrorcode_kernel::tool::ToolResult {
                call_id: "c1".into(),
                content: "file contents".into(),
                is_error: false,
            }),
        ];
        let req = WireRequest::from_parts("claude", 4096, &msgs, &[], &ChatOptions::default());
        // user / assistant(tool_use) / user(tool_result)
        assert_eq!(req.messages.len(), 3);
        assert_eq!(req.messages[0].role, "user");
        assert_eq!(req.messages[1].role, "assistant");
        assert_eq!(req.messages[2].role, "user");
        // 第三条 user 消息含 tool_result 块
        match &req.messages[2].content[0] {
            WireBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                assert_eq!(tool_use_id, "c1");
                assert_eq!(content, "file contents");
            }
            other => panic!("expected ToolResult block, got {other:?}"),
        }
    }

    #[test]
    fn assistant_text_and_tool_use_same_message() {
        let mut a = Message::assistant_text("Let me check.");
        a.tool_calls.push(ToolCall::new("c1", "bash"));
        let msgs = [Message::user("run ls"), a.clone()];
        let req = WireRequest::from_parts("claude", 4096, &msgs, &[], &ChatOptions::default());
        assert_eq!(req.messages[1].content.len(), 2); // text + tool_use
    }

    #[test]
    fn sse_tracks_text_and_tool_use() {
        let mut tr = AnthropicEventTracker::default();
        let evs = tr.ingest(r#"{"type":"content_block_start","index":0,"block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}"#);
        assert!(
            matches!(&evs[0], StreamEvent::ToolCallStarted { id, name } if id == "toolu_1" && name == "bash")
        );

        let evs2 = tr.ingest(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"},"index":0}"#);
        assert!(
            matches!(&evs2[0], StreamEvent::ToolCallDelta { id, arguments } if id == "toolu_1" && arguments == "{\"cmd\":")
        );

        tr.ingest(r#"{"type":"content_block_stop","index":0}"#);

        let evs3 = tr.ingest(
            r#"{"type":"content_block_start","index":1,"block":{"type":"text","text":""}}"#,
        );
        assert!(evs3.is_empty());

        let evs4 = tr.ingest(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"done"},"index":1}"#);
        assert!(matches!(&evs4[0], StreamEvent::TextDelta(t) if t == "done"));
    }

    #[test]
    fn sse_usage_from_message_start_and_delta() {
        let mut tr = AnthropicEventTracker::default();
        let evs = tr.ingest(r#"{"type":"message_start","message":{"usage":{"input_tokens":15}}}"#);
        assert!(matches!(
            &evs[0],
            StreamEvent::Usage {
                input_tokens: 15,
                output_tokens: 0
            }
        ));

        let evs2 = tr.ingest(r#"{"type":"message_delta","usage":{"output_tokens":42}}"#);
        assert!(matches!(
            &evs2[0],
            StreamEvent::Usage {
                input_tokens: 0,
                output_tokens: 42
            }
        ));
    }

    #[test]
    fn sse_error_event_maps_to_provider_error() {
        let mut tr = AnthropicEventTracker::default();
        let evs =
            tr.ingest(r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#);
        match &evs[0] {
            StreamEvent::Error(e) => {
                assert!(e.retryable);
                assert_eq!(e.message, "busy");
            }
            other => panic!("expected Error event, got {other:?}"),
        }
    }

    #[test]
    fn request_omits_tools_when_empty() {
        let req = WireRequest::from_parts(
            "claude",
            4096,
            &[Message::user("x")],
            &[],
            &ChatOptions::default(),
        );
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("tools").is_none());
        assert_eq!(v["max_tokens"], 4096);
        assert_eq!(v["stream"], true);
    }
}
