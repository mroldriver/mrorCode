//! 中立 agent 执行循环。
//!
//! 单 owner task 驱动一轮轮对话：provider 流式响应 → 累积工具调用 →
//! middleware 审批 → 顺序执行工具（M1 不并发）→ 结果回灌 → 下一轮，
//! 直到模型不再请求工具或触及 max_rounds 保险丝。
//!
//! 会话所有权在轮次开始时移入 turn task、结束后移回，保证任何时刻只有一份；
//! Cancel/Respond 与轮次执行在同一个 `select!` 中竞速。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::event::{AgentCommand, AgentEvent, StopReason};
use crate::message::{Conversation, Message, ToolCall};
use crate::middleware::{BeforeOutcome, ToolMiddleware};
use crate::provider::{ChatOptions, LlmProvider, StreamEvent};
use crate::request::RequestBroker;
use crate::tool::{Tool, ToolContext, ToolRegistry, ToolResult};

/// 工具结果中央截断阈值（head + tail）。
pub const DEFAULT_MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;
/// 每轮最大 round 数保险丝。
pub const DEFAULT_MAX_ROUNDS: usize = 25;

/// 上下文压缩策略（M3）。阈值 0 表示关闭。
#[derive(Clone, Copy, Debug)]
pub struct CompactionPolicy {
    /// 估算 token 超过该值时在下一轮开始前压缩。
    pub threshold_tokens: usize,
    /// 保留最近多少个 user turn 原文。
    pub keep_recent: usize,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            threshold_tokens: 100_000,
            keep_recent: 6,
        }
    }
}

impl CompactionPolicy {
    pub fn is_disabled(&self) -> bool {
        self.threshold_tokens == 0
    }
}

/// 与 agent 通信的句柄。`events` 由唯一 driver 持有轮询。
pub struct AgentHandle {
    pub commands: UnboundedSender<AgentCommand>,
    pub events: UnboundedReceiver<AgentEvent>,
}

/// agent 构建器。
pub struct AgentBuilder {
    provider: Option<Arc<dyn LlmProvider>>,
    tools: ToolRegistry,
    middleware: Vec<Arc<dyn ToolMiddleware>>,
    max_rounds: usize,
    max_tool_result_bytes: usize,
    working_dir: PathBuf,
    chat_options: ChatOptions,
    system_prompt: Option<String>,
    compaction: CompactionPolicy,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            provider: None,
            tools: ToolRegistry::new(),
            middleware: Vec::new(),
            max_rounds: DEFAULT_MAX_ROUNDS,
            max_tool_result_bytes: DEFAULT_MAX_TOOL_RESULT_BYTES,
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            chat_options: ChatOptions::default(),
            system_prompt: None,
            compaction: CompactionPolicy::default(),
        }
    }

    /// 设置 system prompt（作为会话首条 system 消息，仅一次）。
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn provider(mut self, provider: Arc<dyn LlmProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    pub fn add_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.insert(tool);
        self
    }

    pub fn add_middleware(mut self, mw: Arc<dyn ToolMiddleware>) -> Self {
        self.middleware.push(mw);
        self
    }

    pub fn max_rounds(mut self, n: usize) -> Self {
        self.max_rounds = n;
        self
    }

    /// 覆盖工具结果中央截断阈值（测试用）。
    pub fn max_tool_result_bytes(mut self, n: usize) -> Self {
        self.max_tool_result_bytes = n;
        self
    }

    pub fn working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = dir;
        self
    }

    pub fn chat_options(mut self, options: ChatOptions) -> Self {
        self.chat_options = options;
        self
    }

    /// 上下文压缩策略。
    pub fn compaction(mut self, policy: CompactionPolicy) -> Self {
        self.compaction = policy;
        self
    }

    /// 启动 agent owner task，返回控制句柄。
    pub fn spawn(self) -> AgentHandle {
        let provider = self
            .provider
            .expect("AgentBuilder::spawn: provider is required");
        let (cmd_tx, cmd_rx) = unbounded_channel::<AgentCommand>();
        let (evt_tx, evt_rx) = unbounded_channel::<AgentEvent>();

        let mut state = AgentState {
            provider,
            tools: Arc::new(self.tools),
            middleware: self.middleware,
            conversations: HashMap::new(),
            system_prompt: self.system_prompt,
            working_dir: self.working_dir,
            max_rounds: self.max_rounds,
            max_tool_result_bytes: self.max_tool_result_bytes,
            chat_options: self.chat_options,
            compaction: self.compaction,
            mode: crate::event::PermissionMode::default(),
        };
        tokio::spawn(async move {
            run_agent(&mut state, cmd_rx, evt_tx).await;
        });

        AgentHandle {
            commands: cmd_tx,
            events: evt_rx,
        }
    }
}

