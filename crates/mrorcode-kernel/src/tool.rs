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
        self.tools
            .values()
            .map(|t| {
                let mut def = t.def();
                def.parameters = with_intent(def.parameters);
                def
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// 每个工具调用都自带的意图参数名。
///
/// 模型用一句话说明「为什么要调这个工具」，这一行会原样显示成工具卡的首行
/// （参见 `crates/mrorcode-cli/src/repl.rs`）。放在 `defs()` 这个唯一出口注入，
/// 内置工具、MCP 工具、将来新增的工具自动享有，各工具无需自己声明。
pub const INTENT_PARAM: &str = "intent";

/// `intent` 参数的说明；措辞直接决定模型写不写、写得有没有用。
const INTENT_DESCRIPTION: &str = "One short sentence in the user's language saying what you are \
trying to accomplish with this call. It is shown to the person as the title line of the tool \
card, so state the goal, not the mechanics: no tool name, no raw parameters.";

/// 把 `intent` 参数注入到工具 schema 里（可选参数，不进入 `required`）。
///
/// - 工具自己声明过 `intent` 就保留原样（MCP server 可能有同名参数）；
/// - schema 不是带 `properties` 的对象时原样返回（别为了显示去破坏别人的契约）。
fn with_intent(mut parameters: Value) -> Value {
    let Some(obj) = parameters.as_object_mut() else {
        return parameters;
    };
    let props = obj
        .entry("properties")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(props) = props.as_object_mut() {
        props.entry(INTENT_PARAM).or_insert_with(|| {
            serde_json::json!({
                "type": "string",
                "description": INTENT_DESCRIPTION,
            })
        });
    }
    parameters
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只用来验 schema 注入的最小工具；execute 不会被测试调用。
    struct FakeTool {
        params: Value,
    }

    #[async_trait::async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            "fake"
        }
        fn description(&self) -> &str {
            "fake tool"
        }
        fn parameters_schema(&self) -> Value {
            self.params.clone()
        }
        async fn execute(&self, _arguments: &str, _ctx: &ToolContext<'_>) -> ToolResult {
            ToolResult::ok("fake", "ok")
        }
    }

    fn defs_of(params: Value) -> Value {
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(FakeTool { params }));
        reg.defs()[0].parameters.clone()
    }

    #[test]
    fn defs_inject_intent_and_keep_existing_params() {
        let params = defs_of(serde_json::json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"]
        }));
        // 原有参数与 required 原样保留。
        assert_eq!(params["properties"]["command"]["type"], "string");
        assert_eq!(params["required"], serde_json::json!(["command"]));
        // intent 是可选参数：进 properties，不进 required（模型漏给也不算错）。
        assert_eq!(params["properties"][INTENT_PARAM]["type"], "string");
        assert!(!params["properties"][INTENT_PARAM]["description"]
            .as_str()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn defs_do_not_clobber_tool_owned_intent() {
        // MCP server 自带同名参数时不覆盖，避免破坏别人的协议契约。
        let params = defs_of(serde_json::json!({
            "type": "object",
            "properties": { "intent": { "type": "string", "description": "mine" } }
        }));
        assert_eq!(params["properties"]["intent"]["description"], "mine");
    }

    #[test]
    fn defs_create_properties_when_schema_has_none() {
        let params = defs_of(serde_json::json!({ "type": "object" }));
        assert_eq!(params["properties"][INTENT_PARAM]["type"], "string");
    }

    #[test]
    fn defs_leave_non_object_schema_alone() {
        // 非对象 schema（异常但合法）原样返回，不为显示去打碎它。
        let params = defs_of(serde_json::json!("weird-schema"));
        assert_eq!(params, serde_json::json!("weird-schema"));
    }
}
