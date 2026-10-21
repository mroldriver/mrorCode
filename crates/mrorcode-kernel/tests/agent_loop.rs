//! agent 循环集成测试：纯聊天、单工具两轮、连续多工具、deny 后继续、
//! 取消终态、工具结果截断。

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{once, pending, BoxStream, StreamExt};
use serde_json::json;

use mrorcode_kernel::event::{AgentEvent, PermissionMode};
use mrorcode_kernel::message::ToolCall;
use mrorcode_kernel::middleware::{BeforeOutcome, ToolMiddleware};
use mrorcode_kernel::provider::{ChatOptions, LlmProvider, ProviderResult, StreamEvent};
use mrorcode_kernel::testkit::{
    assert_event_types, drain_until_terminal, single_tool_call_script, text_script, EchoTool,
    ScriptedProvider,
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

/// 默认 Risky 的工具（只读模式应被拒绝）。
struct RiskyTool;

#[async_trait]
impl Tool for RiskyTool {
    fn name(&self) -> &str {
        "risky"
    }
    fn description(&self) -> &str {
        "writes something"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }
    // 不覆盖 risk() → 默认 Risky
    async fn execute(&self, _arguments: &str, _ctx: &ToolContext<'_>) -> ToolResult {
        ToolResult::ok("risky-1", "wrote")
    }
}

/// WorkspaceWrite 级别的工具（模拟 edit/write：工作区内写）。
struct WorkspaceWriteTool;

#[async_trait]
impl Tool for WorkspaceWriteTool {
    fn name(&self) -> &str {
        "ws_write"
    }
    fn description(&self) -> &str {
        "writes inside workspace"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }
    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        RiskLevel::WorkspaceWrite
    }
    async fn execute(&self, _arguments: &str, _ctx: &ToolContext<'_>) -> ToolResult {
        ToolResult::ok("ws-1", "wrote")
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
            dyn std::future::Future<Output = ProviderResult<BoxStream<'a, StreamEvent>>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            let s = once(async { StreamEvent::TextDelta("...".into()) }).chain(pending());
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
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "hi".into(),
        })
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
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "use echo".into(),
        })
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
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "two echoes".into(),
        })
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
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "deny me".into(),
        })
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
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::TextDelta(t) if t == "after deny")));
}

#[tokio::test]
async fn cancel_terminates_running_turn() {
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(PendingProvider))
        .tools(registry_with_echo())
        .spawn();

    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "go".into(),
        })
        .unwrap();

    // 等到首个 delta 表明轮次已运行
    assert!(matches!(
        handle.events.recv().await,
        Some(AgentEvent::TurnStarted)
    ));
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

/// 直到取消才返回的工具；取消时其 future 会被 agent 丢弃。
struct BlockingTool;

#[async_trait]
impl Tool for BlockingTool {
    fn name(&self) -> &str {
        "block"
    }
    fn description(&self) -> &str {
        "blocks until cancelled"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }
    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        RiskLevel::Safe
    }
    async fn execute(&self, _arguments: &str, ctx: &ToolContext<'_>) -> ToolResult {
        ctx.cancel.cancelled().await;
        ToolResult::error("block", json!({"error": "cancelled"}).to_string())
    }
}

