//! 屏幕矩形选区模型（M4-6）。
//!
//! 选区从最终 [`ratatui::buffer::Buffer`] 按行提取 symbol 文本，
//! 不依赖中间渲染结构，纯函数、可用 TestBackend 单测。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::theme::Theme;

/// 屏幕坐标（列, 行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub x: u16,
    pub y: u16,
}

/// 以 (start, end) 界定的矩形选区；两点顺序不敏感。
#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub anchor: Pos,
    pub cursor: Pos,
}

impl Selection {
    pub fn new(anchor: Pos) -> Self {
        Self {
            anchor,
            cursor: anchor,
        }
    }

    /// 归一化为 (top_left, bottom_right)。
    pub fn bounds(&self) -> (Pos, Pos) {
        let (s, e) = (self.anchor, self.cursor);
        (
            Pos {
                x: s.x.min(e.x),
                y: s.y.min(e.y),
            },
            Pos {
                x: s.x.max(e.x),
                y: s.y.max(e.y),
            },
        )
    }

    pub fn contains(&self, x: u16, y: u16) -> bool {
        let (a, b) = self.bounds();
        x >= a.x && x <= b.x && y >= a.y && y <= b.y
    }

    /// 从屏幕行文本提取选中文本：行尾去空白、跨行以 `\n` 连接。
    pub fn extract(&self, rows: &[String]) -> String {
        let (a, b) = self.bounds();
        let mut out: Vec<String> = Vec::new();
        for y in a.y..=b.y {
            let Some(row) = rows.get(y as usize) else {
                continue;
            };
            let chars: Vec<char> = row.chars().collect();
            let x0 = (a.x as usize).min(chars.len());
            let x1 = ((b.x as usize) + 1).min(chars.len());
            let seg: String = chars[x0..x1].iter().collect();
            out.push(seg.trim_end().to_string());
        }
        out.join("\n")
    }
}

/// 从渲染后的缓冲区抓取每行文本（每帧末调用一次）。
pub fn rows_from_buffer(buf: &Buffer) -> Vec<String> {
    let area = buf.area();
    let mut rows = Vec::with_capacity(area.height as usize);
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        rows.push(line);
    }
    rows
}

/// 用反色高亮选区（在全部内容绘制后叠加）。
pub fn paint(sel: &Selection, buf: &mut Buffer, _theme: &Theme) {
    let (a, b) = sel.bounds();
    let area = buf.area();
    let (left, right, top, bottom) = (area.left(), area.right(), area.top(), area.bottom());
    for y in a.y..=b.y {
        if y < top || y >= bottom {
            continue;
        }
        for x in a.x..=b.x {
            if x < left || x >= right {
                continue;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

/// 选区覆盖率矩形（未使用时允许保留）。
#[allow(dead_code)]
pub fn selection_rect(sel: &Selection) -> Rect {
    let (a, b) = sel.bounds();
    Rect {
        x: a.x,
        y: a.y,
        width: b.x.saturating_sub(a.x).saturating_add(1),
        height: b.y.saturating_sub(a.y).saturating_add(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn extract_covers_range_and_trims() {
        let rows = vec!["hello".to_string(), "world  ".to_string()];
        let sel = Selection {
            anchor: Pos { x: 1, y: 0 },
            cursor: Pos { x: 3, y: 1 },
        };
        assert_eq!(sel.extract(&rows), "ell\norl");
    }

    #[test]
    fn rows_from_buffer_roundtrip() {
        let mut term = Terminal::new(TestBackend::new(6, 2)).unwrap();
        term.draw(|f| {
            f.buffer_mut()
                .set_string(0, 0, "abc", ratatui::style::Style::default());
        })
        .unwrap();
        let rows = rows_from_buffer(term.backend().buffer());
        assert_eq!(rows.len(), 2);
        assert!(rows[0].starts_with("abc"));
    }

    #[test]
    fn bounds_are_order_insensitive() {
        let sel = Selection {
            anchor: Pos { x: 5, y: 2 },
            cursor: Pos { x: 1, y: 0 },
        };
        let (a, b) = sel.bounds();
        assert_eq!((a.x, a.y), (1, 0));
        assert_eq!((b.x, b.y), (5, 2));
    }
}
