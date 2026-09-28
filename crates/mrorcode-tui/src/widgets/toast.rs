//! Toast 浮层：对齐 opencode——右上角、单条、5s 过期（新 toast 替换当前）。
//! 支持多行文本（自动折行），左右竖条按级别配色。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Wrap;

use crate::theme::Theme;

/// toast 级别：决定竖条配色（info=primary / warn=warning / error=error）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Warn,
    Error,
}

impl ToastLevel {
    /// 主题配色（竖条）。
    pub fn color(self, theme: &Theme) -> Color {
        match self {
            ToastLevel::Info => theme.primary,
            ToastLevel::Warn => theme.warning,
            ToastLevel::Error => theme.error,
        }
    }
}

/// 在屏幕右上角绘制单条 toast（多行自动折行）。
pub fn render(area: Rect, buf: &mut Buffer, theme: &Theme, level: ToastLevel, text: &str) {
    let max_w = area.width.saturating_sub(4).clamp(8, 60);
    let mut lines = wrap_text(text, max_w.saturating_sub(2) as usize);
    if lines.is_empty() {
        lines.push(String::new());
    }
    let w = max_w;
    let x = area.x + area.width.saturating_sub(w) - 2;
    let y = area.y + 2;

    for (i, line) in lines.iter().enumerate() {
        let row = y + i as u16;
        // 面板底色
        for cx in x..x + w {
            if let Some(cell) = buf.cell_mut((cx, row)) {
                cell.set_style(Style::default().bg(theme.background_panel));
                cell.set_symbol(" ");
            }
        }
        // 左竖条按级别着色
        if let Some(cell) = buf.cell_mut((x, row)) {
            cell.set_symbol("\u{2503}") // ┃
                .set_style(
                    Style::default()
                        .fg(level.color(theme))
                        .bg(theme.background_panel),
                );
        }
        buf.set_stringn(
            x + 1,
            row,
            line,
            (w - 2) as usize,
            Style::default()
                .fg(theme.selected_text)
                .bg(theme.background_panel),
        );
    }
    let _ = Modifier::empty();
    let _ = Wrap { trim: false };
}

/// 按显示宽度对文本做词边界折行（长单词硬切）。
fn wrap_text(text: &str, max_w: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        if crate::render::str_width(raw) <= max_w {
            out.push(raw.to_string());
            continue;
        }
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for word in raw.split_inclusive(' ') {
            let ww = crate::render::str_width(word);
            if cur_w + ww > max_w && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            if ww > max_w {
                // 超长 token 硬切
                for ch in word.chars() {
                    let cw = unicode_width(ch);
                    if cur_w + cw > max_w {
                        out.push(std::mem::take(&mut cur));
                        cur_w = 0;
                    }
                    cur.push(ch);
                    cur_w += cw;
                }
            } else {
                cur.push_str(word);
                cur_w += ww;
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

fn unicode_width(ch: char) -> usize {
    use unicode_width::UnicodeWidthChar;
    ch.width().unwrap_or(0)
}

// ---------- 单测 ----------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn long_text_wraps_to_multiple_rows() {
        let text = "这是一条很长的提示信息用于验证 toast 会自动折行而不是溢出屏幕右边界";
        let lines = wrap_text(text, 20);
        assert!(lines.len() >= 2);
        for l in &lines {
            assert!(crate::render::str_width(l) <= 20);
        }
    }

    #[test]
    fn renders_top_right_with_level_border() {
        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("terminal");
        let theme = Theme::default();
        term.draw(|f| {
            render(f.area(), f.buffer_mut(), &theme, ToastLevel::Error, "boom");
        })
        .unwrap();
        let buf = term.backend().buffer();
        // 右上角区域应有 error 色竖条（x = 40-w-2，w=min(36,60)=36 → x=2）
        let cell = buf.cell((2, 2)).unwrap();
        assert_eq!(cell.style().fg, Some(theme.error));
    }
}
