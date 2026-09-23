//! 基准测试支持（criterion 外部 bench target 只能访问公开 API）。
//!
//! 生产代码不调用本模块的任何内容。

use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::store::{AppState, MessageView, Role};
use crate::theme::Theme;
use crate::widgets::markdown::MarkdownView;
use crate::widgets::session;

/// 构造含 N 条 assistant 消息的状态（每条 10 行普通文本）。
pub fn long_history_state(n: usize) -> AppState {
    let mut state = AppState::new("bench-model".into(), PathBuf::from("."));
    for i in 0..n {
        let text = (0..10)
            .map(|j| format!("message {i} line {j} with some markdown *emphasis* text"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut msg = MessageView::assistant();
        msg.role = Role::Assistant;
        msg.doc.update(&text);
        msg.text = text;
        state.messages.push(msg);
    }
    state
}

/// 在 120x40 的 TestBackend 上渲染一帧（窗口化路径）。
pub fn render_long_history_frame(state: &mut AppState) {
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| {
        let area = f.area();
        let theme = Theme::tokyonight();
        session::render(state, area, f.buffer_mut(), &theme);
    })
    .unwrap();
}

/// 对照基准：对整段文本从零解析+折行（模拟 opencode 式每帧全量重排）。
pub fn full_reparse(text: &str, width: u16) -> usize {
    let mut doc = MarkdownView::new();
    doc.update(text);
    doc.line_count(width, &Theme::tokyonight())
}

/// 增量路径：只 push 尾字符再更新（block 前缀缓存命中）。
pub fn incremental_update(doc: &mut MarkdownView, text: &str, width: u16) -> usize {
    doc.update(text);
    doc.line_count(width, &Theme::tokyonight())
}

pub fn new_doc() -> MarkdownView {
    MarkdownView::new()
}