/// 第一轮返回两个工具调用，并记录每次请求的消息快照（用于校验配对）。
#[derive(Clone, Default)]
struct TwoToolRoundProvider {
    requests: Arc<tokio::sync::Mutex<Vec<Vec<mrorcode_kernel::Message>>>>,
    round: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl LlmProvider for TwoToolRoundProvider {
    fn model_name(&self) -> &str {
        "two-tool-round"
    }

    fn chat_stream<'a>(
        &'a self,
        messages: &'a [mrorcode_kernel::Message],
        _tools: &'a [mrorcode_kernel::ToolDef],
        _options: &'a ChatOptions,
        _cancel: &'a tokio_util::sync::CancellationToken,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = ProviderResult<BoxStream<'a, StreamEvent>>>
                + Send
                + 'a,
        >,
    > {
        let requests = Arc::clone(&self.requests);
        let round = Arc::clone(&self.round);
        let snapshot = messages.to_vec();
        Box::pin(async move {
            requests.lock().await.push(snapshot);
            let n = round.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let events = if n == 0 {
                multi_tool_call_script(&[("c1", "block"), ("c2", "block")])
            } else {
                vec![StreamEvent::TextDelta("done".into())]
            };
            Ok(Box::pin(futures::stream::iter(events)) as BoxStream<'a, StreamEvent>)
        })
    }
}

/// 取消发生在多工具轮次中途：历史里不能留下悬空 tool_calls，后续请求必须合法。
#[tokio::test]
async fn cancel_mid_tool_round_leaves_valid_tool_pairing() {
    let provider = TwoToolRoundProvider::default();
    let requests = Arc::clone(&provider.requests);
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(BlockingTool));

    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(reg)
        .spawn();

    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "go".into(),
        })
        .unwrap();

    // 等到工具开始执行：此时带 tool_calls 的 assistant 消息已经落库。
    loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), handle.events.recv())
            .await
            .expect("event timeout")
            .expect("channel closed");
        if matches!(ev, AgentEvent::ToolStarted { .. }) {
            break;
        }
    }

    handle.commands.send(AgentCommand::Cancel).unwrap();
    loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), handle.events.recv())
            .await
            .expect("cancel timeout")
            .expect("channel closed");
        if matches!(ev, AgentEvent::Cancelled) {
            break;
        }
    }

    // 取消后再次提交：provider 会记录第二轮看到的历史，配对必须完整。
    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "continue".into(),
        })
        .unwrap();
    loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), handle.events.recv())
            .await
            .expect("second turn timeout")
            .expect("channel closed");
        if matches!(ev, AgentEvent::TurnComplete { .. }) {
            break;
        }
    }
    handle.commands.send(AgentCommand::Shutdown).unwrap();

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2, "两轮请求都应到达 provider");
    let history = &requests[1];
    let assistant = history
        .iter()
        .position(|m| {
            m.role == mrorcode_kernel::message::Role::Assistant && m.tool_calls.len() == 2
        })
        .expect("assistant tool_calls 消息应保留");
    let calls: Vec<&str> = history[assistant]
        .tool_calls
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    let results: Vec<(&str, &str)> = history[assistant + 1..]
        .iter()
        .take_while(|m| m.role == mrorcode_kernel::message::Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m.text.as_str())))
        .collect();
    assert_eq!(results.len(), 2, "每个 tool_call 都应有结果");
    for (idx, id) in calls.iter().enumerate() {
        assert_eq!(results[idx].0, *id);
        assert!(
            results[idx].1.contains("cancelled"),
            "取消的 tool 结果应标记 cancelled：{}",
            results[idx].1
        );
    }
}

/// 记录每次请求消息数与模型覆盖的 provider。
type RecCalls = Arc<tokio::sync::Mutex<Vec<(usize, Option<String>)>>>;

#[derive(Clone)]
struct RecordingProvider {
    calls: RecCalls,
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    fn model_name(&self) -> &str {
        "base-model"
    }

    fn chat_stream<'a>(
        &'a self,
        _messages: &'a [mrorcode_kernel::Message],
        _tools: &'a [mrorcode_kernel::ToolDef],
        options: &'a ChatOptions,
        _cancel: &'a tokio_util::sync::CancellationToken,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = ProviderResult<BoxStream<'a, StreamEvent>>>
                + Send
                + 'a,
        >,
    > {
        let calls = Arc::clone(&self.calls);
        let n = _messages.len();
        let model = options.model.clone();
        Box::pin(async move {
            calls.lock().await.push((n, model));
            Ok(Box::pin(futures::stream::once(async {
                StreamEvent::TextDelta("ok".into())
            })) as BoxStream<'a, StreamEvent>)
        })
    }
}

async fn submit_and_wait(handle: &mut mrorcode_kernel::AgentHandle, session_id: u64, text: &str) {
    handle
        .commands
        .send(AgentCommand::Submit {
            session_id,
            text: text.into(),
        })
        .unwrap();
    let _ = drain_until_terminal(handle).await;
}

#[tokio::test]
async fn sessions_keep_isolated_conversations() {
    let provider = RecordingProvider {
        calls: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };
    let calls = Arc::clone(&provider.calls);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .system_prompt("sys")
        .spawn();

    // 会话1：system + user1 → 2 条
    submit_and_wait(&mut handle, 1, "from-one").await;
    // 会话2：system + user2（不应包含 from-one）→ 2 条
    submit_and_wait(&mut handle, 2, "from-two").await;
    // 回到会话1：system + user1 + assistant1 + user3 → 4 条（不含 from-two）
    submit_and_wait(&mut handle, 1, "again-one").await;

    let recorded = calls.lock().await.clone();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[0].0, 2);
    assert_eq!(recorded[1].0, 2);
    assert_eq!(recorded[2].0, 4);
}