struct AgentState {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    middleware: Vec<Arc<dyn ToolMiddleware>>,
    /// 多会话会话体（M2 仅内存；M3 接磁盘持久化）。
    conversations: HashMap<u64, Conversation>,
    /// 每个新会话的 system prompt（仅首次写入）。
    system_prompt: Option<String>,
    working_dir: PathBuf,
    max_rounds: usize,
    max_tool_result_bytes: usize,
    chat_options: ChatOptions,
    compaction: CompactionPolicy,
    mode: crate::event::PermissionMode,
}

impl AgentState {
    /// 取走（或新建）一个会话的会话体，供 turn 持有。
    fn take_conversation(&mut self, session_id: u64) -> Conversation {
        self.conversations.remove(&session_id).unwrap_or_else(|| {
            let mut convo = Conversation::new();
            if let Some(system) = &self.system_prompt {
                convo.push(Message::system(system.clone()));
            }
            convo
        })
    }
}

/// 正在运行的轮次：所属会话 + 取消令牌 + 结束时归还会话的 join handle。
struct RunningTurn {
    session_id: u64,
    cancel: CancellationToken,
    done: tokio::task::JoinHandle<Conversation>,
}

async fn run_agent(
    state: &mut AgentState,
    mut cmd_rx: UnboundedReceiver<AgentCommand>,
    evt_tx: UnboundedSender<AgentEvent>,
) {
    let broker = RequestBroker::new(evt_tx.clone());
    let mut running: Option<RunningTurn> = None;
    // busy 时按 FIFO 排队（含 session_id）
    let mut pending: std::collections::VecDeque<(u64, String)> = std::collections::VecDeque::new();

    let start_turn = |state: &mut AgentState,
                      broker: &RequestBroker,
                      events: UnboundedSender<AgentEvent>,
                      session_id: u64,
                      text: String|
     -> RunningTurn {
        let cancel = CancellationToken::new();
        let conversation = state.take_conversation(session_id);
        let turn = TurnCtx {
            provider: Arc::clone(&state.provider),
            tools: Arc::clone(&state.tools),
            middleware: state.middleware.clone(),
            conversation,
            working_dir: state.working_dir.clone(),
            max_rounds: state.max_rounds,
            max_tool_result_bytes: state.max_tool_result_bytes,
            chat_options: state.chat_options.clone(),
            compaction: state.compaction,
            mode: state.mode,
            requester: broker.handle(),
            events,
            cancel: cancel.clone(),
            incoming: text,
        };
        let done = tokio::spawn(async move {
            let mut turn = turn;
            turn.run().await;
            turn.conversation
        });
        RunningTurn {
            session_id,
            cancel,
            done,
        }
    };

    loop {
        tokio::select! {
            biased;

            cmd = cmd_rx.recv() => {
                match cmd {
                    None | Some(AgentCommand::Shutdown) => {
                        if let Some(rt) = running.take() {
                            rt.cancel.cancel();
                            let _ = rt.done.await;
                        }
                        broker.fail_all();
                        break;
                    }
                    Some(AgentCommand::Respond(id, value)) => {
                        broker.resolve(id, value);
                    }
                    Some(AgentCommand::Cancel) => {
                        if let Some(rt) = running.as_ref() {
                            rt.cancel.cancel();
                            broker.fail_all();
                        }
                    }
                    Some(AgentCommand::SetModel(model)) => {
                        state.chat_options.model = Some(model);
                    }
                    Some(AgentCommand::SetMode(mode)) => {
                        state.mode = mode;
                    }
                    Some(AgentCommand::LoadSession { session_id, conversation }) => {
                        state.conversations.insert(session_id, conversation);
                    }
                    Some(AgentCommand::Rewind {
                        session_id,
                        keep_turns,
                    }) => {
                        // 仅空闲时生效；running 中（或排队中）忽略，避免与进行中的对话竞争
                        if running.is_none() && pending.is_empty() {
                            if let Some(convo) = state.conversations.get_mut(&session_id) {
                                let removed = convo.rewind(keep_turns);
                                if removed > 0 {
                                    tracing::info!(
                                        session_id,
                                        removed,
                                        "rewound conversation to keep_turns={keep_turns}"
                                    );
                                }
                            }
                        }
                    }
                    Some(AgentCommand::Submit { session_id, text }) => {
                        if running.is_some() {
                            // 轮次运行中：FIFO 排队（M2 不做跨会话并发）
                            pending.push_back((session_id, text));
                        } else {
                            running =
                                Some(start_turn(state, &broker, evt_tx.clone(), session_id, text));
                        }
                    }
                }
            }

            returned = async {
                match running.as_mut() {
                    Some(rt) => (&mut rt.done).await,
                    None => std::future::pending().await,
                }
            }, if running.is_some() => {
                let session_id = running.as_ref().map(|rt| rt.session_id);
                running = None;
                broker.fail_all();
                if let (Some(session_id), Ok(conversation)) = (session_id, returned) {
                    state.conversations.insert(session_id, conversation);
                }
                if let Some((next_session, next_text)) = pending.pop_front() {
                    running = Some(start_turn(
                        state,
                        &broker,
                        evt_tx.clone(),
                        next_session,
                        next_text,
                    ));
                }
            }
        }
    }
}

