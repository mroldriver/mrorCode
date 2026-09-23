//! agent 循环集成测试：纯聊天、单工具两轮、连续多工具、deny 后继续、
//! 取消终态、工具结果截断。

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt, once, pending};
use serde_json::json;

use mrorcode_kernel::event::AgentEvent;
use mrorcode_kernel::middleware::{BeforeOutcome, ToolMiddleware};
use mrorcode_kernel::message::ToolCall;
use mrorcode_kernel::provider::{ChatOptions, LlmProvider, ProviderResult, StreamEvent};
use mrorcode_kernel::testkit::{
    EchoTool, ScriptedProvider, drain_until_terminal, single_tool_call_script, text_script,
};
use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolRegistry, ToolResult};
use mrorcode_kernel::{AgentBuilder, AgentCommand};

fn registry_with_echo() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(EchoTool));
    reg
}

fn multi_tool_call_script(id_pairs: &[(&str, &str)]) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    for (id, name) in id_pairs {
        events.push(StreamEvent::ToolCallStarted {
            id: (*id).to_string(),
            name: (*name).to_string(),
        });
        events.push(StreamEvent::ToolCallDelta {
            id: (*id).to_string(),
            arguments: json!({"text": id}).to_string(),
        });
    }
    events
}

/// 拒绝一切的中间件。
struct DenyAll;

#[async_trait]
impl ToolMiddleware for DenyAll {
    async fn before(
        &self,
        _call: &mut ToolCall,
        _risk: RiskLevel,
        _tool: &Arc<dyn Tool>,
        _request: &mrorcode_kernel::Requester,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> BeforeOutcome {
        BeforeOutcome::Deny {
            reason: "test deny".to_string(),
        }
    }
}

/// 返回超大内容的工具。
struct BigTool {
    size: usize,
}

#[async_trait]
impl Tool for BigTool {
    fn name(&self) -> &str {
        "big"
    }
    fn description(&self) -> &str {
        "returns huge output"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }
    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        RiskLevel::Safe
    }
    async fn execute(&self, _arguments: &str, _ctx: &ToolContext<'_>) -> ToolResult {
        ToolResult::ok("big-1", "x".repeat(self.size))
    }
}

/// 发出一个 delta 后永久挂起的 provider（用于取消测试）。
#[derive(Default)]
struct PendingProvider;

#[async_trait]
impl LlmProvider for PendingProvider {
    fn model_name(&self) -> &str {
        "pending"
    }

    fn chat_stream<'a>(
        &'a self,
        _messages: &'a [mrorcode_kernel::Message],
        _tools: &'a [mrorcode_kernel::ToolDef],
        _options: &'a ChatOptions,
        _cancel: &'a tokio_util::sync::CancellationToken,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = ProviderResult<BoxStream<'a, StreamEvent>>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            let s = once(async { StreamEvent::TextDelta("...".into()) })
                .chain(pending());
            Ok(Box::pin(s) as BoxStream<'a, StreamEvent>)
        })
    }
}

#[tokio::test]
async fn single_turn_plain_chat() {
    let provider = ScriptedProvider::new(vec![text_script("hello")]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider.clone()))
        .tools(registry_with_echo())
        .spawn();

    handle
        .commands
        .send(AgentCommand::SendMessage("hi".into()))
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    assert!(matches!(
        events.last().unwrap(),
        AgentEvent::TurnComplete {
            stop_reason: mrorcode_kernel::StopReason::Stopped
        }
    ));
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "hello");
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn single_tool_takes_two_rounds() {
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("", "c1", "echo", json!({"text": "hi"})),
        text_script("done"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(registry_with_echo())
        .spawn();

    handle
        .commands
        .send(AgentCommand::SendMessage("use echo".into()))
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolStarted { name, .. } if name == "echo"
    )));
    let result = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult { name, content, .. } if name == "echo" => Some(content),
            _ => None,
        })
        .expect("echo result");
    assert!(result.contains("hi"));
    assert!(matches!(
        events.last().unwrap(),
        AgentEvent::TurnComplete {
            stop_reason: mrorcode_kernel::StopReason::Stopped
        }
    ));
}

#[tokio::test]
async fn multiple_tools_run_in_order() {
    let provider = ScriptedProvider::new(vec![
        multi_tool_call_script(&[("c1", "echo"), ("c2", "echo")]),
        text_script("all done"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(registry_with_echo())
        .spawn();

    handle
        .commands
        .send(AgentCommand::SendMessage("two echoes".into()))
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    let ids: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["c1", "c2"]);
}

#[tokio::test]
async fn denial_is_fed_back_and_turn_continues() {
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("", "c1", "echo", json!({"text": "x"})),
        text_script("after deny"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(registry_with_echo())
        .add_middleware(Arc::new(DenyAll))
        .spawn();

    handle
        .commands
        .send(AgentCommand::SendMessage("deny me".into()))
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    let denied = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult {
                is_error: true,
                content,
                ..
            } => Some(content),
            _ => None,
        })
        .expect("denied result");
    assert!(denied.contains("denied"));
    // 模型拿到拒绝结果后继续完成了一轮
    assert!(events.iter().any(|e| matches!(e, AgentEvent::TextDelta(t) if t == "after deny")));
}

#[tokio::test]
async fn cancel_terminates_running_turn() {
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(PendingProvider))
        .tools(registry_with_echo())
        .spawn();

    handle
        .commands
        .send(AgentCommand::SendMessage("go".into()))
        .unwrap();

    // 等到首个 delta 表明轮次已运行
    assert!(matches!(handle.events.recv().await, Some(AgentEvent::TurnStarted)));
    assert!(matches!(
        handle.events.recv().await,
        Some(AgentEvent::TextDelta(_))
    ));

    handle.commands.send(AgentCommand::Cancel).unwrap();
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(5), handle.events.recv())
        .await
        .expect("cancel should terminate promptly")
        .unwrap();
    assert!(matches!(terminal, AgentEvent::Cancelled));
}

#[tokio::test]
async fn tool_result_is_truncated_head_tail() {
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(BigTool { size: 100_000 }));
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("", "b1", "big", json!({})),
        text_script("ok"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(reg)
        .max_tool_result_bytes(1024)
        .spawn();

    handle
        .commands
        .send(AgentCommand::SendMessage("big".into()))
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    let content = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult { name, content, .. } if name == "big" => Some(content),
            _ => None,
        })
        .unwrap();
    assert!(content.contains("truncated"));
    assert!(content.len() < 1200);
}
