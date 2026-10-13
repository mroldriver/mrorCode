//! 底部输入区的元信息行与样式装配（TextArea 本体在 store 中）。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::store::{AppState, Phase};
use crate::theme::Theme;

/// 输入框内容最大高度（行）。
pub fn max_height(term_height: u16) -> u16 {
    (term_height / 3).clamp(1, 6)
}

/// 输入框实际占用高度：单行起，随内容增长到 [`max_height`] 封顶。
pub fn content_height(state: &AppState, term_height: u16) -> u16 {
    (state.prompt.lines().len() as u16).clamp(1, max_height(term_height))
}

/// 应用统一样式到 TextArea。
///
/// 光标的可见性由应用层节拍翻转（终端 SGR blink 支持参差），
/// 这里只按当前帧的闪烁态给样式：亮帧反相方块，灭帧用文本默认色（即不可见）。
pub fn style_textarea(state: &mut AppState, theme: &Theme) {
    state.prompt.set_style(Style::default().fg(theme.text));
    sync_cursor_style(state, theme);
    state.prompt.set_cursor_line_style(Style::default());
    state
        .prompt
        .set_placeholder_style(Style::default().fg(theme.text_muted));
}

/// 按当前闪烁态同步光标样式（每帧/每次翻转调用）。
///
/// 亮帧：白色实心方块（fg 用面板深色保证方块内字符反相可读）；灭帧用文本默认色（不可见）。
pub fn sync_cursor_style(state: &mut AppState, theme: &Theme) {
    let cursor_style = if state.cursor_blink_on {
        Style::default().fg(theme.background_panel).bg(Color::White)
    } else {
        Style::default()
    };
    state.prompt.set_cursor_style(cursor_style);
}

/// 在给定区域内渲染 textarea（左竖线语义）。
///
/// 对齐 opencode：左竖线用 secondary（默认 agent 色，蓝），内边距左 2 / 右 2 / 上 1。
pub fn render_prompt(
    state: &AppState,
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    theme: &Theme,
) {
    // 对齐 opencode：蓝线(col0) + 灰面板(col1+)，紧贴无空列
    // 外层 box 只有左蓝线；内层灰面板从 col1 开始，paddingLeft=2
    let panel_x = area.x + 1;
    let panel_width = area.width.saturating_sub(1);
    // 灰面板背景：col1 到右边缘
    for y in area.y..area.y + area.height {
        for x in panel_x..panel_x + panel_width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(theme.background_panel));
            }
        }
        // 蓝竖线 col0
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol("\u{2503}"); // ┃
            cell.set_style(Style::default().fg(theme.secondary));
        }
    }
    // 内边距：面板内左 2 / 右 2 / 上 1
    let pad_left: u16 = 2;
    let pad_right: u16 = 2;
    let pad_y: u16 = 1;
    let inset = ratatui::layout::Rect {
        x: panel_x.saturating_add(pad_left),
        y: area.y.saturating_add(pad_y),
        width: panel_width.saturating_sub(pad_left + pad_right),
        height: area.height.saturating_sub(pad_y),
    };
    (&state.prompt).render(inset, buf);
}

/// 底栏元信息。
pub fn meta_line(state: &AppState, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::from(" ")];

    // 左侧状态前缀：OC | model | dir
    spans.push(Span::styled(
        "OC",
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(" | ", Style::default().fg(theme.text_muted)));

    match state.phase {
        Phase::Working | Phase::WaitingApproval => {
            spans.push(Span::styled(
                format!("{} ", state.spinner_glyph()),
                Style::default().fg(theme.primary),
            ));
            spans.push(Span::styled(
                match state.phase {
                    Phase::WaitingApproval => "waiting approval",
                    _ => "working…",
                },
                Style::default().fg(theme.warning),
            ));
        }
        Phase::Idle => {
            if state.plan_mode {
                spans.push(Span::styled("plan ", Style::default().fg(theme.warning)));
            }
            spans.push(Span::styled(
                state.model.clone(),
                Style::default().fg(theme.primary),
            ));
        }
    }

    spans.push(Span::styled(" | ", Style::default().fg(theme.text_muted)));
    let cwd = state
        .working_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| state.working_dir.display().to_string());
    spans.push(Span::styled(cwd, Style::default().fg(theme.text_muted)));

    // 右侧提示
    spans.push(Span::styled(
        "   enter 发送 · ctrl+j 换行 · leader b 侧栏",
        Style::default().fg(theme.text_muted),
    ));
    Line::from(spans)
}
