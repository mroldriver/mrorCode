//! 审批居中卡片（M1 简化版，M2 升级为通用 Dialog 栈）。

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::store::AppState;
use crate::theme::Theme;

pub fn render(state: &AppState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    let Some(req) = &state.permission else {
        return;
    };
    Clear.render(area, buf);

    let width = 60u16.min(area.width.saturating_sub(4)).max(20);
    let height = 9u16.min(area.height.saturating_sub(2)).max(6);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 3;
    let card = Rect {
        x,
        y,
        width,
        height,
    };

    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning))
        .style(Style::default().bg(theme.background_menu))
        .title_alignment(Alignment::Left)
        .render(card, buf);

    let inner = Rect {
        x: card.x + 1,
        y: card.y + 1,
        width: card.width - 2,
        height: card.height - 2,
    };

    let args: serde_json::Value = serde_json::from_str(&req.arguments).unwrap_or_default();
    let detail = args
        .get("command")
        .and_then(|v| v.as_str())
        .or_else(|| args.get("path").and_then(|v| v.as_str()))
        .unwrap_or(&req.arguments)
        .to_string();
    let detail = crate::render::truncate_end(&detail, (width - 4) as usize);

    let lines = vec![
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "Permission required",
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )),
        ratatui::text::Line::raw(""),
        ratatui::text::Line::from(vec![
            ratatui::text::Span::styled("tool: ", Style::default().fg(theme.text_muted)),
            ratatui::text::Span::styled(req.tool.clone(), Style::default().fg(theme.primary)),
        ]),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            detail,
            Style::default().fg(theme.text),
        )),
        ratatui::text::Line::raw(""),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "y 本次允许 · a 始终允许 · n 拒绝 (esc)",
            Style::default().fg(theme.text_muted),
        )),
    ];
    Paragraph::new(lines).render(inner, buf);
}
