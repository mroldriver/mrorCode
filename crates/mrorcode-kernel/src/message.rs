//! 会话消息模型。
//!
//! 所有类型均 serde 可序列化，未来可无损落盘为 snapshot。

use serde::{Deserialize, Serialize};

use crate::tool::ToolResult;

/// 消息角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 模型发起的工具调用。`arguments` 为原始 JSON 字符串，流式期间逐片段累积。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments: String::new(),
        }
    }
}

/// 消息附带的边车统计（token 用量等）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageMeta {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// 一条会话消息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub meta: MessageMeta,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self::plain(Role::System, text)
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::plain(Role::User, text)
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self::plain(Role::Assistant, text)
    }

    pub fn assistant_tool_calls(calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            text: String::new(),
            tool_calls: calls,
            tool_call_id: None,
            is_error: false,
            reasoning: None,
            meta: MessageMeta::default(),
        }
    }

    pub fn tool_result(result: &ToolResult) -> Self {
        Self {
            role: Role::Tool,
            text: result.content.clone(),
            tool_calls: Vec::new(),
            tool_call_id: Some(result.call_id.clone()),
            is_error: result.is_error,
            reasoning: None,
            meta: MessageMeta::default(),
        }
    }

    fn plain(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            is_error: false,
            reasoning: None,
            meta: MessageMeta::default(),
        }
    }
}

/// 完整对话（按时间顺序的消息序列）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn extend(&mut self, messages: impl IntoIterator<Item = Message>) {
        self.messages.extend(messages);
    }

    /// 取 system prompt（首条 system 消息的文本）。
    pub fn system_prompt(&self) -> Option<&str> {
        self.messages
            .iter()
            .find(|m| m.role == Role::System)
            .map(|m| m.text.as_str())
    }

    /// 修复悬空配对：移除找不到对应 assistant tool_call 的孤儿 tool 消息。
    ///
    /// 正常循环中调用与结果总是成对落库；该函数是防御性修复（resume/异常路径）。
    pub fn repair_pairing(&mut self) {
        let known_call_ids: std::collections::HashSet<String> = self
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter().map(|c| c.id.clone()))
            .collect();
        self.messages.retain(|m| {
            m.role != Role::Tool
                || m
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| known_call_ids.contains(id))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_drops_orphan_tool_messages() {
        let mut convo = Conversation::new();
        convo.push(Message::user("hi"));
        convo.push(Message::tool_result(&ToolResult {
            call_id: "orphan".into(),
            content: "x".into(),
            is_error: false,
        }));
        convo.repair_pairing();
        assert_eq!(convo.messages.len(), 1);
    }

    #[test]
    fn repair_keeps_paired_messages() {
        let mut convo = Conversation::new();
        convo.push(Message::assistant_tool_calls(vec![ToolCall::new(
            "c1",
            "bash",
        )]));
        convo.push(Message::tool_result(&ToolResult {
            call_id: "c1".into(),
            content: "ok".into(),
            is_error: false,
        }));
        convo.repair_pairing();
        assert_eq!(convo.messages.len(), 2);
    }
}
