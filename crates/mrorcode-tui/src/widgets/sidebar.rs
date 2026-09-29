//! 右侧 42 列侧栏：可折叠三区分栏（M4-8）。
//!
//! - Sessions：会话列表（active 高亮/busy spinner）
//! - Context：累计 token + 是否触发过压缩
//! - Files：读写/编辑过的文件（最近优先，上限 8）
//!
//! 分区标题可点击折叠；窄屏（宽 < 34）仅渲染 Sessions。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use crate::store::AppState;
use crate::theme::Theme;

const SESSION_TITLE: &str = "Sessions";
const CONTEXT_TITLE: &str = "Context";
const FILES_TITLE: &str = "Files";

pub fn render(
    state: &AppState,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
) -> Vec<crate::hit::HitRegion> {
    let mut hits = Vec::new();
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

    let mut y = inner.y + 2;
    let bottom_limit = inner.y + inner.height.saturating_sub(5);

    // M4-8 窄屏降级：仅 Sessions
    let only_sessions = inner.width < 34;

    // ---- Sessions ----
    let (sec_y, more_hits) =
        section_header(state, SESSION_TITLE, y, inner, buf, theme, bottom_limit);
    hits.extend(more_hits);
    y = sec_y + 1;
    // 固定会话排最前（保持组内原顺序）
    let mut ordered: Vec<&crate::store::SessionMeta> = state.sessions.iter().collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.pinned));
    for session in ordered {
        if y >= bottom_limit {
            break;
        }
        let active = session.id == state.active_session;
        let marker = if active {
            "\u{25CF} " // ●
        } else {
            "  "
        };
        let pin = if session.pinned { "\u{1F4CC} " } else { "  " };
        let glyph = if session.phase.is_busy() {
            format!("{} ", state.spinner_glyph())
        } else {
            String::new()
        };
        let label =
            crate::render::truncate_end(&session.title, (inner.width as usize).saturating_sub(8));
        let color = if active {
            theme.primary
        } else {
            theme.text_muted
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.primary)),
            Span::styled(pin, Style::default().fg(theme.warning)),
            Span::styled(glyph, Style::default().fg(color)),
            Span::styled(label, Style::default().fg(color)),
        ]);
        buf.set_line(inner.x, y, &line, inner.width);
        hits.push(crate::hit::HitRegion {
            rect: Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
            target: crate::hit::HitTarget::Session(session.id),
        });
        y += 1;
    }

    // ---- Context / Files ----
    if !only_sessions {
        let c_vis = !state.is_section_collapsed(CONTEXT_TITLE);
        let (sec_y, more_hits) =
            section_header(state, CONTEXT_TITLE, y + 1, inner, buf, theme, bottom_limit);
        hits.extend(more_hits);
        if c_vis && sec_y + 1 < bottom_limit {
            y = sec_y + 1;
            let mut lines: Vec<Line> = Vec::new();
            let tokens = state
                .usage_input_tokens
                .saturating_add(state.usage_output_tokens);
            lines.push(Line::from(vec![
                Span::styled("tokens   ", Style::default().fg(theme.text_muted)),
                Span::styled(format!("{tokens}"), Style::default().fg(theme.text)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    "compacted",
                    Style::default().fg(if state.compacted {
                        theme.warning
                    } else {
                        theme.text_muted
                    }),
                ),
                Span::styled(
                    format!("  {}", if state.compacted { "yes" } else { "no" }),
                    Style::default().fg(if state.compacted {
                        theme.warning
                    } else {
                        theme.text_muted
                    }),
                ),
            ]));
            for line in lines {
                if y >= bottom_limit {
                    break;
                }
                buf.set_line(inner.x, y, &line, inner.width);
                y += 1;
            }
        }

        let f_vis = !state.is_section_collapsed(FILES_TITLE);
        let (sec_y, more_hits) =
            section_header(state, FILES_TITLE, y + 1, inner, buf, theme, bottom_limit);
        hits.extend(more_hits);
        if f_vis {
            y = sec_y + 1;
            for path in state.touched_files.iter() {
                if y >= bottom_limit {
                    break;
                }
                let file =
                    crate::render::truncate_end(path, (inner.width as usize).saturating_sub(2));
                let line = Line::from(vec![
                    Span::styled("\u{221F} ", Style::default().fg(theme.text_muted)), // ∟
                    Span::styled(file, Style::default().fg(theme.text)),
                ]);
                buf.set_line(inner.x, y, &line, inner.width);
                y += 1;
            }
        }
    }

    // 底部徽标
    let fy = y.max(inner.y + 1);
    if fy + 4 < inner.y + inner.height {
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
            Line::from(Span::styled(
                "ctrl+p 命令 \u{00B7} ctrl+n 新会话",
                Style::default().fg(theme.text_muted),
            )),
        ];
        for (i, line) in footer.into_iter().enumerate() {
            let yy = fy + i as u16;
            if yy < inner.y + inner.height {
                buf.set_line(inner.x, yy, &line, inner.width);
            }
        }
    }
    hits
}

/// 渲染一个分区标题行（点击可折叠），返回该区可用首行 y 与命中区。
fn section_header(
    state: &AppState,
    title: &str,
    y: u16,
    inner: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    bottom_limit: u16,
) -> (u16, Vec<crate::hit::HitRegion>) {
    if y >= bottom_limit {
        return (y, Vec::new());
    }
    let collapsed = state.is_section_collapsed(title);
    let arrow = if collapsed { "\u{25B8} " } else { "\u{25BE} " }; // ▸ / ▾
    let line = Line::from(vec![
        Span::styled(
            arrow,
            Style::default()
                .fg(theme.border_subtle)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            title,
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    buf.set_line(inner.x, y, &line, inner.width);
    let rect = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: 1,
    };
    let hits = vec![crate::hit::HitRegion {
        rect,
        target: crate::hit::HitTarget::SidebarSection(title.to_string()),
    }];
    (y, hits)
}
