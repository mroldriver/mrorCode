//! 会话持久化数据模型（纯数据，无文件 IO）。
//!
//! - `TranscriptEntry`：jsonl 每行一条，忠实记录（不压缩，用于 recall）。
//! - `SessionRecord`：可 resume 的会话快照（完整 `Conversation`）。
//!
//! 文件读写落在 capabilities（kernel 保持零 fs 依赖）。

use serde::{Deserialize, Serialize};

use crate::message::Conversation;
use crate::tool::ToolResult;

/// transcript 行。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptEntry {
    User {
        text: String,
    },
    Assistant {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
    },
}

impl TranscriptEntry {
    /// 从一条消息派生出 0..n 个 transcript 条目。
    pub fn from_message(message: &crate::message::Message) -> Vec<TranscriptEntry> {
        use crate::message::Role;
        match message.role {
            Role::System => Vec::new(),
            Role::User => vec![TranscriptEntry::User {
                text: message.text.clone(),
            }],
            Role::Assistant => {
                let mut out = Vec::new();
                if !message.text.is_empty() || message.reasoning.is_some() {
                    out.push(TranscriptEntry::Assistant {
                        text: message.text.clone(),
                        reasoning: message.reasoning.clone(),
                    });
                }
                for call in &message.tool_calls {
                    out.push(TranscriptEntry::ToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    });
                }
                out
            }
            Role::Tool => vec![TranscriptEntry::ToolResult {
                call_id: message.tool_call_id.clone().unwrap_or_default(),
                content: message.text.clone(),
                is_error: message.is_error,
            }],
        }
    }

    /// 重建一条消息（用于从 transcript 恢复）。ToolCall 续写由调用方处理。
    pub fn to_message(&self) -> Option<crate::message::Message> {
        use crate::message::{Message, ToolCall};
        match self {
            TranscriptEntry::User { text } => Some(Message::user(text.clone())),
            TranscriptEntry::Assistant { text, reasoning } => {
                let mut m = Message::assistant_text(text.clone());
                m.reasoning = reasoning.clone();
                Some(m)
            }
            TranscriptEntry::ToolCall {
                id,
                name,
                arguments,
            } => Some(Message::assistant_tool_calls(vec![ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            }])),
            TranscriptEntry::ToolResult {
                call_id,
                content,
                is_error,
            } => Some(Message::tool_result(&ToolResult {
                call_id: call_id.clone(),
                content: content.clone(),
                is_error: *is_error,
            })),
        }
    }
}

/// 可 resume 的会话快照。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: u64,
    pub title: String,
    /// Unix 秒（无 chrono 依赖）。
    pub updated_at: u64,
    pub conversation: Conversation,
}

/// 侧栏列表项（不需加载完整会话）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: u64,
    pub title: String,
    /// 消息条数（近似）。
    pub message_count: usize,
    pub updated_at: u64,
}

impl SessionSummary {
    pub fn from_record(record: &SessionRecord) -> Self {
        Self {
            id: record.id,
            title: record.title.clone(),
            message_count: record.conversation.messages.len(),
            updated_at: record.updated_at,
        }
    }
}

/// 由首条 user 消息生成标题（截断 40 字符）。
pub fn derive_title(conversation: &Conversation) -> String {
    use crate::message::Role;
    let text = conversation
        .messages
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| m.text.trim())
        .unwrap_or("");
    if text.is_empty() {
        return "untitled".to_string();
    }
    let mut chars: String = text.chars().take(40).collect();
    if text.chars().count() > 40 {
        chars.push('…');
    }
    chars.replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, ToolCall};

    #[test]
    fn transcript_roundtrip_user_and_assistant() {
        let entry = TranscriptEntry::from_message(&Message::user("hi"));
        assert_eq!(entry.len(), 1);
        let msg = entry[0].to_message().unwrap();
        assert_eq!(msg.text, "hi");

        let a = Message::assistant_text("hello");
        let e = TranscriptEntry::from_message(&a);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].to_message().unwrap().text, "hello");
    }

    #[test]
    fn tool_call_transcript_roundtrip() {
        let msg = Message::assistant_tool_calls(vec![ToolCall {
            id: "c1".into(),
            name: "bash".into(),
            arguments: "{\"command\":\"ls\"}".into(),
        }]);
        let entries = TranscriptEntry::from_message(&msg);
        assert_eq!(entries.len(), 1);
        let back = entries[0].to_message().unwrap();
        assert_eq!(back.tool_calls.len(), 1);
        assert_eq!(back.tool_calls[0].name, "bash");
    }

    #[test]
    fn derive_title_truncates_and_flattens() {
        let mut c = Conversation::new();
        c.push(Message::user("line1\nline2"));
        assert_eq!(derive_title(&c), "line1 line2");

        let long = "x".repeat(100);
        let mut c2 = Conversation::new();
        c2.push(Message::user(long));
        assert_eq!(derive_title(&c2).chars().count(), 41);
    }
}
