//! 右侧 42 列侧栏（M1：模型/目录/会话信息；M2 加 context/todo/files 分区）。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use crate::store::AppState;
use crate::theme::Theme;

pub const WIDTH: u16 = 42;

pub fn render(state: &AppState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.background_panel))
        .render(area, buf);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let cwd = state.working_dir.display().to_string();
    let cwd = crate::render::truncate_end(&cwd, inner.width as usize);
    let messages = state.messages.len().to_string();

    let rows = vec![
        Line::from(Span::styled(
            "mrorcode",
            Style::default()
                .fg(theme.primary)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled("model  ", Style::default().fg(theme.text_muted)),
            Span::styled(state.model.clone(), Style::default().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::styled("dir    ", Style::default().fg(theme.text_muted)),
            Span::styled(cwd, Style::default().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::styled("msgs   ", Style::default().fg(theme.text_muted)),
            Span::styled(messages, Style::default().fg(theme.text)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "F2 toggle sidebar",
            Style::default().fg(theme.text_muted),
        )),
    ];

    for (i, line) in rows.into_iter().enumerate() {
        let y = inner.y + i as u16;
        if y < inner.y + inner.height {
            buf.set_line(inner.x, y, &line, inner.width);
        }
    }
}
