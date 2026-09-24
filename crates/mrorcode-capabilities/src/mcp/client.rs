//! MCP stdio 客户端：JSON-RPC over 子进程 stdin/stdout（换行分隔）。
//!
//! 抽象出 `McpTransport` 便于用脚本化传输做单测（与 SSE 录制字节同思路），
//! 生产实现 `StdioTransport` 管理子进程生命周期（`kill_on_drop`）。

use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::protocol::{
    decode_line, initialize_params, parse_tool_list, tools_call_params, McpToolDef, Request,
};

/// 单条 JSON-RPC 行传输。
#[async_trait]
pub trait McpTransport: Send {
    async fn send_line(&mut self, line: &str) -> Result<(), String>;
    /// 读取下一条非空行；EOF 返回 None。
    async fn recv_line(&mut self) -> Result<Option<String>, String>;
}

/// 生产传输：子进程 stdin/stdout。
pub struct StdioTransport {
    /// 持有子进程句柄；`kill_on_drop` 保证 Drop 即 teardown。
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl StdioTransport {
    /// 拉起子进程并建立传输。
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn MCP server {command}: {e}"))?;
        let stdin = child.stdin.take().ok_or("MCP server stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("MCP server stdout unavailable")?;
        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_line(&mut self, line: &str) -> Result<(), String> {
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("MCP write failed: {e}"))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("MCP write failed: {e}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("MCP flush failed: {e}"))
    }

    async fn recv_line(&mut self) -> Result<Option<String>, String> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self
                .stdout
                .read_line(&mut line)
                .await
                .map_err(|e| format!("MCP read failed: {e}"))?;
            if n == 0 {
                return Ok(None);
            }
            if !line.trim().is_empty() {
                return Ok(Some(line));
            }
        }
    }
}

/// 连接一个 MCP server 的客户端（廉价 Clone，共享传输）。
#[derive(Clone)]
pub struct McpClient {
    server: String,
    transport: Arc<Mutex<Box<dyn McpTransport>>>,
    next_id: Arc<AtomicU64>,
    initialized: Arc<std::sync::atomic::AtomicBool>,
}

impl McpClient {
    pub fn new(server: impl Into<String>, transport: Box<dyn McpTransport>) -> Self {
        Self {
            server: server.into(),
            transport: Arc::new(Mutex::new(transport)),
            next_id: Arc::new(AtomicU64::new(1)),
            initialized: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 拉起一个 stdio server。
    pub fn spawn_stdio(
        server: impl Into<String>,
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
    ) -> Result<Self, String> {
        let transport = StdioTransport::spawn(command, args, env)?;
        Ok(Self::new(server, Box::new(transport)))
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// 发送请求并等待匹配 id 的响应（跳过通知/其他消息）。
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = Request::new(id, method, params);
        {
            let mut t = self.transport.lock().await;
            t.send_line(&req.to_line()).await?;
        }
        self.await_response(id).await
    }

    /// 发送通知（无 id，不等待响应）。
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), String> {
        let line = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(Value::Null),
        })
        .to_string();
        let mut t = self.transport.lock().await;
        t.send_line(&line).await
    }

    async fn await_response(&self, id: u64) -> Result<Value, String> {
        let mut t = self.transport.lock().await;
        loop {
            match t.recv_line().await? {
                None => return Err(format!("MCP server {} closed the connection", self.server)),
                Some(line) => {
                    if let Some(msg) = decode_line(&line) {
                        if msg.id != Some(id) {
                            continue; // 通知或过期响应
                        }
                        if let Some(err) = msg.error {
                            return Err(format!("MCP error {}: {}", err.code, err.message));
                        }
                        return Ok(msg.result.unwrap_or(Value::Null));
                    }
                }
            }
        }
    }

    /// 初始化握手（幂等）。
    pub async fn initialize(&self) -> Result<(), String> {
        if self.initialized.load(Ordering::Relaxed) {
            return Ok(());
        }
        self.request("initialize", Some(initialize_params()))
            .await?;
        self.notify("notifications/initialized", None).await?;
        self.initialized.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// 列出工具。
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, String> {
        self.initialize().await?;
        let result = self.request("tools/list", Some(json!({}))).await?;
        Ok(parse_tool_list(&result))
    }

    /// 调用工具，返回可读文本。
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, String> {
        self.initialize().await?;
        let result = self
            .request("tools/call", Some(tools_call_params(name, arguments)))
            .await?;
        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let text = super::protocol::extract_text(&result);
        if is_error {
            Err(text)
        } else {
            Ok(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// 脚本化传输：每个 send 弹出下一个预设响应行。
    struct ScriptedTransport {
        outgoing: Arc<Mutex<Vec<String>>>,
        scripted: VecDeque<String>,
    }

    #[async_trait]
    impl McpTransport for ScriptedTransport {
        async fn send_line(&mut self, line: &str) -> Result<(), String> {
            self.outgoing.lock().await.push(line.to_string());
            Ok(())
        }
        async fn recv_line(&mut self) -> Result<Option<String>, String> {
            Ok(self.scripted.pop_front())
        }
    }

    fn scripted(responses: Vec<&str>) -> (McpClient, Arc<Mutex<Vec<String>>>) {
        let outgoing = Arc::new(Mutex::new(Vec::new()));
        let transport = ScriptedTransport {
            outgoing: outgoing.clone(),
            scripted: responses.into_iter().map(String::from).collect(),
        };
        (McpClient::new("test", Box::new(transport)), outgoing)
    }

    #[tokio::test]
    async fn initialize_and_list_tools() {
        let (client, outgoing) = scripted(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"read","description":"r","inputSchema":{"type":"object"}}]}}"#,
        ]);
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read");
        let sent = outgoing.lock().await;
        assert!(sent[0].contains("initialize"));
        assert!(sent[1].contains("notifications/initialized"));
    }

    #[tokio::test]
    async fn call_tool_extracts_text() {
        let (client, _out) = scripted(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"hello"}]}}"#,
        ]);
        let text = client.call_tool("echo", json!({"x":1})).await.unwrap();
        assert_eq!(text, "hello");
    }

    #[tokio::test]
    async fn error_response_is_propagated() {
        let (client, _out) = scripted(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"no such tool"}}"#,
        ]);
        let err = client.call_tool("missing", json!({})).await.unwrap_err();
        assert!(err.contains("no such tool"), "{err}");
    }

    #[tokio::test]
    async fn eof_is_error() {
        let (client, _out) = scripted(vec![]);
        let err = client.initialize().await.unwrap_err();
        assert!(err.contains("closed the connection"), "{err}");
    }
}
