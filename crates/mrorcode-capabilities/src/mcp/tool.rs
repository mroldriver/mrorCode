//! 把 MCP 工具桥接为 kernel `Tool`。

use async_trait::async_trait;
use serde_json::Value;

use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolResult};

use super::client::McpClient;
use super::protocol::McpToolDef;

/// 单个 MCP 工具。名字前缀 `mcp__<server>__<tool>` 避免与内置冲突。
pub struct McpTool {
    client: McpClient,
    /// 原始（未加前缀）工具名，用于 tools/call。
    remote_name: String,
    /// 暴露给模型的名字。
    qualified: String,
    description: String,
    schema: Value,
    read_only: bool,
}

impl McpTool {
    pub fn new(client: McpClient, def: McpToolDef) -> Self {
        let qualified = format!("mcp__{}__{}", client.server(), def.name);
        Self {
            remote_name: def.name,
            qualified,
            description: def.description,
            schema: def.input_schema,
            read_only: def.read_only,
            client,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.qualified
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    fn risk(&self, _arguments: &Value) -> RiskLevel {
        // readOnlyHint 声明只读才 Safe，否则默认 Risky（需审批）。
        if self.read_only {
            RiskLevel::Safe
        } else {
            RiskLevel::Risky
        }
    }

    async fn execute(&self, arguments: &str, _ctx: &ToolContext<'_>) -> ToolResult {
        let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
        match self.client.call_tool(&self.remote_name, args).await {
            Ok(text) => ToolResult::ok(&self.qualified, text),
            Err(message) => ToolResult::error(&self.qualified, message),
        }
    }
}

/// 报告 server 提供但当前不可用的工具的占位（server 断连后调用直接失败）。
pub struct UnavailableMcpTool {
    qualified: String,
    reason: String,
}

impl UnavailableMcpTool {
    pub fn new(server: &str, tool: &str, reason: impl Into<String>) -> Self {
        Self {
            qualified: format!("mcp__{server}__{tool}"),
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl Tool for UnavailableMcpTool {
    fn name(&self) -> &str {
        &self.qualified
    }

    fn description(&self) -> &str {
        "Unavailable MCP tool (server disconnected)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(&self, _arguments: &str, _ctx: &ToolContext<'_>) -> ToolResult {
        ToolResult::error(&self.qualified, &self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::client::McpTransport;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct ScriptedTransport {
        scripted: VecDeque<String>,
    }

    #[async_trait]
    impl McpTransport for ScriptedTransport {
        async fn send_line(&mut self, _line: &str) -> Result<(), String> {
            Ok(())
        }
        async fn recv_line(&mut self) -> Result<Option<String>, String> {
            Ok(self.scripted.pop_front())
        }
    }

    fn client(responses: Vec<&str>) -> McpClient {
        McpClient::new(
            "fs",
            Box::new(ScriptedTransport {
                scripted: responses.into_iter().map(String::from).collect(),
            }),
        )
    }

    fn def(name: &str, read_only: bool) -> McpToolDef {
        McpToolDef {
            name: name.to_string(),
            description: "d".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            read_only,
        }
    }

    async fn requester() -> mrorcode_kernel::Requester {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        mrorcode_kernel::request::RequestBroker::new(tx).handle()
    }

    #[tokio::test]
    async fn qualified_name_and_readonly_risk() {
        let c = client(vec![]);
        let ro = McpTool::new(c.clone(), def("read", true));
        assert_eq!(ro.name(), "mcp__fs__read");
        assert_eq!(
            ro.risk(&Value::Null),
            mrorcode_kernel::tool::RiskLevel::Safe
        );

        let rw = McpTool::new(c, def("write", false));
        assert_eq!(
            rw.risk(&Value::Null),
            mrorcode_kernel::tool::RiskLevel::Risky
        );
    }

    #[tokio::test]
    async fn execute_forwards_and_returns_text() {
        let c = client(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"ok!"}]}}"#,
        ]);
        let tool = McpTool::new(c, def("read", true));
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = requester().await;
        let dir = std::env::temp_dir();
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
            tool_id: "",
            event_sink: None,
        };
        let r = tool.execute(r#"{"path":"x"}"#, &ctx).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content, "ok!");
        let _ = Arc::new(Mutex::new(()));
    }

    #[tokio::test]
    async fn unavailable_tool_is_error() {
        let tool = UnavailableMcpTool::new("fs", "read", "server disconnected");
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = requester().await;
        let dir = std::env::temp_dir();
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
            tool_id: "",
            event_sink: None,
        };
        let r = tool.execute("{}", &ctx).await;
        assert!(r.is_error);
        assert!(r.content.contains("disconnected"));
    }
}
