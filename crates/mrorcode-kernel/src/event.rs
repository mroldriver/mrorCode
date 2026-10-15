//! agent 命令与事件边界（均可序列化，未来可跨进程）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 权限模式（三档）：决定工具执行前的 gate 策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// 只读：仅 Safe 工具可执行，其余一律拒绝（原 plan 模式）。
    ReadOnly,
    /// 工作区读写：Safe / WorkspaceWrite 直行，Risky 走审批（默认）。
    #[default]
    Workspace,
    /// 完全访问：不做 gate，也跳过审批。
    Full,
}

impl PermissionMode {
    /// 循环切换顺序：只读 → 工作区读写 → 完全访问 → 只读。
    pub fn next(self) -> Self {
        match self {
            PermissionMode::ReadOnly => PermissionMode::Workspace,
            PermissionMode::Workspace => PermissionMode::Full,
            PermissionMode::Full => PermissionMode::ReadOnly,
        }
    }

    /// 中文显示名（TUI 徽章/提示用）。
    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "只读",
            PermissionMode::Workspace => "工作区读写",
            PermissionMode::Full => "完全访问",
        }
    }
}

/// driver → agent 的命令。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentCommand {
    /// 向指定会话发送一条用户消息，开启新一轮对话。
    Submit { session_id: u64, text: String },
    /// 注入/替换一个会话的会话体（resume 从磁盘恢复时使用）。
    LoadSession {
        session_id: u64,
        conversation: crate::message::Conversation,
    },
    /// 切换模型（在当前轮次结束后的下一轮生效）。
    SetModel(String),
    /// 切换权限模式（只读 / 工作区读写 / 完全访问）。
    SetMode(PermissionMode),
    /// 回应一个 [`AgentEvent::Request`]（审批等）。
    Respond(u64, Value),
    /// 回退（rewind）：把会话截断到保留最早 `keep_turns` 个 user turn；空闲时生效（M4-9）。
    Rewind { session_id: u64, keep_turns: usize },
    /// 取消当前轮次。
    Cancel,
    /// 关闭 agent。
    Shutdown,
}

/// 一轮结束的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// 模型未再请求工具，正常结束。
    Stopped,
    /// 达到最大 round 数保险丝。
    MaxRounds,
    /// provider 错误终止。
    ProviderError,
    /// 被取消。
    Cancelled,
}

/// agent → driver 的事件。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentEvent {
    /// 新一轮开始。
    TurnStarted,
    /// 正文增量。
    TextDelta(String),
    /// 推理文本增量。
    Reasoning(String),
    /// 工具开始执行。
    ToolStarted {
        id: String,
        name: String,
        arguments: String,
    },
    /// 工具执行结束。
    ToolResult {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
    /// token 用量。
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// 请求 driver 应答（审批/提问）。
    Request {
        id: u64,
        kind: String,
        payload: Value,
    },
    /// 整轮结束（终态）。
    TurnComplete { stop_reason: StopReason },
    /// 可恢复/不可恢复错误（未终止整轮时的告警也用它）。
    Error { message: String, retryable: bool },
    /// 取消完成（终态）。
    Cancelled,
    /// 子 agent 实时事件投影（task 工具转发子 agent 的内部事件）。
    /// `parent_tool_id` 是主会话中 task 工具卡的 id，用于 TUI 定位投影目标。
    SubAgent {
        parent_tool_id: String,
        event: Box<AgentEvent>,
    },
}
