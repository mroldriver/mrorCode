//! 底部输入区的元信息行与样式装配（TextArea 本体在 store 中）。

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::store::{AppState, Phase};
use crate::theme::Theme;

/// 输入框最大高度（行）。
pub fn max_height(term_height: u16) -> u16 {
    (term_height / 3).clamp(1, 6)
}

/// 应用统一样式到 TextArea。
pub fn style_textarea(state: &mut AppState, theme: &Theme) {
    state.prompt.set_style(Style::default().fg(theme.text));
    state.prompt.set_cursor_style(
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::SLOW_BLINK),
    );
    state.prompt.set_cursor_line_style(Style::default());
    state
        .prompt
        .set_placeholder_style(Style::default().fg(theme.text_muted));
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
