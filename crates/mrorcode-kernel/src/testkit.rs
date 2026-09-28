//! 测试用具：脚本化 provider、简易工具与事件收集助手。
//!
//! 仅在 `test-support` feature 或单测构建时可用。

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::AgentHandle;
use crate::event::AgentEvent;
use crate::message::Message;
use crate::provider::{ChatOptions, LlmProvider, ProviderResult, StreamEvent};
use crate::tool::{RiskLevel, Tool, ToolContext, ToolDef, ToolResult};

/// 按脚本逐次响应的 provider：第 N 次 `chat_stream` 弹出第 N 个脚本。
#[derive(Clone)]
pub struct ScriptedProvider {
    scripts: Arc<Mutex<VecDeque<Vec<StreamEvent>>>>,
    calls: Arc<AtomicUsize>,
    model: String,
}

impl ScriptedProvider {
    pub fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(scripts.into_iter().collect())),
            calls: Arc::new(AtomicUsize::new(0)),
            model: "scripted-model".to_string(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 实际发起的请求次数。
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn chat_stream<'a>(
        &'a self,
        _messages: &'a [Message],
        _tools: &'a [ToolDef],
        _options: &'a ChatOptions,
        _cancel: &'a CancellationToken,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = ProviderResult<BoxStream<'a, StreamEvent>>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut q = self.scripts.lock().await;
            if q.is_empty() {
                return Ok(Box::pin(stream::empty()) as BoxStream<'a, StreamEvent>);
            }
            let events = q.pop_front().expect("ScriptedProvider: script exhausted");
            Ok(Box::pin(stream::iter(events)) as BoxStream<'a, StreamEvent>)
        })
    }
}

/// 原样返回参数 JSON 的工具（Safe）。
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo back the arguments (test tool)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })
    }

    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        RiskLevel::Safe
    }

    async fn execute(&self, arguments: &str, _ctx: &ToolContext<'_>) -> ToolResult {
        ToolResult::ok("echo-1", arguments.to_string())
    }
}

/// 构造一个完整单工具调用脚本（started + 参数 delta 一次给全）。
pub fn single_tool_call_script(
    text: &str,
    id: &str,
    name: &str,
    args: serde_json::Value,
) -> Vec<StreamEvent> {
    vec![
        StreamEvent::TextDelta(text.to_string()),
        StreamEvent::ToolCallStarted {
            id: id.to_string(),
            name: name.to_string(),
        },
        StreamEvent::ToolCallDelta {
            id: id.to_string(),
            arguments: args.to_string(),
        },
    ]
}

/// 纯文本回复脚本。
pub fn text_script(text: &str) -> Vec<StreamEvent> {
    vec![StreamEvent::TextDelta(text.to_string())]
}

/// 收集事件直到遇到终态（TurnComplete/Cancelled）。
pub async fn drain_until_terminal(handle: &mut AgentHandle) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    while let Some(ev) = handle.events.recv().await {
        let terminal = matches!(ev, AgentEvent::TurnComplete { .. } | AgentEvent::Cancelled);
        out.push(ev);
        if terminal {
            break;
        }
    }
    out
}

/// 提取事件的类型标签（conformance 序列断言用，M5-d）。
pub fn event_type_label(ev: &AgentEvent) -> &'static str {
    match ev {
        AgentEvent::TurnStarted => "TurnStarted",
        AgentEvent::TextDelta(_) => "TextDelta",
        AgentEvent::Reasoning(_) => "Reasoning",
        AgentEvent::ToolStarted { .. } => "ToolStarted",
        AgentEvent::ToolResult { .. } => "ToolResult",
        AgentEvent::Usage { .. } => "Usage",
        AgentEvent::Request { .. } => "Request",
        AgentEvent::TurnComplete { .. } => "TurnComplete",
        AgentEvent::Error { .. } => "Error",
        AgentEvent::Cancelled => "Cancelled",
    }
}

/// 断言事件序列的类型标签与期望一致（顺序敏感）。
///
/// conformance 套件：无论底层 provider 是 OpenAI 兼容还是 Anthropic，
/// agent 循环消费 StreamEvent 后产出的 AgentEvent 类型序列在标准场景下应一致。
pub fn assert_event_types(events: &[AgentEvent], expected: &[&str]) {
    let actual: Vec<&str> = events.iter().map(event_type_label).collect();
    assert_eq!(
        actual, expected,
        "event type sequence mismatch (actual={actual:?}, expected={expected:?})"
    );
}

