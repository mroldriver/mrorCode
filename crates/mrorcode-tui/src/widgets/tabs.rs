//! 多会话标签栏（M4-4）。
//!
//! 顶部一行列出全部会话：active 高亮、busy spinner、标题截断；返回每个标签的屏幕矩形
//! 供鼠标命中测试（步骤 5）。窄屏时只显示 active 标签。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::render::{str_width, take_by_width, truncate_end};
use crate::store::AppState;
use crate::theme::Theme;

/// 标签栏高度（1 行）。
pub const HEIGHT: u16 = 1;

/// 一个标签的命中矩形（相对整屏坐标）。
pub struct TabHit {
    pub session_id: u64,
    pub rect: Rect,
}

/// 渲染标签栏，返回标签命中区域（点击切换用）。
pub fn render(state: &AppState, area: Rect, buf: &mut Buffer, theme: &Theme) -> Vec<TabHit> {
    // 背景
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_style(Style::default().bg(theme.background_panel));
            cell.set_symbol(" ");
        }
    }

    let mut hits = Vec::new();
    let mut x = area.x;
    let end = area.x + area.width;
    let compact = area.width < 60;

    for session in &state.sessions {
        let active = session.id == state.active_session;
        let marker = if active { "\u{25CF} " } else { "  " };
        let spinner = if session.phase.is_busy() {
            format!("{} ", state.spinner_glyph())
        } else {
            String::new()
        };
        let label = truncate_end(&session.title, 24);
        let text = format!("{marker}{spinner}{label} ");
        let w = str_width(&text) as u16;

        // 窄屏或空间不足：只保留 active 标签
        if compact && !active {
            continue;
        }
        if x + w > end {
            // 剩余空间放不下：截断收尾
            let avail = end.saturating_sub(x);
            if avail < 4 {
                break;
            }
            let clipped = take_by_width(&text, avail as usize);
            let rect = Rect {
                x,
                y: area.y,
                width: str_width(clipped) as u16,
                height: 1,
            };
            paint(buf, rect, clipped, active, theme, true);
            hits.push(TabHit {
                session_id: session.id,
                rect,
            });
            break;
        }

        let rect = Rect {
            x,
            y: area.y,
            width: w,
            height: 1,
        };
        paint(buf, rect, &text, active, theme, false);
        hits.push(TabHit {
            session_id: session.id,
            rect,
        });
        x += w;
    }
    hits
}

fn paint(buf: &mut Buffer, rect: Rect, text: &str, active: bool, theme: &Theme, dimmed: bool) {
    let (fg, bg, add) = if active {
        (theme.primary, theme.background_element, Modifier::BOLD)
    } else {
        (theme.text_muted, theme.background_panel, Modifier::empty())
    };
    for x in rect.x..rect.x + rect.width {
        if let Some(cell) = buf.cell_mut((x, rect.y)) {
            cell.set_style(Style::default().bg(bg));
            cell.set_symbol(" ");
        }
    }
    let style = if dimmed {
        Style::default().fg(theme.text_muted).bg(bg)
    } else {
        Style::default().fg(fg).bg(bg).add_modifier(add)
    };
    buf.set_stringn(rect.x, rect.y, text, rect.width as usize, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn state_with_two() -> AppState {
        use crate::storage::Store;
        use std::path::PathBuf;
        let path = std::env::temp_dir().join(format!("mrorcode-tabs-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (kv, _) = Store::load_from(&path);
        let mut state = AppState::new("m".into(), Vec::new(), PathBuf::from("."), kv);
        state.sessions[0].title = "first".into();
        state.new_session();
        state.sessions[1].title = "second".into();
        state
    }

    fn contains(term: &Terminal<TestBackend>, needle: &str) -> bool {
        let buf = term.backend().buffer();
        let area = buf.area();
        for y in area.top()..area.bottom() {
            let mut row = String::new();
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell((x, y)) {
                    row.push_str(cell.symbol());
                }
            }
            if row.contains(needle) {
                return true;
            }
        }
        false
    }

    #[test]
    fn renders_all_sessions_and_hits() {
        let state = state_with_two();
        let mut term = Terminal::new(TestBackend::new(120, 3)).unwrap();
        let mut hits = Vec::new();
        term.draw(|f| {
            hits = render(&state, f.area(), f.buffer_mut(), &Theme::tokyonight());
        })
        .unwrap();
        assert!(contains(&term, "first"));
        assert!(contains(&term, "second"));
        assert_eq!(hits.len(), 2);
        // 命中矩形不重叠且递增
        assert!(hits[0].rect.x < hits[1].rect.x);
    }

    #[test]
    fn narrow_screen_keeps_active_only() {
        let state = state_with_two();
        let mut term = Terminal::new(TestBackend::new(40, 3)).unwrap();
        let mut hits = Vec::new();
        term.draw(|f| {
            hits = render(&state, f.area(), f.buffer_mut(), &Theme::tokyonight());
        })
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, state.active_session);
    }
}