#[tokio::test]
async fn set_model_takes_effect_on_next_turn() {
    let provider = RecordingProvider {
        calls: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };
    let calls = Arc::clone(&provider.calls);
    let mut handle = AgentBuilder::new().provider(Arc::new(provider)).spawn();

    submit_and_wait(&mut handle, 1, "first").await;
    handle
        .commands
        .send(AgentCommand::SetModel("other-model".into()))
        .unwrap();
    submit_and_wait(&mut handle, 1, "second").await;

    let recorded = calls.lock().await.clone();
    assert_eq!(recorded[0].1, None);
    assert_eq!(recorded[1].1.as_deref(), Some("other-model"));
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
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "big".into(),
        })
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

#[tokio::test]
async fn loaded_session_continues_context() {
    let provider = RecordingProvider {
        calls: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };
    let calls = Arc::clone(&provider.calls);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .system_prompt("sys")
        .spawn();

    // 注入一个含 2 条历史的会话（无 system），再 submit。
    let mut convo = mrorcode_kernel::Conversation::new();
    convo.push(mrorcode_kernel::Message::user("old-question"));
    convo.push(mrorcode_kernel::Message::assistant_text("old-answer"));
    handle
        .commands
        .send(AgentCommand::LoadSession {
            session_id: 5,
            conversation: convo,
        })
        .unwrap();
    submit_and_wait(&mut handle, 5, "new-question").await;

    let recorded = calls.lock().await.clone();
    // 2 条历史 + 新 user = 3 条
    assert_eq!(recorded[0].0, 3);
}

#[tokio::test]
async fn rewind_truncates_loaded_session_turns() {
    let provider = RecordingProvider {
        calls: Arc::new(tokio::sync::Mutex::new(Vec::new())),
    };
    let calls = Arc::clone(&provider.calls);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .system_prompt("sys")
        .spawn();

    // 注入含 3 个 turn 的历史（无 system）。
    let mut convo = mrorcode_kernel::Conversation::new();
    for i in 0..3 {
        convo.push(mrorcode_kernel::Message::user(format!("q{i}")));
        convo.push(mrorcode_kernel::Message::assistant_text(format!("a{i}")));
    }
    handle
        .commands
        .send(AgentCommand::LoadSession {
            session_id: 7,
            conversation: convo,
        })
        .unwrap();
    // 回退到保留最早 1 个 turn（q0/a0），再 submit。
    handle
        .commands
        .send(AgentCommand::Rewind {
            session_id: 7,
            keep_turns: 1,
        })
        .unwrap();
    submit_and_wait(&mut handle, 7, "new-question").await;

    let recorded = calls.lock().await.clone();
    // q0/a0（2 条保留）+ 新 user = 3 条（注入的会话不带 system 前缀）
    assert_eq!(recorded[0].0, 3, "message count after rewind + submit");
}

#[tokio::test]
async fn read_only_mode_blocks_risky_tools() {
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(RiskyTool));
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("", "c1", "risky", json!({})),
        text_script("done"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(reg)
        .spawn();

    handle
        .commands
        .send(AgentCommand::SetMode(PermissionMode::ReadOnly))
        .unwrap();
    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "edit something".into(),
        })
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    let result = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult { name, content, .. } if name == "risky" => Some(content),
            _ => None,
        })
        .expect("risky result");
    assert!(result.contains("read-only mode"), "{result}");
}

#[tokio::test]
async fn read_only_mode_blocks_workspace_write_tools() {
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(WorkspaceWriteTool));
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("", "c1", "ws_write", json!({})),
        text_script("done"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(reg)
        .spawn();

    handle
        .commands
        .send(AgentCommand::SetMode(PermissionMode::ReadOnly))
        .unwrap();
    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "write something".into(),
        })
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    let result = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult { name, content, .. } if name == "ws_write" => Some(content),
            _ => None,
        })
        .expect("ws_write result");
    assert!(result.contains("read-only mode"), "{result}");
}

#[tokio::test]
async fn read_only_mode_allows_safe_tools() {
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(BigTool { size: 4 }));
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("", "b1", "big", json!({})),
        text_script("done"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(reg)
        .spawn();

    handle
        .commands
        .send(AgentCommand::SetMode(PermissionMode::ReadOnly))
        .unwrap();
    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "read".into(),
        })
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    // BigTool 为 Safe：应正常执行而非被只读 gate 拒绝
    let result = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult { name, content, .. } if name == "big" => Some(content),
            _ => None,
        })
        .expect("big result");
    assert!(!result.contains("read-only mode"));
}

