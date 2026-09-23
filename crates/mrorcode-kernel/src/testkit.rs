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
