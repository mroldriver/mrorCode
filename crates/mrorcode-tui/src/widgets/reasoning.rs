//! reasoning（思考过程）折叠块（M4-1）。
//!
//! 折叠态一行摘要 `▸ 思考过程 · N 行`；展开态渲染 reasoning_doc 全部行 + `▾ 收起`。
//! 行高必须与 `session` 的窗口化布局一致（见 `height`）。

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::render::str_width;
use crate::store::MessageView;
use crate::theme::Theme;

/// 折叠态占 1 行；展开态 = 内容行数 + 收起提示 1 行。
pub const COLLAPSED_HINT: &str = "思考过程";

pub fn is_present(m: &MessageView) -> bool {
    !m.reasoning.trim().is_empty()
}

/// 折叠/展开态下的附加行数（不含消息正文）。
pub fn height(m: &mut MessageView, width: u16, expanded: bool, gen: u64, theme: &Theme) -> u16 {
    if !is_present(m) {
        return 0;
    }
    if !expanded {
        return 1;
    }
    let lines = m.reasoning_doc.line_count(width, gen, theme);
    let lines = if lines == 0 { 1 } else { lines };
    (lines + 1) as u16
}

/// 生成折叠态摘要行。
pub fn collapsed_line(m: &MessageView, theme: &Theme, focused: bool) -> Line<'static> {
    let lines = m.reasoning.lines().count().max(1);
    let color = if focused {
        theme.primary
    } else {
        theme.text_muted
    };
    Line::from(vec![
        Span::styled("  \u{25B8} ", Style::default().fg(theme.secondary)), // ▸
        Span::styled(
            format!("{COLLAPSED_HINT} · {lines} 行"),
            Style::default().fg(color),
        ),
    ])
}

/// 生成展开态的全部行（内容 + 收起提示）。
pub fn expanded_lines(
    m: &mut MessageView,
    width: u16,
    gen: u64,
    theme: &Theme,
    focused: bool,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = m
        .reasoning_doc
        .lines(width, gen, theme)
        .into_iter()
        .map(|mut line| {
            // 缩进 + 左侧弱色竖线，与普通正文区分
            for span in &mut line.spans {
                span.style = span.style.fg(theme.text_muted);
            }
            let mut spans = vec![Span::styled(
                "  \u{2502} ",
                Style::default().fg(theme.border),
            )];
            spans.append(&mut line.spans);
            Line::from(spans)
        })
        .collect();
    if out.is_empty() {
        out.push(Line::raw(""));
    }
    let color = if focused {
        theme.primary
    } else {
        theme.text_muted
    };
    out.push(Line::from(Span::styled(
        "  \u{25BE} 收起",
        Style::default().fg(color).add_modifier(Modifier::DIM),
    )));
    out
}

/// 供测试断言：折叠行里出现的文本不含 reasoning 正文。
#[allow(dead_code)]
pub fn collapsed_width_hint(m: &MessageView) -> usize {
    str_width(&format!(
        "{COLLAPSED_HINT} · {} 行",
        m.reasoning.lines().count().max(1)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MessageView, Role};

    fn msg_with(reasoning: &str) -> MessageView {
        let mut m = MessageView::assistant(1);
        m.role = Role::Assistant;
        m.reasoning = reasoning.to_string();
        m.reasoning_doc.update(reasoning);
        m
    }

    #[test]
    fn absent_reasoning_has_zero_height() {
        let mut m = MessageView::assistant(1);
        assert!(!is_present(&m));
        assert_eq!(height(&mut m, 80, false, 0, &Theme::tokyonight()), 0);
    }

    #[test]
    fn collapsed_is_one_line_expanded_more() {
        let theme = Theme::tokyonight();
        let mut m = msg_with("step one\nstep two\nstep three");
        assert_eq!(height(&mut m, 80, false, 0, &theme), 1);
        // 3 行内容 + 1 行收起提示
        assert_eq!(height(&mut m, 80, true, 0, &theme), 4);
    }

    #[test]
    fn collapsed_line_hides_body() {
        let theme = Theme::tokyonight();
        let m = msg_with("secret reasoning body");
        let line = collapsed_line(&m, &theme, false);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("3 行") || text.contains("行"));
        assert!(!text.contains("secret"));
    }
}