#[tokio::test]
async fn workspace_mode_allows_workspace_write_without_approval() {
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(WorkspaceWriteTool));
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("", "c1", "ws_write", json!({})),
        text_script("done"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(reg)
        .add_middleware(Arc::new(DenyAll)) // 若走审批/middleware 即被拒
        .spawn();

    handle
        .commands
        .send(AgentCommand::SetMode(PermissionMode::Workspace))
        .unwrap();
    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "write something".into(),
        })
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    // WorkspaceWrite 在工作区读写模式下直行，不经过 middleware
    let result = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult { name, content, .. } if name == "ws_write" => Some(content),
            _ => None,
        })
        .expect("ws_write result");
    assert_eq!(result, "wrote");
}

#[tokio::test]
async fn full_mode_skips_approval_middleware() {
    let mut reg = ToolRegistry::new();
    reg.insert(Arc::new(RiskyTool));
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("", "c1", "risky", json!({})),
        text_script("done"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(reg)
        .add_middleware(Arc::new(DenyAll)) // 完全访问下应被跳过
        .spawn();

    handle
        .commands
        .send(AgentCommand::SetMode(PermissionMode::Full))
        .unwrap();
    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "do anything".into(),
        })
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    let result = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolResult { name, content, .. } if name == "risky" => Some(content),
            _ => None,
        })
        .expect("risky result");
    assert_eq!(result, "wrote");
}

// ---------------- conformance 套件（M5-d） ----------------
// 标准场景的事件类型序列断言：对任意 provider 实现（OpenAI 兼容 / Anthropic），
// agent 循环消费 StreamEvent 后产出的 AgentEvent 序列语义一致。

#[tokio::test]
async fn conformance_plain_chat_event_sequence() {
    let provider = ScriptedProvider::new(vec![text_script("hello")]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(registry_with_echo())
        .spawn();

    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "hi".into(),
        })
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    assert_event_types(&events, &["TurnStarted", "TextDelta", "TurnComplete"]);
}

#[tokio::test]
async fn conformance_tool_call_event_sequence() {
    let provider = ScriptedProvider::new(vec![
        single_tool_call_script("let me check", "c1", "echo", json!({"text": "hi"})),
        text_script("done"),
    ]);
    let mut handle = AgentBuilder::new()
        .provider(Arc::new(provider))
        .tools(registry_with_echo())
        .spawn();

    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "use echo".into(),
        })
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    // TurnStarted → (文本) → ToolStarted → ToolResult → (收尾文本) → TurnComplete
    assert_event_types(
        &events,
        &[
            "TurnStarted",
            "TextDelta",
            "ToolStarted",
            "ToolResult",
            "TextDelta",
            "TurnComplete",
        ],
    );
}

#[tokio::test]
async fn conformance_error_event_sequence() {
    use mrorcode_kernel::provider::ProviderError;
    use mrorcode_kernel::StreamEvent;

    // 发出一个可恢复错误后不再产出（模拟 provider 失败）
    struct ErrorProvider;
    #[async_trait]
    impl LlmProvider for ErrorProvider {
        fn model_name(&self) -> &str {
            "err"
        }
        fn chat_stream<'a>(
            &'a self,
            _messages: &'a [mrorcode_kernel::Message],
            _tools: &'a [mrorcode_kernel::ToolDef],
            _options: &'a ChatOptions,
            _cancel: &'a tokio_util::sync::CancellationToken,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = ProviderResult<BoxStream<'a, StreamEvent>>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async {
                Ok(
                    Box::pin(futures::stream::iter([StreamEvent::Error(ProviderError {
                        message: "boom".into(),
                        retryable: false,
                        http_status: None,
                        code: None,
                    })])) as BoxStream<'a, StreamEvent>,
                )
            })
        }
    }

    let mut handle = AgentBuilder::new()
        .provider(Arc::new(ErrorProvider))
        .tools(registry_with_echo())
        .spawn();

    handle
        .commands
        .send(AgentCommand::Submit {
            session_id: 1,
            text: "go".into(),
        })
        .unwrap();
    let events = drain_until_terminal(&mut handle).await;

    assert_event_types(&events, &["TurnStarted", "Error", "TurnComplete"]);
}
