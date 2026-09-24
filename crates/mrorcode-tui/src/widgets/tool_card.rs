//! 工具调用 inline 卡片（图标 + 摘要 + 状态色/spinner）。
//!
//! 长输出（>1 行）默认折叠为一行提示，Tab 聚焦、Enter 切换；
//! 展开后最多展示 10 行，超出显示截断提示。

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::render::str_width;
use crate::store::{ToolStatus, ToolView};
use crate::theme::Theme;

/// 输出最多内联展示的行数。
const MAX_OUTPUT_LINES: usize = 10;

pub fn icon(name: &str) -> &'static str {
    match name {
        "bash" => "$",
        "read_file" => "\u{2192}", // →
        _ => "\u{2731}",           // ✱
    }
}

pub fn summary(tool: &ToolView) -> String {
    let parsed: serde_json::Value = serde_json::from_str(&tool.arguments).unwrap_or_default();
    let target = parsed
        .get("command")
        .and_then(|v| v.as_str())
        .or_else(|| parsed.get("path").and_then(|v| v.as_str()))
        .unwrap_or("");
    crate::render::truncate_end(target, 80)
}

/// 超过 1 行输出的卡片可折叠。
pub fn is_expandable(tool: &ToolView) -> bool {
    tool.output.trim().lines().count() > 1
}

fn output_line_count(tool: &ToolView) -> usize {
    tool.output.trim().lines().count()
}

pub fn line(tool: &ToolView, spinner: &str, theme: &Theme, focused: bool) -> Line<'static> {
    let (glyph, color) = match tool.status {
        ToolStatus::Waiting => ("~", theme.warning),
        ToolStatus::Running => (spinner, theme.primary),
        ToolStatus::Ok => ("\u{2713}", theme.success), // ✓
        ToolStatus::Failed => ("\u{2717}", theme.error), // ✗
    };
    // 焦点标记：❯；普通留空保持左竖线对齐
    let marker = if focused {
        Span::styled(" \u{276F} ", Style::default().fg(theme.primary)) // ❯
    } else {
        Span::raw("   ")
    };
    let name_color = if focused {
        theme.primary
    } else {
        theme.text_muted
    };
    let summary_color = if focused { theme.primary } else { theme.text };
    Line::from(vec![
        marker,
        Span::styled(format!(" {glyph} "), Style::default().fg(color)),
        Span::styled(
            format!("{} ", icon(&tool.name)),
            Style::default().fg(theme.secondary),
        ),
        Span::styled(format!("{} ", tool.name), Style::default().fg(name_color)),
        Span::styled(summary(tool), Style::default().fg(summary_color)),
    ])
}

/// 卡片行之外的附加行：短输出直接展示；长输出按折叠态给提示或内容。
pub fn extra_lines(
    tool: &ToolView,
    expanded: bool,
    focused: bool,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let text = tool.output.trim_end();
    if text.is_empty() {
        return Vec::new();
    }
    let total = output_line_count(tool);

    if total == 1 {
        // 单行输出直接显示，不做折叠
        let clipped = clip(text, width);
        return vec![Line::from(Span::styled(
            format!("   {clipped}"),
            Style::default().fg(output_color(tool, theme)),
        ))];
    }

    if !expanded {
        let hint = format!("   \u{25B8} 输出 {total} 行（enter 展开）"); // ▸
        return vec![styled_hint(&hint, focused, theme)];
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    let color = output_color(tool, theme);
    for l in text.lines().take(MAX_OUTPUT_LINES) {
        let clipped = clip(l, width);
        out.push(Line::from(Span::styled(
            format!("   {clipped}"),
            Style::default().fg(color),
        )));
    }
    if total > MAX_OUTPUT_LINES {
        out.push(Line::from(Span::styled(
            format!("   \u{2026} 共 {total} 行，仅显示前 {MAX_OUTPUT_LINES} 行"),
            Style::default().fg(theme.text_muted),
        )));
    }
    out.push(styled_hint("   \u{25BE} 收起（enter）", focused, theme)); // ▾
    out
}

fn styled_hint(text: &str, focused: bool, theme: &Theme) -> Line<'static> {
    let color = if focused {
        theme.primary
    } else {
        theme.text_muted
    };
    Line::from(Span::styled(text.to_string(), Style::default().fg(color)))
}

/// 附加行的高度（窗口化布局用），与 extra_lines 行数保持一致。
pub fn extra_height(tool: &ToolView, expanded: bool) -> u16 {
    let total = output_line_count(tool);
    if total == 0 {
        0
    } else if total == 1 || !expanded {
        1
    } else {
        let shown = total.min(MAX_OUTPUT_LINES) as u16;
        let truncated = u16::from(total > MAX_OUTPUT_LINES);
        shown + truncated + 1
    }
}

fn output_color(tool: &ToolView, theme: &Theme) -> ratatui::style::Color {
    if tool.status == ToolStatus::Failed {
        theme.error
    } else {
        theme.text_muted
    }
}

fn clip(l: &str, width: u16) -> String {
    let avail = width.saturating_sub(3) as usize;
    if str_width(l) > avail {
        crate::render::truncate_end(l, avail)
    } else {
        l.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(output: &str) -> ToolView {
        ToolView {
            id: "t".into(),
            name: "bash".into(),
            arguments: String::new(),
            status: ToolStatus::Ok,
            output: output.to_string(),
        }
    }

    #[test]
    fn empty_output_has_zero_height() {
        let t = tool("   ");
        assert!(!is_expandable(&t));
        assert_eq!(extra_height(&t, false), 0);
        assert!(extra_lines(&t, false, false, 100, &Theme::tokyonight()).is_empty());
    }

    #[test]
    fn single_line_always_shown_inline() {
        let t = tool("only one line\n");
        assert!(!is_expandable(&t));
        assert_eq!(extra_height(&t, false), 1);
        let lines = extra_lines(&t, false, false, 100, &Theme::tokyonight());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans[0].content.contains("only one line"));
    }

    #[test]
    fn multi_line_collapsed_is_one_hint() {
        let t = tool("a\nb\nc");
        assert!(is_expandable(&t));
        let lines = extra_lines(&t, false, false, 100, &Theme::tokyonight());
        assert_eq!(lines.len(), 1);
        assert_eq!(extra_height(&t, false), 1);
    }

    #[test]
    fn expanded_shows_all_plus_collapse_hint() {
        let t = tool("a\nb\nc");
        let lines = extra_lines(&t, true, false, 100, &Theme::tokyonight());
        assert_eq!(lines.len(), 4); // 3 输出 + 收起提示
        assert_eq!(extra_height(&t, true), 4);
    }

    #[test]
    fn expanded_caps_at_ten_with_truncation_note() {
        let raw = (0..12)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let t = tool(&raw);
        let lines = extra_lines(&t, true, false, 100, &Theme::tokyonight());
        // 10 行 + 截断提示 + 收起提示
        assert_eq!(lines.len(), 12);
        assert_eq!(extra_height(&t, true), 12);
    }
}
