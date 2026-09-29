//! Home 页：居中 logo + 居中输入框（宽 max(75, 70%屏宽)）。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::store::{AppState, Route};
use crate::theme::Theme;
use crate::widgets::prompt;

pub fn render(state: &mut AppState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    let body = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3), // logo
        Constraint::Length(1),
        Constraint::Max(8), // prompt（随内容 1→封顶 行）
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .flex(Flex::Center)
    .split(area);

    let logo = vec![
        Line::from(Span::styled(
            "mrorcode",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "a fast terminal coding agent",
            Style::default().fg(theme.text_muted),
        )),
    ];
    Paragraph::new(logo).centered().render(body[1], buf);

    let pw = 75u16.min(((area.width as f32 * 0.7) as u16).max(40));
    let ph = prompt::content_height(state, area.height) + 2;
    let [slot] = Layout::horizontal([Constraint::Length(pw)])
        .flex(Flex::Center)
        .areas(body[3]);
    // 输入区只有几行时在预留槽内垂直居中，避免整块悬在槽顶部
    let y_off = slot.height.saturating_sub(ph) / 2;
    let prompt_area = Rect {
        y: slot.y + y_off,
        height: ph,
        ..slot
    };

    prompt::render_prompt(state, prompt_area, buf, theme);

    // 底部键位提示
    let [footer] = Layout::horizontal([Constraint::Length(pw)])
        .flex(Flex::Center)
        .areas(body[4]);
    let tips = vec![
        Line::from(Span::styled(
            "enter 发送 · ctrl+j 换行 · esc 退出",
            Style::default().fg(theme.text_muted),
        )),
        Line::from(Span::styled(
            "ctrl+p 命令面板 · ctrl+n 新会话 · leader e 编辑器",
            Style::default().fg(theme.text_muted),
        )),
    ];
    Paragraph::new(tips).centered().render(footer, buf);

    let _ = Route::Home;
}
