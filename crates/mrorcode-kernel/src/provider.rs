//! LLM provider 抽象与流式事件。

use std::pin::Pin;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::message::Message;
use crate::tool::ToolDef;

/// provider 返回的流式事件。
///
/// 工具调用参数可能分片到达：先 `ToolCallStarted`，随后若干 `ToolCallDelta`
/// 追加 JSON 片段，由 agent 负责累积。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    /// 正文文本增量。
    TextDelta(String),
    /// 推理/思考文本增量（provider 不支持时不会出现）。
    Reasoning(String),
    /// 新工具调用开始。
    ToolCallStarted { id: String, name: String },
    /// 工具调用参数 JSON 片段。
    ToolCallDelta { id: String, arguments: String },
    /// token 用量（同一会话中按字段取最大值合并）。
    Usage { input_tokens: u64, output_tokens: u64 },
    /// provider 错误（流中途失败）。
    Error(ProviderError),
}

/// provider 错误。`retryable` 决定 agent 是否重试。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("provider error: {message}")]
pub struct ProviderError {
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl ProviderError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            http_status: None,
            code: None,
        }
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    /// 识别上下文窗口溢出（错误文案启发式，兼容各家 provider）。
    pub fn is_context_overflow(&self) -> bool {
        let m = self.message.to_ascii_lowercase();
        const NEEDLES: &[&str] = &[
            "context_length_exceeded",
            "context length",
            "context window",
            "maximum context",
            "too many tokens",
            "prompt is too long",
        ];
        NEEDLES.iter().any(|n| m.contains(n))
    }
}

pub type ProviderResult<T> = Result<T, ProviderError>;

/// 请求选项——内核只转发，不做策略。
#[derive(Clone, Debug, Default)]
pub struct ChatOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tool_choice: ToolChoice,
}

/// 工具选择倾向（透传给 provider）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Named(String),
}

/// 模型 provider。实现者负责协议细节与 SSE 解码，产出中立 [`StreamEvent`]。
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// 模型名（展示与日志用）。
    fn model_name(&self) -> &str;

    /// 上下文窗口大小（token 数）。
    fn context_window(&self) -> u32 {
        128_000
    }

    /// 发起一次流式对话。`cancel` 在 provider 侧应参与 select，
    /// 取消时尽快终止流并返回（或直接丢弃流）。
    fn chat_stream<'a>(
        &'a self,
        messages: &'a [Message],
        tools: &'a [ToolDef],
        options: &'a ChatOptions,
        cancel: &'a CancellationToken,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = ProviderResult<BoxStream<'a, StreamEvent>>,
                > + Send
                + 'a,
        >,
    >;
}
