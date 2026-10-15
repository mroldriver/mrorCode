//! 工具 trait、执行上下文与注册表。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::event::AgentEvent;
use crate::request::Requester;

/// 风险等级：只是建议元数据，强制力在 middleware。默认按 Risky 对待（安全默认）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// 纯只读，任何模式都可直接执行。
    Safe,
    /// 限定在工作目录内的写（edit/write 工具）；只读模式下仍被拒绝。
    WorkspaceWrite,
    /// 副作用超出工作区（bash 写命令、网络、子 agent 等），需审批。
    Risky,
}

/// 工具执行结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            is_error: true,
        }
    }
}

/// 发给模型的工具定义（OpenAI function schema 形态）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// 工具执行时拿到的最小上下文。
pub struct ToolContext<'a> {
    pub working_dir: &'a Path,
    pub cancel: &'a CancellationToken,
    pub request: &'a Requester,
    /// 当前工具调用的 id（agent 权威分配），用于子 agent 事件投影定位。
    pub tool_id: &'a str,
    /// 子 agent 事件转发通道；task 等嵌套工具通过它把内部事件实时投影到主会话。
    /// None 表示调用方不关心子事件（普通工具无需设置）。
    pub event_sink: Option<&'a UnboundedSender<AgentEvent>>,
}

/// 工具实现。实现者绝不 panic：错误以 `is_error=true` 的结果返回。
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;

    /// 风险建议。安全默认返回 Risky，只读工具覆盖为 Safe，工作区写覆盖为 WorkspaceWrite。
    fn risk(&self, _arguments: &Value) -> RiskLevel {
        RiskLevel::Risky
    }

    async fn execute(&self, arguments: &str, ctx: &ToolContext<'_>) -> ToolResult;

    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

/// 工具注册表（BTreeMap 保证确定性顺序）。
/// 深层工具对象是 `Arc`，因此可直接 `Clone`（浅拷贝共享同一工具实例）。
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    pub fn defs(&self) -> Vec<ToolDef> {
        self.tools.values().map(|t| t.def()).collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
