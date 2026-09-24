//! MCP JSON-RPC 2.0 消息模型与行编解码（stdio transport，换行分隔）。
//!
//! 与 SSE 解码器同理：纯函数 `decode_line`，用录制字节做单测，不依赖网络。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// MCP 协议版本（初始化握手时发送）。
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// 出站请求。
#[derive(Clone, Debug, Serialize)]
pub struct Request {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.into(),
            params,
        }
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("request serialization cannot fail")
    }
}

/// 入站消息（响应或通知）。
#[derive(Clone, Debug, Deserialize)]
pub struct Incoming {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
    #[serde(default)]
    pub method: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// 解码一行；空行/非对象返回 None。
pub fn decode_line(line: &str) -> Option<Incoming> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// 构造 initialize 请求参数。
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "mrorcode", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// 构造 tools/call 请求参数。
pub fn tools_call_params(name: &str, arguments: Value) -> Value {
    json!({ "name": name, "arguments": arguments })
}

/// 从 tools/call 的 result 中提取可读文本（拼接 text content 块）。
pub fn extract_text(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        parts.push(t.to_string());
                    }
                }
                Some(other) => parts.push(format!("[{other} content]")),
                None => {}
            }
        }
    }
    if parts.is_empty() {
        // 无 content：回退到整个 result 的 JSON
        result.to_string()
    } else {
        parts.join("\n")
    }
}

/// 单个 MCP 工具描述。
#[derive(Clone, Debug)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

/// 解析 tools/list 的 result。
pub fn parse_tool_list(result: &Value) -> Vec<McpToolDef> {
    result
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let name = t.get("name")?.as_str()?.to_string();
                    let description = t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input_schema = t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"}));
                    let read_only = t
                        .get("annotations")
                        .and_then(|a| a.get("readOnlyHint"))
                        .and_then(|r| r.as_bool())
                        .unwrap_or(false);
                    Some(McpToolDef {
                        name,
                        description,
                        input_schema,
                        read_only,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_response_line() {
        let line = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let inc = decode_line(line).unwrap();
        assert_eq!(inc.id, Some(1));
        assert!(inc.result.is_some());
        assert!(inc.error.is_none());
    }

    #[test]
    fn decode_error_line() {
        let line = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"not found"}}"#;
        let inc = decode_line(line).unwrap();
        assert_eq!(inc.error.unwrap().code, -32601);
    }

    #[test]
    fn decode_ignores_blank_and_garbage() {
        assert!(decode_line("").is_none());
        assert!(decode_line("  ").is_none());
        assert!(decode_line("not json").is_none());
    }

    #[test]
    fn request_roundtrip_line() {
        let req = Request::new(7, "tools/call", Some(tools_call_params("x", json!({}))));
        let line = req.to_line();
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["id"], 7);
        assert_eq!(parsed["method"], "tools/call");
        assert_eq!(parsed["params"]["name"], "x");
    }

    #[test]
    fn parse_tools_with_readonly_hint() {
        let result = json!({
            "tools": [
                { "name": "read", "description": "r", "inputSchema": {"type":"object"},
                  "annotations": {"readOnlyHint": true} },
                { "name": "write", "description": "w" }
            ]
        });
        let tools = parse_tool_list(&result);
        assert_eq!(tools.len(), 2);
        assert!(tools[0].read_only);
        assert!(!tools[1].read_only);
        assert_eq!(tools[1].input_schema["type"], "object");
    }

    #[test]
    fn extract_text_joins_content_blocks() {
        let result = json!({
            "content": [
                { "type": "text", "text": "line1" },
                { "type": "text", "text": "line2" }
            ]
        });
        assert_eq!(extract_text(&result), "line1\nline2");
    }
}
