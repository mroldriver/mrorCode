//! 单行文本输入对话框（用于会话重命名等场景）。
//!
//! 居中面板 + 遮罩，Enter 提交、Esc 取消，支持基本光标移动与退格。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::render::truncate_end;
use crate::theme::Theme;
use crate::widgets::dialog::select::{draw_border_static, fill, OVERLAY};

/// 输入对话框的用途（提交后由 app 分派）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPurpose {
    /// 重命名当前会话。
    RenameSession,
}

#[derive(Debug)]
pub struct InputState {
    pub title: String,
    pub purpose: InputPurpose,
    text: String,
    cursor: usize, // 字符索引（不是字节）
}

impl InputState {
    pub fn new(
        title: impl Into<String>,
        purpose: InputPurpose,
        initial: impl Into<String>,
    ) -> Self {
        let initial = initial.into();
        let cursor = initial.chars().count();
        Self {
            title: title.into(),
            purpose,
            text: initial,
            cursor,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn on_key(&mut self, key: &KeyEvent) -> crate::widgets::dialog::DialogOutcome {
        use crate::widgets::dialog::DialogOutcome;
        match key.code {
            KeyCode::Enter => DialogOutcome::InputSubmitted(self.purpose, self.text.clone()),
            KeyCode::Esc => DialogOutcome::Closed,
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_char(c);
                DialogOutcome::None
            }
            KeyCode::Backspace => {
                self.delete_before();
                DialogOutcome::None
            }
            KeyCode::Delete => {
                self.delete_after();
                DialogOutcome::None
            }
            KeyCode::Left => {
                self.move_cursor(-1);
                DialogOutcome::None
            }
            KeyCode::Right => {
                self.move_cursor(1);
                DialogOutcome::None
            }
            KeyCode::Home => {
                self.cursor = 0;
                DialogOutcome::None
            }
            KeyCode::End => {
                self.cursor = self.text.chars().count();
                DialogOutcome::None
            }
            // Ctrl+A / Ctrl+E 对齐 Emacs 行内键位
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
                DialogOutcome::None
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.text.chars().count();
                DialogOutcome::None
            }
            // Ctrl+U 清空到行首
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let chars: Vec<char> = self.text.chars().collect();
                let keep: String = chars[self.cursor..].iter().collect();
                self.text = keep;
                self.cursor = 0;
                DialogOutcome::None
            }
            _ => DialogOutcome::None,
        }
    }

    fn insert_char(&mut self, c: char) {
        let byte_idx = self.char_to_byte(self.cursor);
        self.text.insert(byte_idx, c);
        self.cursor += 1;
    }

    fn delete_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let byte_idx = self.char_to_byte(self.cursor - 1);
        self.text.remove(byte_idx);
        self.cursor -= 1;
    }

    fn delete_after(&mut self) {
        if self.cursor >= self.text.chars().count() {
            return;
        }
        let byte_idx = self.char_to_byte(self.cursor);
        self.text.remove(byte_idx);
    }

    fn move_cursor(&mut self, delta: i32) {
        let len = self.text.chars().count() as i32;
        self.cursor = (self.cursor as i32 + delta).clamp(0, len) as usize;
    }

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }
}

pub fn render(input: &InputState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    // 遮罩
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(OVERLAY));
            }
        }
    }

    // 紧凑居中框：宽 60%（最小 40，最大 80），高固定 6 行
    let w = (area.width * 3 / 5).clamp(40, 80.min(area.width.saturating_sub(2)));
    let h = 6u16.min(area.height.saturating_sub(2));
    let px = area.x + (area.width - w) / 2;
    let py = area.y + (area.height - h) / 2;
    let panel = Rect {
        x: px,
        y: py,
        width: w,
        height: h,
    };
    fill(panel, buf, theme.background_menu);
    draw_border_static(panel, buf, theme);

    let inner_w = w as usize - 4;

    // 标题
    buf.set_line(
        px + 2,
        py + 1,
        &Line::from(Span::styled(
            truncate_end(&input.title, inner_w),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        w - 4,
    );

    // 输入框（第三行）：显示文本 + 光标
    let input_y = py + 3;
    let display = truncate_end(&input.text, inner_w.saturating_sub(2));
    let cursor_char_idx = input.cursor.min(display.chars().count());
    let before: String = display.chars().take(cursor_char_idx).collect();
    let at: String = display.chars().skip(cursor_char_idx).take(1).collect();
    let after: String = display.chars().skip(cursor_char_idx + 1).collect();

    let mut spans = vec![
        Span::styled("> ", Style::default().fg(theme.text_muted)),
        Span::styled(before, Style::default().fg(theme.text)),
    ];
    if at.is_empty() {
        spans.push(Span::styled(
            " ",
            Style::default()
                .fg(theme.background_menu)
                .bg(theme.text)
                .add_modifier(Modifier::REVERSED),
        ));
    } else {
        spans.push(Span::styled(
            at,
            Style::default()
                .fg(theme.background_menu)
                .bg(theme.text)
                .add_modifier(Modifier::REVERSED),
        ));
    }
    spans.push(Span::styled(after, Style::default().fg(theme.text)));
    buf.set_line(px + 2, input_y, &Line::from(spans), w - 4);

    // 底部提示
    buf.set_line(
        px + 2,
        py + h - 2,
        &Line::from(Span::styled(
            "enter 确认 · esc 取消",
            Style::default().fg(theme.text_muted),
        )),
        w - 4,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::dialog::DialogOutcome;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn typing_inserts_at_cursor() {
        let mut input = InputState::new("test", InputPurpose::RenameSession, "");
        input.on_key(&key(KeyCode::Char('a')));
        input.on_key(&key(KeyCode::Char('b')));
        assert_eq!(input.text(), "ab");
    }

    #[test]
    fn backspace_deletes_before_cursor() {
        let mut input = InputState::new("test", InputPurpose::RenameSession, "abc");
        input.on_key(&key(KeyCode::Backspace));
        assert_eq!(input.text(), "ab");
    }

    #[test]
    fn left_right_moves_cursor() {
        let mut input = InputState::new("test", InputPurpose::RenameSession, "abc");
        input.on_key(&key(KeyCode::Left));
        input.on_key(&key(KeyCode::Char('x')));
        assert_eq!(input.text(), "abxc");
    }

    #[test]
    fn ctrl_u_clears_to_start() {
        let mut input = InputState::new("test", InputPurpose::RenameSession, "hello");
        input.on_key(&key(KeyCode::Left));
        input.on_key(&ctrl_key(KeyCode::Char('u')));
        assert_eq!(input.text(), "o");
    }

    #[test]
    fn enter_submits_and_esc_closes() {
        let mut input = InputState::new("test", InputPurpose::RenameSession, "name");
        match input.on_key(&key(KeyCode::Enter)) {
            DialogOutcome::InputSubmitted(purpose, text) => {
                assert_eq!(purpose, InputPurpose::RenameSession);
                assert_eq!(text, "name");
            }
            _ => panic!("expected InputSubmitted"),
        }
        let mut input2 = InputState::new("test", InputPurpose::RenameSession, "");
        match input2.on_key(&key(KeyCode::Esc)) {
            DialogOutcome::Closed => {}
            _ => panic!("expected Closed"),
        }
    }

    #[test]
    fn unicode_handling() {
        let mut input = InputState::new("test", InputPurpose::RenameSession, "你好");
        input.on_key(&key(KeyCode::Backspace));
        assert_eq!(input.text(), "你");
        input.on_key(&key(KeyCode::Char('世')));
        assert_eq!(input.text(), "你世");
    }
}
