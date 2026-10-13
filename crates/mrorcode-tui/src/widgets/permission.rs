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
    let height = 11u16.min(area.height.saturating_sub(2)).max(8);
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

    let options = ["本次允许", "始终允许", "拒绝"];
    let cursor = state.permission_cursor.min(options.len() - 1);

    let mut lines: Vec<ratatui::text::Line> = vec![
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
    ];
    for (i, opt) in options.iter().enumerate() {
        let selected = i == cursor;
        let marker = if selected { "\u{25B6} " } else { "  " }; // ▶
        let style = if selected {
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_muted)
        };
        lines.push(ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(marker, style),
            ratatui::text::Span::styled(*opt, style),
        ]));
    }
    lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
        "\u{2191}\u{2193} 选择 · Enter 确认 · Esc 拒绝",
        Style::default().fg(theme.text_muted),
    )));
    Paragraph::new(lines).render(inner, buf);
}
