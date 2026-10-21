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

    /// 修复悬空配对：移除孤儿 tool 消息，并为「有 call 无结果」补合成结果。
    ///
    /// 正常循环中调用与结果总是成对落库；该函数是防御性修复（取消/resume/异常路径）。
    /// OpenAI/Anthropic 都要求每个 tool_call 紧跟着对应结果，否则下一次请求会 400。
    pub fn repair_pairing(&mut self) {
        // 1) 移除找不到对应 assistant tool_call 的孤儿 tool 消息。
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

        // 2) 为未执行的 call 补一条合成结果（取消/异常退出时可能出现）。
        //    结果插在该 assistant 消息之后连续 tool 结果块的末尾，保持协议顺序。
        let mut i = 0;
        while i < self.messages.len() {
            if self.messages[i].role != Role::Assistant || self.messages[i].tool_calls.is_empty() {
                i += 1;
                continue;
            }
            let call_ids: Vec<String> = self.messages[i]
                .tool_calls
                .iter()
                .map(|c| c.id.clone())
                .collect();
            let mut end = i + 1;
            let mut answered = std::collections::HashSet::new();
            while end < self.messages.len() && self.messages[end].role == Role::Tool {
                if let Some(id) = &self.messages[end].tool_call_id {
                    answered.insert(id.clone());
                }
                end += 1;
            }
            let missing: Vec<Message> = call_ids
                .iter()
                .filter(|id| !answered.contains(*id))
                .map(|id| {
                    Message::tool_result(&ToolResult::error(
                        id.clone(),
                        serde_json::json!({"status": "cancelled"}).to_string(),
                    ))
                })
                .collect();
            let inserted = missing.len();
            if inserted > 0 {
                self.messages.splice(end..end, missing);
            }
            i = end + inserted;
        }
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

    /// 回退（rewind）：只保留首条 system 与最早 `keep_turns` 个 user turn，其后全部丢弃并修复配对。
    ///
    /// 用于 M4-9：rewind 到历史某轮之前继续对话。snapshot 会被覆盖，transcript 保留。
    /// 「一个 turn」与 [`Self::compact`] 同义：一条 user 消息及其后到下一个 user 之前的所有消息。
    /// 返回被删除的条数；`keep_turns` 不小于现有 turn 数时视为 no-op 返回 0。
    pub fn rewind(&mut self, keep_turns: usize) -> usize {
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
        // 截断点 = 第 keep_turns+1 条 user 消息处；其前的消息整体保留（含本轮收尾的 assistant 消息）。
        let cut = user_indices[keep_turns];
        let removed = self.messages.len() - cut;
        self.messages.truncate(cut);
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

    #[test]
    fn repair_synthesizes_missing_tool_results() {
        let mut convo = Conversation::new();
        convo.push(Message::user("hi"));
        convo.push(Message::assistant_tool_calls(vec![
            ToolCall::new("c1", "bash"),
            ToolCall::new("c2", "bash"),
        ]));
        // 只有 c1 执行完成（取消发生在执行 c2 之前）
        convo.push(Message::tool_result(&ToolResult {
            call_id: "c1".into(),
            content: "ok".into(),
            is_error: false,
        }));
        convo.repair_pairing();

        assert_eq!(convo.messages.len(), 4);
        assert_eq!(convo.messages[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(convo.messages[3].role, Role::Tool);
        assert_eq!(convo.messages[3].tool_call_id.as_deref(), Some("c2"));
        assert!(convo.messages[3].is_error);
        assert!(convo.messages[3].text.contains("cancelled"));

        // 幂等：再次修复不改变消息数
        let len = convo.messages.len();
        convo.repair_pairing();
        assert_eq!(convo.messages.len(), len);
    }

    #[test]
    fn repair_synthesizes_all_results_when_tool_block_missing() {
        let mut convo = Conversation::new();
        convo.push(Message::user("hi"));
        convo.push(Message::assistant_tool_calls(vec![ToolCall::new(
            "c1", "bash",
        )]));
        // 取消发生在工具执行前：assistant 之后直接是下一条 user
        convo.push(Message::user("next"));
        convo.repair_pairing();

        assert_eq!(convo.messages.len(), 4);
        assert_eq!(convo.messages[2].role, Role::Tool);
        assert_eq!(convo.messages[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(convo.messages[3].role, Role::User);
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
    fn rewind_truncates_and_repairs() {
        let mut convo = Conversation::new();
        convo.push(Message::system("SYS"));
        turn_with_tool(&mut convo, "u0", "c0");
        turn_with_tool(&mut convo, "u1", "c1");
        turn_with_tool(&mut convo, "u2", "c2");
        let total = convo.messages.len();
        // 保留前 1 个 turn（u0 含其 tool 结果与 answer）；配对修复
        let removed = convo.rewind(1);
        assert!(removed > 0);
        assert_eq!(convo.messages.len(), total - removed);
        assert_eq!(convo.messages[0].role, Role::System);
        assert_eq!(convo.messages[0].text, "SYS");
        // 保留的 turn 末尾是 u0 的 answer
        assert_eq!(convo.messages.last().unwrap().text, "answer for u0");
        // 无孤儿 tool 结果
        convo.repair_pairing();
        let before = convo.messages.len();
        convo.repair_pairing();
        assert_eq!(before, convo.messages.len());
    }

    #[test]
    fn rewind_noop_when_keep_beyond_turns() {
        let mut convo = Conversation::new();
        convo.push(Message::user("u0"));
        assert_eq!(convo.rewind(100), 0);
        assert_eq!(convo.rewind(1), 0);
        assert_eq!(convo.rewind(0), 0);
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
