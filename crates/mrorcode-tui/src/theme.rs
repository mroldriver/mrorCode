//! 主题系统：直接加载 opencode 格式主题 JSON（MIT vendored，见
//! licenses/OPENCODE-LICENSE）。
//!
//! 支持：`#hex` / defs 引用（含多级、循环检测）/ `{dark,light}` 双值 /
//! `transparent`、`none`（回退内置默认）/ ANSI 数字。
//! 缺 token 用内置 tokyonight 补齐，解析失败不阻塞启动。

use std::collections::HashSet;

use ratatui::style::Color;
use serde_json::Value;

/// 字段与 opencode 主题 JSON token 对齐；部分 M2 暂未使用（M4 diff 等）。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Theme {
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub info: Color,

    pub text: Color,
    pub text_muted: Color,
    pub selected_text: Color,

    pub background: Color,
    pub background_panel: Color,
    pub background_element: Color,
    pub background_menu: Color,

    pub border: Color,
    pub border_active: Color,
    pub border_subtle: Color,

    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_context: Color,
    pub diff_hunk_header: Color,
    pub diff_highlight_added: Color,
    pub diff_highlight_removed: Color,
    pub diff_added_bg: Color,
    pub diff_removed_bg: Color,
    pub diff_context_bg: Color,
    pub diff_line_number: Color,
    pub diff_added_line_number_bg: Color,
    pub diff_removed_line_number_bg: Color,

    pub md_text: Color,
    pub md_heading: Color,
    pub md_link: Color,
    pub md_link_text: Color,
    pub md_code: Color,
    pub md_blockquote: Color,
    pub md_emph: Color,
    pub md_strong: Color,
    pub md_horizontal_rule: Color,
    pub md_list_item: Color,
    pub md_list_enumeration: Color,
    pub md_image: Color,
    pub md_image_text: Color,
    pub md_code_bg: Color,
    pub md_code_block: Color,

    pub syntax_comment: Color,
    pub syntax_keyword: Color,
    pub syntax_function: Color,
    pub syntax_variable: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_type: Color,
    pub syntax_operator: Color,
    pub syntax_punctuation: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }

    pub fn parse(s: &str) -> Mode {
        match s.to_ascii_lowercase().as_str() {
            "light" => Mode::Light,
            _ => Mode::Dark,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::tokyonight()
    }
}

