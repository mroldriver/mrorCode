//! unified diff 行着色：识别工具输出里的 diff 并按前缀上色。
//!
//! write_file/edit_file 的结果文本形如：
//! ```text
//! Edited src/main.rs (1 replacement(s))
//! --- src/main.rs
//! +++ src/main.rs
//! @@ -1,3 +1,3 @@
//! -old
//! +new
//!  context
//! ```
//! 检测到 `@@ ` 行时判为 diff，仅对 diff 区段着色。

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// 输出是否包含统一 diff（用于决定是否走 diff 着色路径）。
pub fn looks_like_diff(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("@@ "))
}

/// 单行 diff 的样式；非 diff 行返回 `None`（由调用方走普通输出色）。
pub fn line_style(line: &str, theme: &Theme) -> Option<(Color, Color)> {
    if line.starts_with("@@ ") {
        Some((theme.diff_hunk_header, theme.background))
    } else if line.starts_with("+++ ") || line.starts_with("--- ") {
        Some((theme.text_muted, theme.background))
    } else if line.starts_with('+') {
        Some((theme.diff_added, theme.diff_added_bg))
    } else if line.starts_with('-') {
        Some((theme.diff_removed, theme.diff_removed_bg))
    } else if line.starts_with(' ') {
        Some((theme.diff_context, theme.diff_context_bg))
    } else {
        None
    }
}

/// 把一行 diff（或普通输出行）渲染为带缩进的 Span 列表。
pub fn render_line(text: &str, indent: &str, theme: &Theme) -> Line<'static> {
    match line_style(text, theme) {
        Some((fg, bg)) => Line::from(Span::styled(
            format!("{indent}{text}"),
            Style::default().fg(fg).bg(bg),
        )),
        None => Line::from(Span::raw(format!("{indent}{text}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_diff_by_hunk_header() {
        assert!(looks_like_diff("+++ a\n--- a\n@@ -1 +1 @@\n-x\n+y"));
        assert!(!looks_like_diff("just output\n"));
    }

    #[test]
    fn assigns_expected_colors() {
        let theme = Theme::tokyonight();
        assert_eq!(line_style("+added", &theme).unwrap().0, theme.diff_added);
        assert_eq!(
            line_style("-removed", &theme).unwrap().0,
            theme.diff_removed
        );
        assert_eq!(
            line_style("@@ -1 +1 @@", &theme).unwrap().0,
            theme.diff_hunk_header
        );
        assert!(line_style("plain", &theme).is_none());
    }
}
