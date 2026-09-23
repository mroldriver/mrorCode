//! Session 主区：消息窗口化渲染（只画可视行 ± 余量，万行历史不重排不可见内容）。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::store::{AppState, MessageView, Role};
use crate::theme::Theme;
use crate::widgets::tool_card;

fn output_height(tool: &crate::store::ToolView) -> u16 {
    let n = tool.output.trim().lines().count() as u16;
    if n == 0 {
        0
    } else {
        n.min(10) + if n > 10 { 1 } else { 0 }
    }
}

fn message_height(m: &mut MessageView, width: u16, theme: &Theme) -> u16 {
    // 文档同步发生在 store 的事件投影中；这里只读缓存，不 clone。
    let body = m.doc.line_count(width, theme).max(1) as u16;
    let tools: u16 = m.tools.iter().map(|t| 1 + output_height(t)).sum();
    body + tools + 1
}

/// 渲染主消息区。
pub fn render(state: &mut AppState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    // 背景
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(theme.background));
            }
        }
    }

    let width = area.width;
    state.viewport_lines = area.height;

    // 第一遍：用缓存的行高表算总高（不产生样式分配）
    let mut heights: Vec<u16> = Vec::with_capacity(state.messages.len());
    let mut total: u16 = 0;
    for m in &mut state.messages {
        let h = message_height(m, width.saturating_sub(2), theme);
        heights.push(h);
        total = total.saturating_add(h);
    }
    state.last_total_lines = total;
    let top = total
        .saturating_sub(area.height)
        .saturating_sub(state.backscroll);

    // 第二遍：仅布局与绘制与可视区相交的消息
    let spinner = state.spinner_glyph().to_string();
    let mut cursor: u16 = 0;
    for (i, m) in state.messages.iter_mut().enumerate() {
        let h = heights[i];
        let start = cursor;
        let end = cursor + h;
        cursor = end;
        if end <= top || start >= top + area.height {
            continue;
        }
        let y0 = area.y + start.saturating_sub(top);
        render_message(m, y0, area, buf, width, theme, &spinner);
    }

    // 空态提示
    if state.messages.is_empty() {
        let hint = Line::from(Span::styled(
            "输入消息开始对话（enter 发送）",
            Style::default().fg(theme.text_muted),
        ));
        buf.set_line(area.x + 1, area.y + 1, &hint, area.width - 2);
    }
}

fn render_message(
    m: &mut MessageView,
    y0: u16,
    area: Rect,
    buf: &mut Buffer,
    width: u16,
    theme: &Theme,
    spinner: &str,
) {
    let inner_width = width.saturating_sub(2);
    let lines: Vec<Line<'static>> = {
        let mut lines = m.doc.lines(inner_width, theme);
        if m.is_error {
            for line in &mut lines {
                for span in &mut line.spans {
                    span.style = span.style.fg(theme.error);
                }
            }
        }
        lines
    };

    let (accent, body_bg) = match m.role {
        Role::User => (theme.primary, theme.background_panel),
        Role::Assistant => (theme.border_subtle, theme.background),
    };

    for (i, line) in lines.iter().enumerate() {
        let y = y0 + i as u16;
        if y >= area.y + area.height {
            break;
        }
        // 卡片背景 + 左竖线
        for x in area.x..area.x + width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(body_bg));
            }
        }
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol("┃");
            cell.set_style(Style::default().fg(accent).bg(body_bg));
        }
        buf.set_line(area.x + 2, y, line, inner_width);
    }

    let mut row = y0 + lines.len() as u16;
    for tool in &m.tools {
        if row < area.y + area.height {
            let l = tool_card::line(tool, spinner, theme);
            let mut spans = vec![Span::from(" ")];
            spans.extend(l.spans.iter().cloned());
            buf.set_line(area.x + 1, row, &Line::from(spans), width - 1);
        }
        row += 1;
        let outs = tool_card::output_lines(tool, width - 3, theme);
        for l in outs {
            if row < area.y + area.height {
                buf.set_line(area.x + 1, row, &l, width - 1);
            }
            row += 1;
        }
    }
}
