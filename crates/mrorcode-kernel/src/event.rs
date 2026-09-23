//! agent 命令与事件边界（均可序列化，未来可跨进程）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// driver → agent 的命令。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentCommand {
    /// 发送一条用户消息，开启新一轮对话。
    SendMessage(String),
    /// 回应一个 [`AgentEvent::Request`]（审批等）。
    Respond(u64, Value),
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
}