impl Theme {
    /// 内置暗色兜底主题（参考 tokyonight night 调色板）。
    pub fn tokyonight() -> Self {
        Self {
            primary: Color::Rgb(0x7a, 0xa2, 0xf7),
            secondary: Color::Rgb(0xbb, 0x9a, 0xf7),
            accent: Color::Rgb(0x7d, 0xcf, 0xff),
            error: Color::Rgb(0xf7, 0x76, 0x8e),
            warning: Color::Rgb(0xe0, 0xaf, 0x68),
            success: Color::Rgb(0x9e, 0xce, 0x6a),
            info: Color::Rgb(0x7d, 0xcf, 0xff),

            text: Color::Rgb(0xc0, 0xca, 0xf5),
            text_muted: Color::Rgb(0x56, 0x5f, 0x89),
            selected_text: Color::Rgb(0x1a, 0x1b, 0x26),

            background: Color::Rgb(0x1a, 0x1b, 0x26),
            background_panel: Color::Rgb(0x22, 0x24, 0x33),
            background_element: Color::Rgb(0x2d, 0x31, 0x45),
            background_menu: Color::Rgb(0x29, 0x2e, 0x42),

            border: Color::Rgb(0x3b, 0x42, 0x61),
            border_active: Color::Rgb(0x7a, 0xa2, 0xf7),
            border_subtle: Color::Rgb(0x2d, 0x31, 0x45),

            diff_added: Color::Rgb(0x9e, 0xce, 0x6a),
            diff_removed: Color::Rgb(0xf7, 0x76, 0x8e),
            diff_context: Color::Rgb(0x56, 0x5f, 0x89),
            diff_hunk_header: Color::Rgb(0x7d, 0xcf, 0xff),
            diff_highlight_added: Color::Rgb(0x36, 0x50, 0x4a),
            diff_highlight_removed: Color::Rgb(0x5a, 0x3a, 0x47),
            diff_added_bg: Color::Rgb(0x23, 0x2e, 0x2b),
            diff_removed_bg: Color::Rgb(0x33, 0x24, 0x2c),
            diff_context_bg: Color::Rgb(0x24, 0x28, 0x3b),
            diff_line_number: Color::Rgb(0x56, 0x5f, 0x89),
            diff_added_line_number_bg: Color::Rgb(0x2d, 0x44, 0x36),
            diff_removed_line_number_bg: Color::Rgb(0x46, 0x2d, 0x36),

            md_text: Color::Rgb(0xc0, 0xca, 0xf5),
            md_heading: Color::Rgb(0x7a, 0xa2, 0xf7),
            md_link: Color::Rgb(0x7d, 0xcf, 0xff),
            md_link_text: Color::Rgb(0x82, 0xaa, 0xf7),
            md_code: Color::Rgb(0xe0, 0xaf, 0x68),
            md_blockquote: Color::Rgb(0x56, 0x5f, 0x89),
            md_emph: Color::Rgb(0xc0, 0xca, 0xf5),
            md_strong: Color::Rgb(0xc0, 0xca, 0xf5),
            md_horizontal_rule: Color::Rgb(0x3b, 0x42, 0x61),
            md_list_item: Color::Rgb(0x9e, 0xce, 0x6a),
            md_list_enumeration: Color::Rgb(0x9e, 0xce, 0x6a),
            md_image: Color::Rgb(0x7d, 0xcf, 0xff),
            md_image_text: Color::Rgb(0x82, 0xaa, 0xf7),
            md_code_bg: Color::Rgb(0x24, 0x28, 0x3b),
            md_code_block: Color::Rgb(0x24, 0x28, 0x3b),

            syntax_comment: Color::Rgb(0x56, 0x5f, 0x89),
            syntax_keyword: Color::Rgb(0xbb, 0x9a, 0xf7),
            syntax_function: Color::Rgb(0x7d, 0xcf, 0xff),
            syntax_variable: Color::Rgb(0xc0, 0xca, 0xf5),
            syntax_string: Color::Rgb(0x9e, 0xce, 0x6a),
            syntax_number: Color::Rgb(0xff, 0x9e, 0x64),
            syntax_type: Color::Rgb(0x2a, 0xc3, 0xde),
            syntax_operator: Color::Rgb(0x89, 0xdd, 0xdd),
            syntax_punctuation: Color::Rgb(0x86, 0x8c, 0xb0),
        }
    }
}

// ---------------- 解析 ----------------

include!(concat!(env!("OUT_DIR"), "/themes_bundled.rs"));

/// 已解析的主题集合（给定模式）。
pub struct ThemeSet {
    /// (主题名, 主题)，按名称排序。
    themes: Vec<(String, Theme)>,
}

impl ThemeSet {
    /// 解析全部内置主题；无法解析的条目跳过（调用方无需处理错误）。
    pub fn bundled(mode: Mode) -> Self {
        let mut themes = Vec::new();
        for (name, json) in BUNDLED {
            if let Some(theme) = parse(json, mode) {
                themes.push((name.to_string(), theme));
            }
        }
        themes.sort_by(|a, b| a.0.cmp(&b.0));
        ThemeSet { themes }
    }

    /// 从用户目录 `~/.mrorcode/themes/*.json` 加载主题；同名覆盖内置。
    pub fn with_user_themes(self, mode: Mode) -> Self {
        let Some(home) = dirs::home_dir() else {
            return self;
        };
        let dir = home.join(".mrorcode").join("themes");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return self;
        };
        let mut themes = self.themes;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            let Ok(json) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(theme) = parse(&json, mode) {
                // 覆盖同名内置主题
                if let Some(slot) = themes.iter_mut().find(|(n, _)| *n == name) {
                    slot.1 = theme;
                } else {
                    themes.push((name, theme));
                }
            }
        }
        themes.sort_by(|a, b| a.0.cmp(&b.0));
        ThemeSet { themes }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.themes.iter().map(|(n, _)| n.as_str())
    }

    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.themes.iter().find(|(n, _)| n == name).map(|(_, t)| t)
    }

    pub fn len(&self) -> usize {
        self.themes.len()
    }
}