/// 一个 round 的流式累积结果。
struct Accumulated {
    text: String,
    reasoning: String,
    calls: Vec<ToolCall>,
    input_tokens: u64,
    output_tokens: u64,
}

struct TurnCtx {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    middleware: Vec<Arc<dyn ToolMiddleware>>,
    conversation: Conversation,
    working_dir: PathBuf,
    max_rounds: usize,
    max_tool_result_bytes: usize,
    chat_options: ChatOptions,
    compaction: CompactionPolicy,
    mode: crate::event::PermissionMode,
    requester: crate::request::Requester,
    events: UnboundedSender<AgentEvent>,
    cancel: CancellationToken,
    incoming: String,
}

impl TurnCtx {
    fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    async fn run(&mut self) {
        self.emit(AgentEvent::TurnStarted);
        self.conversation.push(Message::user(self.incoming.clone()));

        // 上下文压缩：超阈值时在发起请求前折叠较早轮次（纯函数，driver 不参与）。
        if !self.compaction.is_disabled()
            && self.conversation.estimate_tokens() > self.compaction.threshold_tokens
        {
            let removed = self.conversation.compact(self.compaction.keep_recent);
            if removed > 0 {
                tracing::info!(removed, "compacted conversation before turn");
            }
        }

        let tool_defs = self.tools.defs();

        for _round in 0..self.max_rounds {
            if self.cancel.is_cancelled() {
                self.emit(AgentEvent::Cancelled);
                return;
            }

            // ---- 发起流式请求并累积事件 ----
            // 独立作用域：流借用了 conversation.messages，必须在追加消息前析构。
            #[allow(clippy::needless_late_init)]
            let accumulated: Accumulated;
            {
                let result = tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => {
                        self.emit(AgentEvent::Cancelled);
                        return;
                    }
                    result = self.provider.chat_stream(
                        &self.conversation.messages,
                        &tool_defs,
                        &self.chat_options,
                        &self.cancel,
                    ) => result,
                };
                let mut stream = match result {
                    Ok(s) => s,
                    Err(e) => {
                        self.emit(AgentEvent::Error {
                            message: e.message.clone(),
                            retryable: e.retryable,
                        });
                        self.emit(AgentEvent::TurnComplete {
                            stop_reason: StopReason::ProviderError,
                        });
                        return;
                    }
                };

                let mut text = String::new();
                let mut reasoning = String::new();
                let mut calls: Vec<ToolCall> = Vec::new();
                let mut call_index: HashMap<String, usize> = HashMap::new();
                let mut input_tokens = 0u64;
                let mut output_tokens = 0u64;
                let mut stream_error = false;

                while let Some(ev) = {
                    tokio::select! {
                        biased;
                        _ = self.cancel.cancelled() => {
                            self.emit(AgentEvent::Cancelled);
                            return;
                        }
                        ev = stream.next() => ev,
                    }
                } {
                    match ev {
                        StreamEvent::TextDelta(t) => {
                            text.push_str(&t);
                            self.emit(AgentEvent::TextDelta(t));
                        }
                        StreamEvent::Reasoning(t) => {
                            reasoning.push_str(&t);
                            self.emit(AgentEvent::Reasoning(t));
                        }
                        StreamEvent::ToolCallStarted { id, name } => {
                            call_index.insert(id.clone(), calls.len());
                            calls.push(ToolCall::new(id, name));
                        }
                        StreamEvent::ToolCallDelta { id, arguments } => {
                            if let Some(i) = call_index.get(&id) {
                                calls[*i].arguments.push_str(&arguments);
                            }
                        }
                        StreamEvent::Usage {
                            input_tokens: i,
                            output_tokens: o,
                        } => {
                            input_tokens = input_tokens.max(i);
                            output_tokens = output_tokens.max(o);
                        }
                        StreamEvent::Error(e) => {
                            stream_error = true;
                            self.emit(AgentEvent::Error {
                                message: e.message,
                                retryable: e.retryable,
                            });
                            self.emit(AgentEvent::TurnComplete {
                                stop_reason: StopReason::ProviderError,
                            });
                        }
                    }
                }
                if stream_error {
                    return;
                }
                accumulated = Accumulated {
                    text,
                    reasoning,
                    calls,
                    input_tokens,
                    output_tokens,
                };
            }
            let Accumulated {
                text,
                reasoning,
                calls,
                input_tokens,
                output_tokens,
            } = accumulated;

            // ---- 助手消息落库 ----
            if !text.is_empty() || !reasoning.is_empty() || !calls.is_empty() {
                let mut msg = Message::assistant_text(text);
                msg.tool_calls = calls.clone();
                if !reasoning.is_empty() {
                    msg.reasoning = Some(reasoning);
                }
                msg.meta.input_tokens = Some(input_tokens).filter(|v| *v > 0);
                msg.meta.output_tokens = Some(output_tokens).filter(|v| *v > 0);
                if input_tokens + output_tokens > 0 {
                    self.emit(AgentEvent::Usage {
                        input_tokens,
                        output_tokens,
                    });
                }
                self.conversation.push(msg);
            }

            if calls.is_empty() {
                self.emit(AgentEvent::TurnComplete {
                    stop_reason: StopReason::Stopped,
                });
                return;
            }

            // ---- 顺序执行工具 ----
            for call in &calls {
                if self.cancel.is_cancelled() {
                    self.emit(AgentEvent::Cancelled);
                    return;
                }

                let tool = match self.tools.get(&call.name) {
                    Some(t) => Arc::clone(t),
                    None => {
                        let result = ToolResult::error(
                            call.id.clone(),
                            json!({
                                "status": "error",
                                "reason": format!("unknown tool: {}", call.name),
                            })
                            .to_string(),
                        );
                        self.emit_tool_result(&call.name, &result);
                        self.conversation.push(Message::tool_result(&result));
                        continue;
                    }
                };

                let args_value: Value =
                    serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
                let risk = tool.risk(&args_value);
                let mut live_call = call.clone();

                // 权限 gate（优先于审批，不弹窗）：
                // 只读模式拒绝一切非 Safe 工具；工作区读写放行 WorkspaceWrite，
                // Risky 交给审批中间件；完全访问不做 gate。
                let mode_blocked = match (self.mode, risk) {
                    (crate::event::PermissionMode::ReadOnly, crate::tool::RiskLevel::Safe) => false,
                    (crate::event::PermissionMode::ReadOnly, _) => true,
                    (crate::event::PermissionMode::Workspace, crate::tool::RiskLevel::Risky) => {
                        false
                    }
                    (crate::event::PermissionMode::Workspace, _) => false,
                    (crate::event::PermissionMode::Full, _) => false,
                };
                if mode_blocked {
                    let preview = one_line_preview(&live_call.arguments);
                    self.emit(AgentEvent::ToolStarted {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: live_call.arguments.clone(),
                    });
                    let mut result = ToolResult::error(
                        call.id.clone(),
                        json!({
                            "status": "blocked",
                            "reason": format!("read-only mode: `{}` is a write operation; only read-only tools are allowed ({preview})", call.name),
                        })
                        .to_string(),
                    );
                    result.content =
                        truncate_tool_result(&result.content, self.max_tool_result_bytes);
                    self.emit_tool_result(&call.name, &result);
                    self.conversation.push(Message::tool_result(&result));
                    continue;
                }

                // 完全访问：跳过审批等 middleware（用户已显式授权全部操作）。
                let mut denied: Option<String> = None;
                if self.mode != crate::event::PermissionMode::Full {
                    for mw in &self.middleware {
                        match mw
                            .before(&mut live_call, risk, &tool, &self.requester, &self.cancel)
                            .await
                        {
                            BeforeOutcome::Proceed | BeforeOutcome::Allow { .. } => continue,
                            BeforeOutcome::Deny { reason } => {
                                denied = Some(reason);
                                break;
                            }
                        }
                    }
                }

                self.emit(AgentEvent::ToolStarted {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: live_call.arguments.clone(),
                });

                let mut result = if let Some(reason) = denied {
                    ToolResult::error(
                        call.id.clone(),
                        json!({"status": "denied", "reason": reason}).to_string(),
                    )
                } else {
                    let ctx = ToolContext {
                        working_dir: &self.working_dir,
                        cancel: &self.cancel,
                        request: &self.requester,
                        tool_id: &call.id,
                        event_sink: Some(&self.events),
                    };
                    let executed = tokio::select! {
                        biased;
                        _ = self.cancel.cancelled() => None,
                        r = tool.execute(&live_call.arguments, &ctx) => Some(r),
                    };
                    match executed {
                        None => {
                            self.emit(AgentEvent::Cancelled);
                            return;
                        }
                        Some(mut result) => {
                            // 调用 id 由 agent 权威指定，工具返回值不可信
                            result.call_id = call.id.clone();
                            for mw in &self.middleware {
                                mw.after(&mut result, Some(&tool)).await;
                            }
                            result
                        }
                    }
                };

                result.content = truncate_tool_result(&result.content, self.max_tool_result_bytes);
                self.emit_tool_result(&call.name, &result);
                self.conversation.push(Message::tool_result(&result));
            }
            // 工具结果已回灌，进入下一 round
        }

