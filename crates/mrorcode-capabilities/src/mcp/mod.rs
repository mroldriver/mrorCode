//! MCP stdio 支持：JSON-RPC 协议、客户端与工具桥接。

pub mod client;
pub mod protocol;
pub mod tool;

use std::sync::Arc;

use mrorcode_kernel::tool::{Tool, ToolRegistry};

pub use client::{McpClient, McpTransport, StdioTransport};
pub use protocol::{McpToolDef, PROTOCOL_VERSION};
pub use tool::{McpTool, UnavailableMcpTool};

/// 一个已连接的 server 及其工具。
pub struct McpServerHandle {
    pub name: String,
    pub client: McpClient,
    pub tools: Vec<McpToolDef>,
}

/// 连接结果：成功接入的 server + 失败诊断。
pub struct McpBootstrap {
    pub servers: Vec<McpServerHandle>,
    pub warnings: Vec<String>,
}

/// 按配置拉起全部 server、初始化并列出工具。
///
/// 单个 server 失败只产生 warning，不阻塞启动（其余工具照常可用）。
pub async fn bootstrap(servers: &[mrorcode_config::McpServer]) -> McpBootstrap {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for cfg in servers {
        if cfg.name.is_empty() || cfg.command.is_empty() {
            warnings.push("MCP server entry missing name or command; skipped".to_string());
            continue;
        }
        match McpClient::spawn_stdio(cfg.name.clone(), &cfg.command, &cfg.args, &cfg.env) {
            Ok(client) => match client.list_tools().await {
                Ok(tools) => {
                    tracing::info!(server = %cfg.name, count = tools.len(), "MCP server connected");
                    out.push(McpServerHandle {
                        name: cfg.name.clone(),
                        client,
                        tools,
                    });
                }
                Err(e) => warnings.push(format!("MCP server {}: {e}", cfg.name)),
            },
            Err(e) => warnings.push(e),
        }
    }
    McpBootstrap {
        servers: out,
        warnings,
    }
}

impl McpBootstrap {
    /// 把全部已连接 server 的工具注册进注册表。
    pub fn register_into(&self, registry: &mut ToolRegistry) {
        for server in &self.servers {
            for def in &server.tools {
                let tool: Arc<dyn Tool> =
                    Arc::new(McpTool::new(server.client.clone(), def.clone()));
                registry.insert(tool);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}
