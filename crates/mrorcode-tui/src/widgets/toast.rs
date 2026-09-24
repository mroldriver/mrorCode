//! Toast 浮层：底部居中、单行、自下而上堆叠；过期清理由 app 慢节拍负责。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::render::{str_width, truncate_end};
use crate::theme::Theme;

/// 在屏幕底部居中绘制 toast（顺序：最新在下）。
pub fn render(area: Rect, buf: &mut Buffer, theme: &Theme, toasts: &[&str]) {
    let mut y = area.y.saturating_add(area.height).saturating_sub(2);
    for text in toasts {
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
        // 左右边框色点缀
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol("\u{2503}") // ┃
                .set_style(Style::default().fg(theme.primary).bg(theme.background_menu));
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