        // 超过 max_rounds
        self.emit(AgentEvent::TurnComplete {
            stop_reason: StopReason::MaxRounds,
        });
    }

    fn emit_tool_result(&self, name: &str, result: &ToolResult) {
        self.emit(AgentEvent::ToolResult {
            id: result.call_id.clone(),
            name: name.to_string(),
            content: result.content.clone(),
            is_error: result.is_error,
        });
    }
}

/// 不超过 `idx` 的最大 UTF-8 字符边界（兼容 MSRV 1.88；std 方法 1.91 才稳定）。
fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// plan gate 提示用的单行参数摘要。
fn one_line_preview(arguments: &str) -> String {
    let parsed: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let text = parsed
        .get("command")
        .and_then(|v| v.as_str())
        .or_else(|| parsed.get("path").and_then(|v| v.as_str()))
        .unwrap_or(arguments);
    text.chars().take(80).collect()
}

/// head+tail 中央截断，保证不切断 UTF-8 字符。
fn truncate_tool_result(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    let budget = max_bytes.saturating_sub(96) / 2;
    let head = floor_char_boundary(content, budget);
    let tail_from = floor_char_boundary(content, content.len() - budget);
    let omitted = tail_from - head;
    format!(
        "{}\n...[{} bytes truncated]...\n{}",
        &content[..head],
        omitted,
        &content[tail_from..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::RiskLevel;

    #[test]
    fn truncate_head_tail_keeps_boundaries() {
        let s = "a".repeat(10_000);
        let out = truncate_tool_result(&s, 1024);
        assert!(out.contains("truncated"));
        assert!(out.len() < 1100);
    }

    #[test]
    fn truncate_unicode_safe() {
        let s = "中".repeat(10_000);
        let out = truncate_tool_result(&s, 2048);
        assert!(out.contains("truncated"));
        // 构造出的字符串必须仍然合法
        let _ = out.chars().count();
    }

    #[test]
    fn default_risk_is_safe_constant() {
        // 编译期保障：RiskLevel 可比较
        assert_ne!(RiskLevel::Safe, RiskLevel::Risky);
    }
}
