//! mrorcode-kernel: 中立的 agent 执行内核（L0）。
//!
//! 只定义机制（消息、provider/tool trait、事件、agent 循环），
//! 不含任何产品策略（审批规则、persona、具体 provider）。

pub mod agent;
pub mod error;
pub mod event;
pub mod message;
pub mod middleware;
pub mod provider;
pub mod request;
pub mod session;
pub mod tool;

#[cfg(any(test, feature = "test-support"))]
pub mod testkit;

pub use agent::{AgentBuilder, AgentHandle};
pub use event::{AgentCommand, AgentEvent, StopReason};
pub use message::{Conversation, Message, Role, ToolCall};
pub use middleware::{BeforeOutcome, ToolMiddleware};
pub use provider::{ChatOptions, LlmProvider, ProviderError, StreamEvent, ToolChoice};
pub use request::{RequestBroker, Requester};
pub use session::{SessionRecord, SessionSummary, TranscriptEntry};
pub use tool::{RiskLevel, Tool, ToolContext, ToolDef, ToolRegistry, ToolResult};
