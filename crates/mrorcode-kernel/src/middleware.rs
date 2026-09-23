//! 工具中间件（审批、参数修复等）。
//!
//! 注册顺序即执行顺序。约定：参数修复类中间件必须排在审批类之前。

use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::message::ToolCall;
use crate::request::Requester;
use crate::tool::{RiskLevel, Tool, ToolResult};

/// `before` 的裁决。
#[derive(Clone, Debug)]
pub enum BeforeOutcome {
    /// 继续执行。
    Proceed,
    /// 显式放行（如 "always grant"，与 Proceed 一样执行，区别仅在语义记录）。
    Allow { reason: String },
    /// 拒绝执行；原因将作为工具结果回灌模型。
    Deny { reason: String },
}

/// `after` 的裁决（M1 仅透传，预留扩展）。
#[derive(Clone, Debug, Default)]
pub enum AfterOutcome {
    #[default]
    Proceed,
}

/// 工具中间件。用内部可变性保存授权状态等。
#[async_trait::async_trait]
pub trait ToolMiddleware: Send + Sync {
    /// 工具执行前。可改写 `call.arguments`。
    async fn before(
        &self,
        _call: &mut ToolCall,
        _risk: RiskLevel,
        _tool: &Arc<dyn Tool>,
        _request: &Requester,
        _cancel: &CancellationToken,
    ) -> BeforeOutcome {
        BeforeOutcome::Proceed
    }

    /// 工具执行后。
    async fn after(
        &self,
        _result: &mut ToolResult,
        _tool: Option<&Arc<dyn Tool>>,
    ) -> AfterOutcome {
        AfterOutcome::Proceed
    }
}

/// 把拒绝裁决转成回灌给模型的 JSON 工具结果内容。
pub fn denial_payload(reason: &str) -> Value {
    json!({ "status": "denied", "reason": reason })
}