/// 解析单个主题 JSON；任何 token 失败只跳过该 token（用兜底默认），
/// 仅当 `theme` 段整体缺失/非法时返回 None。
pub fn parse(json_text: &str, mode: Mode) -> Option<Theme> {
    let root: Value = serde_json::from_str(json_text).ok()?;
    let theme_raw = root.get("theme")?;
    let defs = root.get("defs");
    let mut theme = Theme::tokyonight();

    let overlay = |field: &str, set: &mut dyn FnMut(Color)| {
        if let Some(v) = theme_raw.get(field) {
            if let Some(c) = resolve_value(v, defs, mode, &mut HashSet::new()) {
                set(c);
            }
        }
    };
    // 语义
    overlay("primary", &mut |c| theme.primary = c);
    overlay("secondary", &mut |c| theme.secondary = c);
    overlay("accent", &mut |c| theme.accent = c);
    overlay("error", &mut |c| theme.error = c);
    overlay("warning", &mut |c| theme.warning = c);
    overlay("success", &mut |c| theme.success = c);
    overlay("info", &mut |c| theme.info = c);
    // 文本/背景
    overlay("text", &mut |c| theme.text = c);
    overlay("textMuted", &mut |c| theme.text_muted = c);
    overlay("selectedListItemText", &mut |c| theme.selected_text = c);
    overlay("background", &mut |c| theme.background = c);
    overlay("backgroundPanel", &mut |c| theme.background_panel = c);
    overlay("backgroundElement", &mut |c| theme.background_element = c);
    overlay("backgroundMenu", &mut |c| theme.background_menu = c);
    // 边框/diff
    overlay("border", &mut |c| theme.border = c);
    overlay("borderActive", &mut |c| theme.border_active = c);
    overlay("borderSubtle", &mut |c| theme.border_subtle = c);
    overlay("diffAdded", &mut |c| theme.diff_added = c);
    overlay("diffRemoved", &mut |c| theme.diff_removed = c);
    overlay("diffContext", &mut |c| theme.diff_context = c);
    overlay("diffHunkHeader", &mut |c| theme.diff_hunk_header = c);
    overlay("diffHighlightAdded", &mut |c| {
        theme.diff_highlight_added = c
    });
    overlay("diffHighlightRemoved", &mut |c| {
        theme.diff_highlight_removed = c
    });
    overlay("diffAddedBg", &mut |c| theme.diff_added_bg = c);
    overlay("diffRemovedBg", &mut |c| theme.diff_removed_bg = c);
    overlay("diffContextBg", &mut |c| theme.diff_context_bg = c);
    overlay("diffLineNumber", &mut |c| theme.diff_line_number = c);
    overlay("diffAddedLineNumberBg", &mut |c| {
        theme.diff_added_line_number_bg = c
    });
    overlay("diffRemovedLineNumberBg", &mut |c| {
        theme.diff_removed_line_number_bg = c
    });
    // markdown
    overlay("markdownText", &mut |c| theme.md_text = c);
    overlay("markdownHeading", &mut |c| theme.md_heading = c);
    overlay("markdownLink", &mut |c| theme.md_link = c);
    overlay("markdownLinkText", &mut |c| theme.md_link_text = c);
    overlay("markdownCode", &mut |c| theme.md_code = c);
    overlay("markdownBlockQuote", &mut |c| theme.md_blockquote = c);
    overlay("markdownEmph", &mut |c| theme.md_emph = c);
    overlay("markdownStrong", &mut |c| theme.md_strong = c);
    overlay("markdownHorizontalRule", &mut |c| {
        theme.md_horizontal_rule = c
    });
    overlay("markdownListItem", &mut |c| theme.md_list_item = c);
    overlay("markdownListEnumeration", &mut |c| {
        theme.md_list_enumeration = c
    });
    overlay("markdownImage", &mut |c| theme.md_image = c);
    overlay("markdownImageText", &mut |c| theme.md_image_text = c);
    overlay("markdownCodeBlock", &mut |c| theme.md_code_block = c);
    // syntax
    overlay("syntaxComment", &mut |c| theme.syntax_comment = c);
    overlay("syntaxKeyword", &mut |c| theme.syntax_keyword = c);
    overlay("syntaxFunction", &mut |c| theme.syntax_function = c);
    overlay("syntaxVariable", &mut |c| theme.syntax_variable = c);
    overlay("syntaxString", &mut |c| theme.syntax_string = c);
    overlay("syntaxNumber", &mut |c| theme.syntax_number = c);
    overlay("syntaxType", &mut |c| theme.syntax_type = c);
    overlay("syntaxOperator", &mut |c| theme.syntax_operator = c);
    overlay("syntaxPunctuation", &mut |c| theme.syntax_punctuation = c);

    Some(theme)
}

