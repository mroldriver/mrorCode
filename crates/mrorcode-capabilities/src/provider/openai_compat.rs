//! OpenAI 兼容 `/chat/completions` 流式 provider。
//!
//! 覆盖 OpenAI / DeepSeek / GLM / vLLM / Ollama OpenAI 端点等一切同构 API。
//! 手写 SSE 解码（见 [`super::sse`]），不依赖 eventsource 库。

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_stream::stream;
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use mrorcode_kernel::message::{Message, Role};
use mrorcode_kernel::provider::{
    ChatOptions, LlmProvider, ProviderError, ProviderResult, StreamEvent, ToolChoice,
};
use mrorcode_kernel::tool::ToolDef;

use super::retry::{is_retryable_status, parse_retry_after, RetryPolicy};
use super::sse::SseDecoder;

/// 配置。
#[derive(Clone, Debug)]
pub struct OpenAiCompatConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub context_window: u32,
    pub timeout: Duration,
}

impl Default for OpenAiCompatConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4o-mini".to_string(),
            context_window: 128_000,
            timeout: Duration::from_secs(300),
        }
    }
}

/// OpenAI 兼容 provider。
pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    context_window: u32,
    retry: RetryPolicy,
}

impl OpenAiCompatProvider {
    pub fn new(config: OpenAiCompatConfig) -> anyhow::Result<Self> {
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
            retry: RetryPolicy::default(),
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiCompatProvider {
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
        // 每轮模型覆盖（SetModel）优先于 provider 自带模型
        let model = options.model.clone().unwrap_or_else(|| self.model.clone());
        let body = WireRequest::from_parts(&model, messages, tools, options);
        Box::pin(async move { self.start(body, cancel).await })
    }
}

impl OpenAiCompatProvider {
    /// 发起请求（含建连/首响应重试），成功后返回事件流。
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
                .headers(self.auth_headers())
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

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if !self.api_key.is_empty() {
            if let Ok(v) =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.api_key))
            {
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        headers
    }

    /// 把成功响应体的 SSE 字节流转换为中立事件流。
    fn event_stream<'a>(
        &'a self,
        response: reqwest::Response,
        cancel: &'a CancellationToken,
    ) -> impl futures::Stream<Item = StreamEvent> + Send + 'a {
        let mut byte_stream = response.bytes_stream();
        let mut decoder = SseDecoder::new();
        let mut tracker = ToolCallTracker::default();

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
                            if payload == "[DONE]" {
                                continue;
                            }
                            match serde_json::from_str::<Chunk>(&payload) {
                                Ok(chunk) => {
                                    for ev in tracker.ingest(chunk) {
                                        yield ev;
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("openai: skipping unparsed chunk: {e}");
                                }
                            }
                        }
                    }
                }
            }
            // 刷新无尾换行的最后一行（服务端通常以 [DONE]\n 结尾）
            if decoder.has_pending() {
                for payload in decoder.feed(b"\n") {
                    if payload == "[DONE]" {
                        continue;
                    }
                    if let Ok(chunk) = serde_json::from_str::<Chunk>(&payload) {
                        for ev in tracker.ingest(chunk) {
                            yield ev;
                        }
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

/// 流式 tool_calls 的 index → 稳定 id 映射与事件产出。
#[derive(Default)]
struct ToolCallTracker {
    by_index: HashMap<usize, String>,
}

impl ToolCallTracker {
    fn ingest(&mut self, chunk: Chunk) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if let Some(usage) = chunk.usage {
            if usage.prompt_tokens + usage.completion_tokens > 0 {
                out.push(StreamEvent::Usage {
                    input_tokens: usage.prompt_tokens,
                    output_tokens: usage.completion_tokens,
                });
            }
        }
        for choice in chunk.choices {
            if let Some(t) = choice.delta.content {
                if !t.is_empty() {
                    out.push(StreamEvent::TextDelta(t));
                }
            }
            if let Some(t) = choice.delta.reasoning_content {
                if !t.is_empty() {
                    out.push(StreamEvent::Reasoning(t));
                }
            }
            for tc in choice.delta.tool_calls {
                let id = if let Some(id) = self.by_index.get(&tc.index) {
                    id.clone()
                } else {
                    let id = tc
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("call-{}", tc.index));
                    let name = tc.function.as_ref().and_then(|f| f.name.clone());
                    out.push(StreamEvent::ToolCallStarted {
                        id: id.clone(),
                        name: name.unwrap_or_default(),
                    });
                    self.by_index.insert(tc.index, id.clone());
                    id
                };
                if let Some(func) = tc.function {
                    if let Some(args) = func.arguments {
                        if !args.is_empty() {
                            out.push(StreamEvent::ToolCallDelta {
                                id,
                                arguments: args,
                            });
                        }
                    }
                }
            }
        }
        out
    }
}

// ---------------- 线格式类型 ----------------

#[derive(Clone, Serialize)]
struct WireRequest {
    model: String,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<WireTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    stream: bool,
    stream_options: StreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Clone, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Clone, Serialize)]
struct WireMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<WireToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Clone, Serialize)]
struct WireToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunctionCall,
}

#[derive(Clone, Serialize)]
struct WireFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Clone, Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunctionDef,
}

