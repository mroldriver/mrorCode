//! 底部输入区的元信息行与样式装配（TextArea 本体在 store 中）。

use ratatui::style::Style;
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
pub fn sync_cursor_style(state: &mut AppState, theme: &Theme) {
    let cursor_style = if state.cursor_blink_on {
        Style::default()
            .fg(theme.background_panel)
            .bg(theme.primary)
    } else {
        Style::default()
    };
    state.prompt.set_cursor_style(cursor_style);
}

/// 在给定区域内渲染 textarea（左竖线语义）。
pub fn render_prompt(
    state: &AppState,
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    theme: &Theme,
) {
    // 背景
    ratatui::widgets::Block::default()
        .style(Style::default().bg(theme.background_panel))
        .render(area, buf);
    // 左侧竖线
    let border = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::LEFT)
        .border_style(Style::default().fg(theme.primary))
        .border_set(ratatui::symbols::border::Set {
            vertical_left: "┃",
            ..ratatui::symbols::border::PLAIN
        });
    let inner = border.inner(area);
    border.render(area, buf);
    let inset = ratatui::layout::Rect {
        x: inner.x.saturating_add(0),
        y: inner.y,
        width: inner.width,
        height: inner.height,
    };
    (&state.prompt).render(inset, buf);
}

/// 底栏元信息。
pub fn meta_line(state: &AppState, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::from(" ")];
    match state.phase {
        Phase::Working | Phase::WaitingApproval => {
            spans.push(Span::styled(
                format!("{} ", state.spinner_glyph()),
                Style::default().fg(theme.primary),
            ));
            spans.push(Span::styled(
                match state.phase {
                    Phase::WaitingApproval => "waiting approval · esc interrupt",
                    _ => "working…  esc interrupt",
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

    let cwd = state
        .working_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| state.working_dir.display().to_string());
    spans.push(Span::styled(
        format!("   {cwd}   enter 发送 · ctrl+j 换行 · F2 侧栏"),
        Style::default().fg(theme.text_muted),
    ));
    Line::from(spans)
}
