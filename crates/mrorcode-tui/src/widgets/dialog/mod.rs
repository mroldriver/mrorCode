//! 通用对话框栈：任意时刻只渲染栈顶，按键先交栈顶 modal 层。
//!
//! M2-6 fuzzy Select；M2-10 Confirm；M2-11 Alert/Help。

pub mod forms;
pub mod select;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::theme::Theme;

pub use forms::{AlertState, ConfirmAction, ConfirmState, HelpRow, HelpState};
pub use select::{Dialog, DialogOutcome, SelectItem, SelectPurpose, SelectState};

/// 渲染栈顶对话框（含遮罩与面板）。
pub fn render_top(dialog: &Dialog, area: Rect, buf: &mut Buffer, theme: &Theme) {
    match dialog {
        Dialog::Select(sel) => select::render(sel, area, buf, theme),
        Dialog::Confirm(c) => forms::render(c, area, buf, theme),
        Dialog::Alert(a) => forms::render_alert(a, area, buf, theme),
        Dialog::Help(h) => forms::render_help(h, area, buf, theme),
    }
}

/// 把按键交给栈顶对话框。
pub fn key_top(dialog: &mut Dialog, key: &KeyEvent) -> DialogOutcome {
    match dialog {
        Dialog::Select(sel) => sel.on_key(key),
        Dialog::Confirm(c) => c.on_key(key),
        Dialog::Alert(a) => a.on_key(key),
        Dialog::Help(h) => h.on_key(key),
    }
}