/// 解析一个颜色值（含 defs 引用解析与循环检测）；无法解析返回 None（调用方保留默认）。
fn resolve_value(
    v: &Value,
    defs: Option<&Value>,
    mode: Mode,
    seen: &mut HashSet<String>,
) -> Option<Color> {
    match v {
        Value::String(s) => resolve_string(s, defs, mode, seen),
        Value::Object(_) => v
            .get(mode.as_str())
            .and_then(|x| resolve_value(x, defs, mode, seen)),
        Value::Number(n) => n
            .as_u64()
            .and_then(|i| u8::try_from(i).ok())
            .map(Color::Indexed),
        _ => None,
    }
}

fn resolve_string(
    s: &str,
    defs: Option<&Value>,
    mode: Mode,
    seen: &mut HashSet<String>,
) -> Option<Color> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("transparent") || s.eq_ignore_ascii_case("none") {
        return None;
    }
    if let Some(hex) = parse_hex(s) {
        return Some(hex);
    }
    // defs 引用
    if seen.contains(s) {
        tracing::warn!("theme: cyclic definition at {s}");
        return None;
    }
    let referenced = defs.and_then(|d| d.get(s))?;
    seen.insert(s.to_string());
    resolve_value(referenced, defs, mode, seen)
}

/// 解析 #rgb / #rrggbb / #rrggbbaa。
fn parse_hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    let (r, g, b) = match h.len() {
        3 => {
            let mut it = h.chars();
            let r = it.next()?;
            let g = it.next()?;
            let b = it.next()?;
            (
                u8::from_str_radix(&format!("{r}{r}"), 16).ok()?,
                u8::from_str_radix(&format!("{g}{g}"), 16).ok()?,
                u8::from_str_radix(&format!("{b}{b}"), 16).ok()?,
            )
        }
        6 | 8 => {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            (r, g, b)
        }
        _ => return None,
    };
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_variants() {
        assert_eq!(parse_hex("#fff"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(parse_hex("#1a2b3c"), Some(Color::Rgb(0x1a, 0x2b, 0x3c)));
        assert_eq!(parse_hex("#1a2b3cff"), Some(Color::Rgb(0x1a, 0x2b, 0x3c)));
        assert!(parse_hex("nothex").is_none());
    }

    #[test]
    fn defs_references_resolve() {
        let json = r##"{
            "defs": {"c1": "#112233", "c2": "c1"},
            "theme": {"primary": "c2", "warning": "c1"}
        }"##;
        let t = parse(json, Mode::Dark).unwrap();
        assert_eq!(t.primary, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(t.warning, Color::Rgb(0x11, 0x22, 0x33));
    }

    #[test]
    fn dark_light_dual_values() {
        let json = r##"{"theme":{"primary":{"dark":"#111111","light":"#eeeeee"}}}"##;
        assert_eq!(
            parse(json, Mode::Dark).unwrap().primary,
            Color::Rgb(0x11, 0x11, 0x11)
        );
        assert_eq!(
            parse(json, Mode::Light).unwrap().primary,
            Color::Rgb(0xee, 0xee, 0xee)
        );
    }

    #[test]
    fn transparent_and_unknown_fall_back_to_default() {
        let json = r##"{"theme":{"primary":"transparent","nonsenseToken":"#000000"}}"##;
        let t = parse(json, Mode::Dark).unwrap();
        assert_eq!(t.primary, Theme::tokyonight().primary);
    }

    #[test]
    fn cyclic_defs_do_not_panic() {
        let json = r#"{
            "defs": {"a": "b", "b": "a"},
            "theme": {"primary": "a"}
        }"#;
        let t = parse(json, Mode::Dark).unwrap();
        assert_eq!(t.primary, Theme::tokyonight().primary);
    }

    #[test]
    fn all_bundled_themes_parse_dark_and_light() {
        let dark = ThemeSet::bundled(Mode::Dark);
        let light = ThemeSet::bundled(Mode::Light);
        assert_eq!(dark.len(), BUNDLED.len());
        assert_eq!(light.len(), BUNDLED.len());
        assert!(dark.names().any(|n| n == "tokyonight"));
    }
}
