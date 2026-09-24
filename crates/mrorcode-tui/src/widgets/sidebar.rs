//! 右侧 42 列侧栏：会话列表（active 高亮/busy spinner）+ 模型/目录徽标。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
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

    let title = Line::from(Span::styled(
        "mrorcode",
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::BOLD),
    ));
    buf.set_line(inner.x, inner.y, &title, inner.width);

    // 底部徽标占 5 行（空行/model/dir/空/提示）
    let footer_h = 5u16;
    let list_top = inner.y + 2;
    let list_bottom = inner
        .y
        .saturating_add(inner.height)
        .saturating_sub(footer_h);

    for (i, session) in state.sessions.iter().enumerate() {
        let y = list_top + i as u16;
        if y >= list_bottom {
            break;
        }
        let active = session.id == state.active_session;
        let marker = if active {
            "\u{25CF} " // ●
        } else {
            "  "
        };
        let glyph = if session.phase.is_busy() {
            format!("{} ", state.spinner_glyph())
        } else {
            String::new()
        };
        let label =
            crate::render::truncate_end(&session.title, (inner.width as usize).saturating_sub(6));
        let color = if active {
            theme.primary
        } else {
            theme.text_muted
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.primary)),
            Span::styled(glyph, Style::default().fg(color)),
            Span::styled(label, Style::default().fg(color)),
        ]);
        buf.set_line(inner.x, y, &line, inner.width);
    }

    // 底部徽标
    let fy = inner.y + inner.height.saturating_sub(footer_h);
    let cwd = crate::render::truncate_end(
        &state.working_dir.display().to_string(),
        inner.width as usize - 8,
    );
    let footer = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("model  ", Style::default().fg(theme.text_muted)),
            Span::styled(state.model.clone(), Style::default().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::styled("dir    ", Style::default().fg(theme.text_muted)),
            Span::styled(cwd, Style::default().fg(theme.text)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "ctrl+p 命令 \u{00B7} ctrl+n 新会话",
            Style::default().fg(theme.text_muted),
        )),
    ];
    for (i, line) in footer.into_iter().enumerate() {
        let y = fy + i as u16;
        if y < inner.y + inner.height {
            buf.set_line(inner.x, y, &line, inner.width);
        }
    }
}
