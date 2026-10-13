//! Session 主区：消息窗口化渲染（只画可视行 ± 余量，万行历史不重排不可见内容）。

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::store::{AppState, MessageView, Role};
use crate::theme::Theme;
use crate::widgets::{reasoning, tool_card};

fn message_height(
    m: &mut MessageView,
    width: u16,
    gen: u64,
    theme: &Theme,
    expanded: &HashSet<String>,
    expanded_reasoning: &HashSet<u64>,
) -> u16 {
    // 文档同步发生在 store 的事件投影中；这里只读缓存，不 clone。
    let body = m.doc.line_count(width, gen, theme).max(1) as u16;
    let reason = reasoning::height(m, width, expanded_reasoning.contains(&m.uid), gen, theme);
    let tools: u16 = m
        .tools
        .iter()
        .map(|t| 1 + tool_card::extra_height(t, expanded.contains(t.id.as_str())))
        .sum();
    let bubble_pad = if matches!(m.role, Role::User) { 2 } else { 0 };
    reason + body + tools + 1 + bubble_pad
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
    let gen = state.theme_gen;
    state.viewport_lines = area.height;

    // 第一遍：用缓存的行高表算总高（不产生样式分配）
    let mut heights: Vec<u16> = Vec::with_capacity(state.messages.len());
    let mut total: u16 = 0;
    for m in &mut state.messages {
        let h = message_height(
            m,
            width.saturating_sub(2),
            gen,
            theme,
            &state.expanded_tools,
            &state.expanded_reasoning,
        );
        heights.push(h);
        total = total.saturating_add(h);
    }
    state.last_total_lines = total;
    let top = total
        .saturating_sub(area.height)
        .saturating_sub(state.backscroll);

    // 第二遍：仅布局与绘制与可视区相交的消息
    let spinner = state.spinner_glyph().to_string();
    let focused = state.focused_tool.clone();
    let focused_reasoning = state.focused_reasoning;
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
        render_message(
            m,
            y0,
            area,
            buf,
            width,
            gen,
            theme,
            &spinner,
            &state.expanded_tools,
            &state.expanded_reasoning,
            focused.as_deref(),
            focused_reasoning,
        );
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

#[allow(clippy::too_many_arguments)]
fn render_message(
    m: &mut MessageView,
    y0: u16,
    area: Rect,
    buf: &mut Buffer,
    width: u16,
    gen: u64,
    theme: &Theme,
    spinner: &str,
    expanded: &HashSet<String>,
    expanded_reasoning: &HashSet<u64>,
    focused: Option<&str>,
    focused_reasoning: Option<u64>,
) {
    let inner_width = width.saturating_sub(2);
    let mut row = y0;

    // 思考过程块（先于正文）
    if reasoning::is_present(m) {
        let is_exp = expanded_reasoning.contains(&m.uid);
        let is_focus = focused_reasoning == Some(m.uid);
        if is_exp {
            let lines = reasoning::expanded_lines(m, inner_width, gen, theme, is_focus);
            for line in lines {
                if row >= area.y + area.height {
                    break;
                }
                buf.set_line(area.x + 1, row, &line, width - 1);
                row += 1;
            }
        } else {
            if row < area.y + area.height {
                let line = reasoning::collapsed_line(m, theme, is_focus);
                buf.set_line(area.x + 1, row, &line, width - 1);
            }
            row += 1;
        }
    }

    let (accent, body_bg) = match m.role {
        // 用户气泡左竖线与输入框统一用 secondary（蓝）
        Role::User => (theme.secondary, theme.background_panel),
        Role::Assistant => (theme.border_subtle, theme.background),
    };

    let lines: Vec<Line<'static>> = {
        let mut lines = m.doc.lines(inner_width, gen, theme);
        if m.is_error {
            for line in &mut lines {
                for span in &mut line.spans {
                    span.style = span.style.fg(theme.error);
                }
            }
        }
        // 用户消息气泡：Span 必须带 bg，否则 set_line 会把整行背景重置掉
        if matches!(m.role, Role::User) {
            for line in &mut lines {
                for span in &mut line.spans {
                    span.style = span.style.bg(body_bg);
                }
            }
        }
        lines
    };

    let is_user = matches!(m.role, Role::User);
    let pad_top = if is_user { 1 } else { 0 };

    // 渲染一行气泡（背景 + 左竖线），text 为 None 时只画背景
    // 对齐 opencode：竖线是最左边缘，右边全是灰面板，文字 paddingLeft=2
    let mut render_bubble_row = |y: u16, text: Option<&Line<'static>>| {
        if y >= area.y + area.height {
            return;
        }
        for x in area.x..area.x + width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(body_bg));
            }
        }
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol("\u{2503}"); // ┃
            cell.set_style(Style::default().fg(accent).bg(body_bg));
        }
        if let Some(line) = text {
            buf.set_line(area.x + 2, y, line, inner_width);
        }
    };

    // 顶部留白
    if is_user {
        render_bubble_row(row, None);
    }

    for (i, line) in lines.iter().enumerate() {
        render_bubble_row(row + pad_top + i as u16, Some(line));
    }

    // 底部留白
    if is_user {
        render_bubble_row(row + pad_top + lines.len() as u16, None);
    }

    let mut row = row + pad_top + lines.len() as u16 + if is_user { 1 } else { 0 };
    for tool in &m.tools {
        let is_focused = focused.is_some_and(|f| f == tool.id);
        let is_expanded = expanded.contains(tool.id.as_str());
        if row < area.y + area.height {
            let l = tool_card::line(tool, spinner, theme, is_focused);
            buf.set_line(area.x + 1, row, &l, width - 1);
        }
        row += 1;
        let extras = tool_card::extra_lines(tool, is_expanded, is_focused, width - 3, theme);
        for l in extras {
            if row < area.y + area.height {
                buf.set_line(area.x + 1, row, &l, width - 1);
            }
            row += 1;
        }
    }
}
