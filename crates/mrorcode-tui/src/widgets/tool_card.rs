//! 工具调用 inline 一行卡（图标 + 摘要 + 状态色/spinner）。

use ratatui::style::Style;
use ratatui::text::{Line, Span};

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

pub fn line(tool: &ToolView, spinner: &str, theme: &Theme) -> Line<'static> {
    let (glyph, color) = match tool.status {
        ToolStatus::Waiting => ("~", theme.warning),
        ToolStatus::Running => (spinner, theme.primary),
        ToolStatus::Ok => ("\u{2713}", theme.success), // ✓
        ToolStatus::Failed => ("\u{2717}", theme.error), // ✗
    };
    Line::from(vec![
        Span::styled(format!(" {glyph} "), Style::default().fg(color)),
        Span::styled(
            format!("{} ", icon(&tool.name)),
            Style::default().fg(theme.secondary),
        ),
        Span::styled(
            format!("{} ", tool.name),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled(summary(tool), Style::default().fg(theme.text)),
    ])
}

/// 失败/有输出的工具，M1 输出以等宽淡化行展示（M2 做折叠卡）。
pub fn output_lines(tool: &ToolView, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let text = tool.output.trim_end();
    if text.is_empty() {
        return out;
    }
    for l in text.lines().take(10) {
        let clipped = if crate::render::str_width(l) > width as usize {
            crate::render::truncate_end(l, width.saturating_sub(2).into())
        } else {
            l.to_string()
        };
        let color = if tool.status == ToolStatus::Failed {
            theme.error
        } else {
            theme.text_muted
        };
        out.push(Line::from(Span::styled(
            format!("   {clipped}"),
            Style::default().fg(color),
        )));
    }
    if text.lines().count() > 10 {
        out.push(Line::from(Span::styled(
            "   ...[output truncated]".to_string(),
            Style::default().fg(theme.text_muted),
        )));
    }
    out
}
