//! syntect 代码高亮：用我们的 Theme syntax token 构造 syntect 主题，
//! 把高亮行映射成 ratatui Span。
//!
//! SyntaxSet 一次性 lazy 加载；高亮结果由 markdown block 缓存持有，
//! 主题切换经 theme_gen 失效，不会用错颜色。

use std::str::FromStr;
use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SynColor, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::SyntaxSet;

use crate::theme::Theme as AppTheme;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// 用应用主题的 syntax token 构造一个 syntect 主题。
fn syntect_theme(theme: &AppTheme) -> Theme {
    let code_bg = to_syn(theme.md_code_bg);

    fn item(scope: &str, color: Color, italic: bool, code_bg: SynColor) -> ThemeItem {
        // 我们的 scope 串都是字面常量；解析失败兜底 "source"（总有效）。
        let selectors = ScopeSelectors::from_str(scope)
            .unwrap_or_else(|_| ScopeSelectors::from_str("source").expect("source parses"));
        let mut font_style = FontStyle::empty();
        if italic {
            font_style.insert(FontStyle::ITALIC);
        }
        ThemeItem {
            scope: selectors,
            style: StyleModifier {
                foreground: Some(to_syn(color)),
                background: Some(code_bg),
                font_style: Some(font_style),
            },
        }
    }

    // 顺序不敏感：syntect 自身按 scope 特异度选最佳匹配
    let scopes = vec![
        item("comment", theme.syntax_comment, true, code_bg),
        item("keyword", theme.syntax_keyword, false, code_bg),
        item("keyword.operator", theme.syntax_operator, false, code_bg),
        item("constant.numeric", theme.syntax_number, false, code_bg),
        item("string", theme.syntax_string, false, code_bg),
        item(
            "entity.name.function",
            theme.syntax_function,
            false,
            code_bg,
        ),
        item("support.function", theme.syntax_function, false, code_bg),
        item("storage.type", theme.syntax_type, false, code_bg),
        item("support.type", theme.syntax_type, false, code_bg),
        item("entity.name.type", theme.syntax_type, false, code_bg),
        item("variable", theme.syntax_variable, false, code_bg),
        item("punctuation", theme.syntax_punctuation, false, code_bg),
    ];

    Theme {
        name: Some("mrorcode".to_string()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(to_syn(theme.text)),
            background: Some(code_bg),
            ..Default::default()
        },
        scopes,
    }
}

fn to_syn(c: Color) -> SynColor {
    match c {
        Color::Rgb(r, g, b) => SynColor { r, g, b, a: 0xFF },
        Color::Indexed(_) | Color::Reset => SynColor {
            r: 0xCC,
            g: 0xCC,
            b: 0xCC,
            a: 0xFF,
        },
        Color::Black => SynColor::BLACK,
        Color::Red => SynColor {
            r: 0xFF,
            g: 0,
            b: 0,
            a: 0xFF,
        },
        Color::Green => SynColor {
            r: 0,
            g: 0xFF,
            b: 0,
            a: 0xFF,
        },
        Color::Yellow => SynColor {
            r: 0xFF,
            g: 0xFF,
            b: 0,
            a: 0xFF,
        },
        Color::Blue => SynColor {
            r: 0,
            g: 0,
            b: 0xFF,
            a: 0xFF,
        },
        Color::Magenta => SynColor {
            r: 0xFF,
            g: 0,
            b: 0xFF,
            a: 0xFF,
        },
        Color::Cyan => SynColor {
            r: 0,
            g: 0xFF,
            b: 0xFF,
            a: 0xFF,
        },
        Color::Gray | Color::DarkGray => SynColor {
            r: 0x80,
            g: 0x80,
            b: 0x80,
            a: 0xFF,
        },
        Color::White => SynColor::WHITE,
        // ANSI 亮色（主题 token 正常都是 Rgb，这里只做兜底）
        Color::LightRed => SynColor {
            r: 0xFF,
            g: 0x5F,
            b: 0x5F,
            a: 0xFF,
        },
        Color::LightGreen => SynColor {
            r: 0x5F,
            g: 0xFF,
            b: 0x5F,
            a: 0xFF,
        },
        Color::LightYellow => SynColor {
            r: 0xFF,
            g: 0xFF,
            b: 0x5F,
            a: 0xFF,
        },
        Color::LightBlue => SynColor {
            r: 0x5F,
            g: 0x5F,
            b: 0xFF,
            a: 0xFF,
        },
        Color::LightMagenta => SynColor {
            r: 0xFF,
            g: 0x5F,
            b: 0xFF,
            a: 0xFF,
        },
        Color::LightCyan => SynColor {
            r: 0x5F,
            g: 0xFF,
            b: 0xFF,
            a: 0xFF,
        },
    }
}