/// 流事件的紧凑摘要：跨 provider 对比时，两个 wire 格式应产出同一份摘要。
pub fn stream_signature(ev: &StreamEvent) -> String {
    match ev {
        StreamEvent::TextDelta(t) => format!("text:{t}"),
        StreamEvent::Reasoning(t) => format!("reasoning:{t}"),
        StreamEvent::ToolCallStarted { id, name } => format!("tool_start:{id}:{name}"),
        StreamEvent::ToolCallDelta { id, arguments } => format!("tool_args:{id}:{arguments}"),
        StreamEvent::Usage {
            input_tokens,
            output_tokens,
        } => format!("usage:{input_tokens}/{output_tokens}"),
        StreamEvent::Error(e) => format!("error:{}:{}", e.retryable, e.message),
    }
}

/// 断言流事件序列的摘要等于期望（M5-d：provider conformance 的公共断言器）。
pub fn assert_stream_sequence(actual: &[StreamEvent], expected: &[&str]) {
    let got: Vec<String> = actual.iter().map(stream_signature).collect();
    assert_eq!(got, expected, "stream event sequence drifted");
}

/// agent 事件的紧凑摘要（不含字段值，只留类型与关键 id）。
pub fn event_signature(ev: &AgentEvent) -> String {
    match ev {
        AgentEvent::TurnStarted => "turn_started".to_string(),
        AgentEvent::TextDelta(_) => "text_delta".to_string(),
        AgentEvent::Reasoning(_) => "reasoning_delta".to_string(),
        AgentEvent::ToolStarted { name, .. } => format!("tool_started:{name}"),
        AgentEvent::ToolResult { name, is_error, .. } => {
            format!(
                "tool_result:{name}:{}",
                if *is_error { "error" } else { "ok" }
            )
        }
        AgentEvent::Usage { .. } => "usage".to_string(),
        AgentEvent::Request { kind, .. } => format!("request:{kind}"),
        AgentEvent::TurnComplete { stop_reason } => {
            format!("turn_complete:{stop_reason:?}").to_lowercase()
        }
        AgentEvent::Error { retryable, .. } => format!("error:{retryable}"),
        AgentEvent::Cancelled => "cancelled".to_string(),
    }
}

/// 断言 agent 事件序列的摘要等于期望（M5-d：driver 侧序列冻结的公共断言器）。
pub fn assert_event_sequence(actual: &[AgentEvent], expected: &[&str]) {
    let got: Vec<String> = actual.iter().map(event_signature).collect();
    assert_eq!(got, expected, "agent event sequence drifted");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderError;

    #[test]
    fn signatures_are_stable_and_typed() {
        let evs = vec![
            StreamEvent::TextDelta("a".into()),
            StreamEvent::Reasoning("b".into()),
            StreamEvent::ToolCallStarted {
                id: "i".into(),
                name: "n".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "i".into(),
                arguments: "{}".into(),
            },
            StreamEvent::Usage {
                input_tokens: 1,
                output_tokens: 2,
            },
            StreamEvent::Error(ProviderError {
                message: "m".into(),
                retryable: true,
                http_status: None,
                code: None,
            }),
        ];
        assert_stream_sequence(
            &evs,
            &[
                "text:a",
                "reasoning:b",
                "tool_start:i:n",
                "tool_args:i:{}",
                "usage:1/2",
                "error:true:m",
            ],
        );
    }

    #[test]
    fn agent_event_signatures() {
        let evs = vec![
            AgentEvent::TurnStarted,
            AgentEvent::TextDelta("x".into()),
            AgentEvent::ToolStarted {
                id: "i".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            },
            AgentEvent::ToolResult {
                id: "i".into(),
                name: "bash".into(),
                content: "c".into(),
                is_error: true,
            },
            AgentEvent::TurnComplete {
                stop_reason: crate::event::StopReason::Stopped,
            },
        ];
        assert_event_sequence(
            &evs,
            &[
                "turn_started",
                "text_delta",
                "tool_started:bash",
                "tool_result:bash:error",
                "turn_complete:stopped",
            ],
        );
    }
}
