//! Toast 浮层：底部居中、单行、自下而上堆叠；过期清理由 app 慢节拍负责。（M4-11 分区）

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::render::{str_width, truncate_end};
use crate::theme::Theme;

/// toast 级别：决定左边框配色（info=primary / warn=warning / error=error）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Warn,
    Error,
}

impl ToastLevel {
    /// 主题配色（左竖线）。
    pub fn color(self, theme: &Theme) -> Color {
        match self {
            ToastLevel::Info => theme.primary,
            ToastLevel::Warn => theme.warning,
            ToastLevel::Error => theme.error,
        }
    }
}

/// 在屏幕底部居中绘制 toast（顺序：最新在下）。
/// `skip_bottom`：底部保留行数（which-key/补全浮层占用时避让，含原末行）。
pub fn render(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    toasts: &[(ToastLevel, &str)],
    skip_bottom: u16,
) {
    let mut y = area
        .y
        .saturating_add(area.height)
        .saturating_sub(skip_bottom + 2);
    for (level, text) in toasts {
        if y < area.y {
            break;
        }
        let max_w = area.width.saturating_sub(4).max(8);
        let raw_w = (str_width(text) as u16).saturating_add(4);
        let w = raw_w.min(max_w);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let line_rect = Rect {
            x,
            y,
            width: w,
            height: 1,
        };

        // 菜单底色块
        for cx in line_rect.x..line_rect.x + line_rect.width {
            if let Some(cell) = buf.cell_mut((cx, y)) {
                cell.set_style(Style::default().bg(theme.background_menu));
                cell.set_symbol(" ");
            }
        }
        // 左边框按级别着色 （M4-11）
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol("\u{2503}") // ┃
                .set_style(
                    Style::default()
                        .fg(level.color(theme))
                        .bg(theme.background_menu),
                );
        }
        let body = truncate_end(text, (w - 3) as usize);
        if let Some(cell) = buf.cell_mut((x + 1, y)) {
            cell.set_symbol(" ")
                .set_style(Style::default().bg(theme.background_menu));
        }
        buf.set_stringn(
            x + 2,
            y,
            body,
            (w - 3) as usize,
            Style::default()
                .fg(theme.selected_text)
                .bg(theme.background_menu),
        );
        y = y.saturating_sub(1);
    }
}

// ---------- 单测（M4-11） ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_to_row(
        toasts: &[(ToastLevel, &str)],
        skip_bottom: u16,
    ) -> (ratatui::buffer::Cell, ratatui::layout::Position) {
        let mut term = Terminal::new(TestBackend::new(40, 5)).expect("terminal");
        let theme = Theme::default();
        term.draw(|f| {
            render(f.area(), f.buffer_mut(), &theme, toasts, skip_bottom);
        })
        .expect("draw");
        let buf = term.backend().buffer();
        let y = 5 - skip_bottom - 2;
        let x = (40u16.saturating_sub((4 + 4) as u16)) / 2; // 文本 "test" 宽 4
        (
            buf.cell((x, y)).unwrap().clone(),
            ratatui::layout::Position { x, y },
        )
    }

    #[test]
    fn toast_left_border_uses_level_color() {
        let t = Theme::default();
        let (cell, _pos) = render_to_row(&[(ToastLevel::Error, "boom")], 0);
        // 左竖线应取 error 色
        assert_eq!(cell.style().fg, Some(t.error));
    }

    #[test]
    fn toast_skips_bottom_rows() {
        let t = Theme::default();
        // skip_bottom=2 时 toast 上移一行：y = 5-2-2 = 1
        let (cell, pos) = render_to_row(&[(ToastLevel::Info, "test")], 2);
        assert_eq!(pos.y, 1);
        assert_eq!(cell.style().fg, Some(t.primary));
    }
}