fn from_syn(c: SynColor) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// 按语言高亮一行行文本；语言无法识别时返回 None（调用方走朴素底色）。
pub fn highlight_lines(lang: &str, body: &str, theme: &AppTheme) -> Option<Vec<Line<'static>>> {
    let syntax = find_syntax(lang)?;
    let syn_theme = syntect_theme(theme);
    let mut hl = HighlightLines::new(syntax, &syn_theme);

    let mut out: Vec<Line<'static>> = Vec::new();
    for raw_line in body.trim_end_matches('\n').lines() {
        let ranges = hl.highlight_line(raw_line, &SYNTAX_SET).ok()?;
        let spans: Vec<Span<'static>> = ranges
            .into_iter()
            .map(|(style, text)| {
                let mut s = Style::default()
                    .fg(from_syn(style.foreground))
                    .bg(theme.md_code_bg);
                if style.font_style.contains(FontStyle::ITALIC) {
                    s = s.add_modifier(Modifier::ITALIC);
                }
                if style.font_style.contains(FontStyle::BOLD) {
                    s = s.add_modifier(Modifier::BOLD);
                }
                Span::styled(text.to_string(), s)
            })
            .collect();
        out.push(Line::from(spans));
    }
    Some(out)
}

fn find_syntax(lang: &str) -> Option<&'static syntect::parsing::SyntaxReference> {
    let lang = lang.trim().to_ascii_lowercase();
    // syntect 默认包主要按扩展名识别；每个别名给一组候选依次尝试。
    let candidates: &[&str] = match lang.as_str() {
        "js" | "node" | "javascriptreact" | "jsx" | "mjs" | "cjs" => &["javascript", "js"],
        // syntect 默认包不含 TypeScript 语法：回退 JavaScript（词法大同小异）
        "ts" | "mts" | "cts" | "tsx" | "typescript" | "typescriptreact" => {
            &["typescript", "ts", "tsx", "javascript", "js"]
        }
        "sh" | "shell" | "zsh" | "fish" | "bash" => &["bash", "sh", "zsh", "fish"],
        "py" | "python" | "python3" => &["python", "py"],
        "rb" | "ruby" => &["ruby", "rb"],
        "rs" | "rust" => &["rust", "rs"],
        "md" | "markdown" => &["markdown", "md"],
        "yml" | "yaml" => &["yaml", "yml"],
        "c++" | "cxx" | "cpp" => &["cpp", "cxx", "c++"],
        other => &[other],
    };
    for token in candidates {
        if let Some(s) = SYNTAX_SET.find_syntax_by_token(token) {
            return Some(s);
        }
    }
    // 最后按语法显示名大小写不敏感匹配
    SYNTAX_SET
        .syntaxes()
        .iter()
        .find(|s| s.name.to_ascii_lowercase() == lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_language_highlights_every_line() {
        let theme = AppTheme::tokyonight();
        let code = "fn main() {\n    // hi\n    let x = 1;\n}\n";
        let lines = highlight_lines("rust", code, &theme).expect("rust syntax recognized");
        assert_eq!(lines.len(), 4);
        // 至少产生一个非纯白 span（关键字/注释等被着色）
        let colored = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg.is_some_and(|c| c != theme.text))
        });
        assert!(colored, "expected syntax-tinted spans");
    }

    #[test]
    fn unknown_language_returns_none() {
        let theme = AppTheme::tokyonight();
        assert!(highlight_lines("nonsense-lang-xyz", "abc", &theme).is_none());
    }

    #[test]
    fn aliases_resolve() {
        assert!(find_syntax("sh").is_some());
        assert!(find_syntax("zsh").is_some());
        assert!(find_syntax("ts").is_some());
        assert!(find_syntax("tsx").is_some());
        assert!(find_syntax("c++").is_some());
        assert!(find_syntax("rust").is_some());
        assert!(find_syntax("yml").is_some());
    }
}
