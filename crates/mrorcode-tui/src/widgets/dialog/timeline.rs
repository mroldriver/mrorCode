//! 消息时间线对话框（P2-9）。
//!
//! 列出当前会话的所有消息（user + assistant），显示角色图标与摘要；
//! Enter 回退到选中消息之后（与 rewind 语义一致：保留选中消息及之前的内容）。

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::store::{MessageView, Role};
use crate::theme::Theme;

/// 时间线条目：消息索引 + 显示文本。
#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub msg_index: usize,
    pub role: Role,
    pub summary: String,
    pub tool_count: usize,
}

/// timeline 对话框状态。
#[derive(Debug)]
pub struct TimelineState {
    title: String,
    entries: Vec<TimelineEntry>,
    state: ListState,
}

impl TimelineState {
    /// 从消息视图构建时间线。
    pub fn from_messages(title: impl Into<String>, messages: &[MessageView]) -> Self {
        let entries: Vec<TimelineEntry> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| TimelineEntry {
                msg_index: i,
                role: m.role,
                summary: crate::render::truncate_end(
                    m.text.lines().next().unwrap_or("").trim(),
                    60,
                ),
                tool_count: m.tools.len(),
            })
            .collect();
        let mut state = ListState::default();
        if !entries.is_empty() {
            state.select(Some(entries.len() - 1));
        }
        Self {
            title: title.into(),
            entries,
            state,
        }
    }

    /// 当前选中的消息索引（None 表示无选中）。
    pub fn selected(&self) -> Option<usize> {
        self.state.selected().map(|i| self.entries[i].msg_index)
    }

    /// 键盘处理。返回 Some(usize) 表示用户确认回退到该消息索引之后。
    pub fn on_key(&mut self, key: &crate::keymap::KeyPress) -> Option<usize> {
        use crate::keymap::PressCode;
        match key.code {
            PressCode::Up | PressCode::Char('k') => {
                if let Some(i) = self.state.selected() {
                    if i > 0 {
                        self.state.select(Some(i - 1));
                    }
                }
                None
            }
            PressCode::Down | PressCode::Char('j') => {
                if let Some(i) = self.state.selected() {
                    if i + 1 < self.entries.len() {
                        self.state.select(Some(i + 1));
                    }
                }
                None
            }
            PressCode::Enter => self.selected(),
            _ => None,
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(theme.primary),
            ))
            .title_alignment(Alignment::Left);

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|e| {
                let (icon, color) = match e.role {
                    Role::User => ("❯", theme.secondary),
                    Role::Assistant => ("◇", theme.primary),
                };
                let tools = if e.tool_count > 0 {
                    format!(" [{} tools]", e.tool_count)
                } else {
                    String::new()
                };
                let text = format!("{icon} #{:02} {}{}", e.msg_index, e.summary, tools);
                ListItem::new(Line::from(Span::styled(text, Style::default().fg(color))))
            })
            .collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().bg(theme.primary).fg(theme.background))
            .highlight_symbol("▶ ");

        let mut state = self.state;
        ratatui::widgets::StatefulWidget::render(list, area, buf, &mut state);

        // 底部提示
        let hint = Paragraph::new(Line::from(Span::styled(
            " ↑↓ 选择  Enter 回退到此消息之后  Esc 关闭 ",
            Style::default().fg(theme.text_muted),
        )))
        .alignment(Alignment::Center);
        let hint_area = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(1),
            width: area.width,
            height: 1,
        };
        use ratatui::widgets::Widget;
        hint.render(hint_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MessageView;

    fn msg(role: Role, text: &str) -> MessageView {
        let mut m = match role {
            Role::User => MessageView::user(1, text.into()),
            Role::Assistant => MessageView::assistant(1),
        };
        m.text = text.into();
        m
    }

    #[test]
    fn from_messages_builds_entries() {
        let messages = vec![
            msg(Role::User, "hello"),
            msg(Role::Assistant, "hi there"),
            msg(Role::User, "how are you"),
        ];
        let tl = TimelineState::from_messages("timeline", &messages);
        assert_eq!(tl.entries.len(), 3);
        assert_eq!(tl.entries[0].msg_index, 0);
        assert!(matches!(tl.entries[0].role, Role::User));
        assert_eq!(tl.entries[1].summary, "hi there");
        // 默认选中最后一条
        assert_eq!(tl.selected(), Some(2));
    }

    #[test]
    fn on_key_navigates_and_returns_selection() {
        let messages = vec![msg(Role::User, "a"), msg(Role::Assistant, "b")];
        let mut tl = TimelineState::from_messages("t", &messages);
        assert_eq!(tl.selected(), Some(1));

        // 上移
        let up = crate::keymap::KeyPress {
            code: crate::keymap::PressCode::Up,
            ctrl: false,
            alt: false,
            shift: false,
        };
        assert!(tl.on_key(&up).is_none());
        assert_eq!(tl.selected(), Some(0));

        // 确认
        let enter = crate::keymap::KeyPress {
            code: crate::keymap::PressCode::Enter,
            ctrl: false,
            alt: false,
            shift: false,
        };
        assert_eq!(tl.on_key(&enter), Some(0));
    }
}
