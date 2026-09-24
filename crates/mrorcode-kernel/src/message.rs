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
                || m.tool_call_id
                    .as_deref()
                    .is_some_and(|id| known_call_ids.contains(id))
        });
    }

    /// 粗略 token 估算（约 4 字符 / token，含 tool 调用参数），不引入 tokenizer。
    ///
    /// 只用于压缩触发判断，误差由阈值留白吸收。
    pub fn estimate_tokens(&self) -> usize {
        let chars: usize = self
            .messages
            .iter()
            .map(|m| {
                m.text.chars().count()
                    + m.reasoning
                        .as_deref()
                        .map(|r| r.chars().count())
                        .unwrap_or(0)
                    + m.tool_calls
                        .iter()
                        .map(|c| c.name.chars().count() + c.arguments.chars().count())
                        .sum::<usize>()
            })
            .sum();
        chars / 4 + self.messages.len() * 4
    }

    /// 压缩：保留首条 system 与最近 `keep_turns` 个 user turn，中间区间折叠为一条注记。
    ///
    /// 「一个 turn」= 一条 user 消息及其后到下一个 user 之前的所有消息。
    /// 折叠后的注记是 system 消息，保证不产生孤儿 tool 结果（内部会再 `repair_pairing`）。
    /// 返回被折叠的消息条数；无需折叠时返回 0。
    pub fn compact(&mut self, keep_turns: usize) -> usize {
        if keep_turns == 0 {
            return 0;
        }
        // 定位全部 user 消息下标。
        let user_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == Role::User)
            .map(|(i, _)| i)
            .collect();
        if user_indices.len() <= keep_turns {
            return 0;
        }
        // 被保留的最早 user 下标；其之前（除 system）的区间全部折叠。
        let keep_from = user_indices[user_indices.len() - keep_turns];
        let system_prefix: Vec<Message> = self
            .messages
            .iter()
            .take_while(|m| m.role == Role::System)
            .cloned()
            .collect();
        let fold_start = system_prefix.len();
        if fold_start >= keep_from {
            return 0;
        }
        let folded = &self.messages[fold_start..keep_from];
        let tool_calls = folded.iter().map(|m| m.tool_calls.len()).sum::<usize>();
        let first_user = folded
            .iter()
            .find(|m| m.role == Role::User)
            .map(|m| truncate_chars(&m.text, 200))
            .unwrap_or_else(|| "(no user text)".to_string());

        let note = Message::system(format!(
            "[compacted {} earlier message(s) incl. {} tool call(s); first request: {}]",
            folded.len(),
            tool_calls,
            first_user
        ));

        let mut rebuilt: Vec<Message> = Vec::with_capacity(self.messages.len());
        rebuilt.extend(system_prefix);
        rebuilt.push(note);
        rebuilt.extend(self.messages[keep_from..].iter().cloned());
        let removed = self.messages.len() - rebuilt.len();
        self.messages = rebuilt;
        self.repair_pairing();
        removed
    }
}

/// 截断到最多 `max` 个字符（按 UTF-8 字符边界），超出加省略号。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
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
            "c1", "bash",
        )]));
        convo.push(Message::tool_result(&ToolResult {
            call_id: "c1".into(),
            content: "ok".into(),
            is_error: false,
        }));
        convo.repair_pairing();
        assert_eq!(convo.messages.len(), 2);
    }

    fn turn_with_tool(convo: &mut Conversation, user: &str, call: &str) {
        convo.push(Message::user(user));
        convo.push(Message::assistant_tool_calls(vec![ToolCall::new(
            call, "bash",
        )]));
        convo.push(Message::tool_result(&ToolResult {
            call_id: call.into(),
            content: format!("output for {user}"),
            is_error: false,
        }));
        convo.push(Message::assistant_text(format!("answer for {user}")));
    }

    #[test]
    fn compact_keeps_system_and_recent_turns() {
        let mut convo = Conversation::new();
        convo.push(Message::system("SYS"));
        for i in 0..5 {
            turn_with_tool(&mut convo, &format!("u{i}"), &format!("c{i}"));
        }
        let removed = convo.compact(2);
        assert!(removed > 0);
        // system 保留
        assert_eq!(convo.messages[0].role, Role::System);
        assert_eq!(convo.messages[0].text, "SYS");
        // 折叠注记
        assert!(convo.messages[1].text.contains("compacted"));
        // 最近两轮逐字保留
        let texts: Vec<&str> = convo.messages.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"answer for u3"));
        assert!(texts.contains(&"answer for u4"));
        assert!(!texts.contains(&"answer for u0"));
        // 无孤儿 tool
        convo.repair_pairing();
        let before = convo.messages.len();
        convo.repair_pairing();
        assert_eq!(before, convo.messages.len());
    }

    #[test]
    fn compact_is_noop_when_few_turns() {
        let mut convo = Conversation::new();
        convo.push(Message::system("SYS"));
        turn_with_tool(&mut convo, "u0", "c0");
        assert_eq!(convo.compact(4), 0);
    }

    #[test]
    fn estimate_tokens_grows_with_content() {
        let mut small = Conversation::new();
        small.push(Message::user("hi"));
        let mut big = Conversation::new();
        big.push(Message::user("x".repeat(4000)));
        assert!(big.estimate_tokens() > small.estimate_tokens());
    }
}