#[derive(Clone, Serialize)]
struct WireFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl WireRequest {
    fn from_parts(
        model: &str,
        messages: &[Message],
        tools: &[ToolDef],
        options: &ChatOptions,
    ) -> Self {
        WireRequest {
            model: model.to_string(),
            messages: messages.iter().map(WireMessage::from).collect(),
            tools: if tools.is_empty() {
                None
            } else {
                Some(
                    tools
                        .iter()
                        .map(|t| WireTool {
                            kind: "function",
                            function: WireFunctionDef {
                                name: t.name.clone(),
                                description: t.description.clone(),
                                parameters: t.parameters.clone(),
                            },
                        })
                        .collect(),
                )
            },
            tool_choice: if tools.is_empty() {
                None
            } else {
                Some(match &options.tool_choice {
                    ToolChoice::Auto => serde_json::json!("auto"),
                    ToolChoice::None => serde_json::json!("none"),
                    ToolChoice::Required => serde_json::json!("required"),
                    ToolChoice::Named(n) => {
                        serde_json::json!({"type":"function","function":{"name":n}})
                    }
                })
            },
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
            temperature: options.temperature,
            max_tokens: options.max_tokens,
        }
    }
}

impl From<&Message> for WireMessage {
    fn from(m: &Message) -> Self {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        let content = if m.text.is_empty() {
            None
        } else {
            Some(m.text.clone())
        };
        let tool_calls = if m.tool_calls.is_empty() {
            None
        } else {
            Some(
                m.tool_calls
                    .iter()
                    .map(|c| WireToolCall {
                        id: c.id.clone(),
                        kind: "function",
                        function: WireFunctionCall {
                            name: c.name.clone(),
                            arguments: c.arguments.clone(),
                        },
                    })
                    .collect(),
            )
        };
        WireMessage {
            role,
            content,
            tool_calls,
            tool_call_id: m.tool_call_id.clone(),
        }
    }
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<ChunkUsage>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
}

#[derive(Default, Deserialize)]
struct Delta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallChunk>,
}

#[derive(Deserialize)]
struct ToolCallChunk {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionChunk>,
}

#[derive(Deserialize)]
struct FunctionChunk {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ChunkUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_maps_content_reasoning_and_tool_fragments() {
        let mut tr = ToolCallTracker::default();
        let evs = tr.ingest(
            serde_json::from_str(
                r#"{
            "choices":[{"delta":{"content":"hi","reasoning_content":"think","tool_calls":[
              {"index":0,"id":"call_1","function":{"name":"bash","arguments":""}}
            ]}}]
        }"#,
            )
            .unwrap(),
        );
        assert!(matches!(&evs[0], StreamEvent::TextDelta(t) if t == "hi"));
        assert!(matches!(&evs[1], StreamEvent::Reasoning(t) if t == "think"));
        assert!(matches!(
            &evs[2],
            StreamEvent::ToolCallStarted { id, name } if id == "call_1" && name == "bash"
        ));

        // 后续参数分片复用同一 id，不再重复 Started
        let evs2 = tr.ingest(
            serde_json::from_str(
                r#"{
            "choices":[{"delta":{"tool_calls":[
              {"index":0,"function":{"arguments":"{\"cmd\":"}},
              {"index":1,"id":"call_2","function":{"name":"echo","arguments":"{}"}}
            ]}}]
        }"#,
            )
            .unwrap(),
        );
        assert!(matches!(
            &evs2[0],
            StreamEvent::ToolCallDelta { id, arguments } if id == "call_1" && arguments == "{\"cmd\":"
        ));
        assert!(matches!(&evs2[1], StreamEvent::ToolCallStarted { id, .. } if id == "call_2"));
        assert!(matches!(&evs2[2], StreamEvent::ToolCallDelta { id, .. } if id == "call_2"));
    }

    #[test]
    fn tracker_emits_usage_once() {
        let mut tr = ToolCallTracker::default();
        let evs = tr.ingest(
            serde_json::from_str(
                r#"{
            "choices":[],
            "usage":{"prompt_tokens":10,"completion_tokens":20}
        }"#,
            )
            .unwrap(),
        );
        assert_eq!(
            evs,
            vec![StreamEvent::Usage {
                input_tokens: 10,
                output_tokens: 20
            }]
        );
    }

    #[test]
    fn message_wire_mapping() {
        let msgs = [
            Message::user("hi"),
            Message::assistant_tool_calls(vec![mrorcode_kernel::message::ToolCall::new(
                "c1", "bash",
            )]),
            Message::tool_result(&mrorcode_kernel::tool::ToolResult {
                call_id: "c1".into(),
                content: "ok".into(),
                is_error: false,
            }),
        ];
        let wire: Vec<WireMessage> = msgs.iter().map(Into::into).collect();
        assert_eq!(wire[0].role, "user");
        assert_eq!(wire[0].content.as_deref(), Some("hi"));
        assert_eq!(
            wire[1].tool_calls.as_ref().unwrap()[0].function.name,
            "bash"
        );
        assert_eq!(wire[2].role, "tool");
        assert_eq!(wire[2].tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn request_omits_tools_when_empty() {
        let req = WireRequest::from_parts("m", &[Message::user("x")], &[], &ChatOptions::default());
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("tools").is_none());
        assert!(v.get("tool_choice").is_none());
        assert_eq!(v["stream"], true);
        assert_eq!(v["stream_options"]["include_usage"], true);
    }
}
