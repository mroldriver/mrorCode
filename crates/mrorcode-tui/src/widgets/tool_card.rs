//! 工具调用 inline 卡片（图标 + 摘要 + 状态色/spinner）。
//!
//! 长输出（>1 行）默认折叠为一行提示；对齐 opencode：展开后显示全部输出（无行数上限）。

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::render::str_width;
use crate::store::{ToolStatus, ToolView};
use crate::theme::Theme;

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
/// task 工具额外显示子 agent 实时投影（流式文本 + 工具调用活动）。
pub fn extra_lines(
    tool: &ToolView,
    expanded: bool,
    focused: bool,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    // 子 agent 实时投影（task 工具专用）
    if tool.name == "task" && (!tool.sub_agent_text.is_empty() || !tool.sub_agent_tools.is_empty())
    {
        out.push(styled_hint(
            "   \u{251C} sub-agent:", // ├
            focused,
            theme,
        ));
        // 子 agent 工具调用活动（最多显示最近 5 个）
        for sub in tool.sub_agent_tools.iter().rev().take(5) {
            let glyph = match sub.status {
                ToolStatus::Running => "\u{25D4}", // ◔
                ToolStatus::Ok => "\u{2713}",      // ✓
                ToolStatus::Failed => "\u{2717}",  // ✗
                ToolStatus::Waiting => "~",
            };
            let color = match sub.status {
                ToolStatus::Running => theme.primary,
                ToolStatus::Ok => theme.success,
                ToolStatus::Failed => theme.error,
                ToolStatus::Waiting => theme.warning,
            };
            out.push(Line::from(Span::styled(
                format!("   │ {glyph} {}", sub.name),
                Style::default().fg(color),
            )));
        }
        // 子 agent 流式文本（最多显示最后 3 行）
        if !tool.sub_agent_text.is_empty() {
            let lines: Vec<&str> = tool.sub_agent_text.trim_end().lines().collect();
            let preview = lines.iter().rev().take(3).collect::<Vec<_>>();
            for l in preview.iter().rev() {
                let clipped = clip(l, width);
                out.push(Line::from(Span::styled(
                    format!("   │ {clipped}"),
                    Style::default().fg(theme.text_muted),
                )));
            }
        }
    }

    let text = tool.output.trim_end();
    if text.is_empty() {
        return out;
    }
    let total = output_line_count(tool);

    if total == 1 {
        // 单行输出直接显示，不做折叠
        let clipped = clip(text, width);
        out.push(Line::from(Span::styled(
            format!("   {clipped}"),
            Style::default().fg(output_color(tool, theme)),
        )));
        return out;
    }

    if !expanded {
        let hint = format!("   \u{25B8} 输出 {total} 行（enter 展开）"); // ▸
        out.push(styled_hint(&hint, focused, theme));
        return out;
    }

    let color = output_color(tool, theme);
    let is_diff = crate::widgets::diff::looks_like_diff(text);
    for l in text.lines() {
        if is_diff {
            out.push(crate::widgets::diff::render_line(
                &clip(l, width),
                "   ",
                theme,
            ));
        } else {
            let clipped = clip(l, width);
            out.push(Line::from(Span::styled(
                format!("   {clipped}"),
                Style::default().fg(color),
            )));
        }
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
    let mut h = 0u16;

    // 子 agent 投影高度
    if tool.name == "task" && (!tool.sub_agent_text.is_empty() || !tool.sub_agent_tools.is_empty())
    {
        h += 1; // 标题行
        h += tool.sub_agent_tools.len().min(5) as u16; // 工具活动
        if !tool.sub_agent_text.is_empty() {
            let lines = tool.sub_agent_text.trim_end().lines().count();
            h += lines.min(3) as u16;
        }
    }

    let total = output_line_count(tool);
    if total == 0 {
        return h;
    }
    let add = if total == 1 || !expanded {
        1u16
    } else {
        u16::try_from(total.saturating_add(1)).unwrap_or(u16::MAX)
    };
    h = h.saturating_add(add);
    h
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
            sub_agent_text: String::new(),
            sub_agent_tools: Vec::new(),
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
    fn expanded_shows_all_lines_no_cap() {
        let raw = (0..20)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let t = tool(&raw);
        let lines = extra_lines(&t, true, false, 100, &Theme::tokyonight());
        // 20 输出 + 收起提示（无截断）
        assert_eq!(lines.len(), 21);
        assert_eq!(extra_height(&t, true), 21);
    }

    #[test]
    fn extra_height_saturates_on_huge_output() {
        // 超过 u16::MAX 行时不得回绕成小高度
        let mut t = tool("x");
        t.output = "x\n".repeat(70_000);
        let h = extra_height(&t, true);
        assert_eq!(h, u16::MAX);
    }
}
