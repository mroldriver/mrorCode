//! 主题系统：直接加载 opencode 格式主题 JSON（MIT vendored，见
//! licenses/OPENCODE-LICENSE）。
//!
//! 支持：`#hex` / defs 引用（含多级、循环检测）/ `{dark,light}` 双值 /
//! `transparent`、`none`（回退内置默认）/ ANSI 数字。
//! 缺 token 用内置 Ant Design 暗黑补齐，解析失败不阻塞启动。

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
    /// 亮色模式：当前未在运行时暴露（内置主题 JSON 仍解析 light 分支以便将来恢复）。
    #[allow(dead_code)]
    Light,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::opencode()
    }
}

impl Theme {
    /// 内置暗色兜底主题：严格对齐 vendored `themes/opencode.json` 的 dark 分支，
    /// 源文件 `packages/tui/src/theme/assets/opencode.json`（MIT vendored）。
    /// 用于：JSON 解析失败兜底、JSON 未定义的 token（如 backgroundMenu）。
    pub fn opencode() -> Self {
        // darkStep 阶（defs）
        let s1 = Color::Rgb(0x0a, 0x0a, 0x0a); // darkStep1 → background
        let s2 = Color::Rgb(0x14, 0x14, 0x14); // darkStep2 → backgroundPanel / diffContextBg
        let s3 = Color::Rgb(0x1e, 0x1e, 0x1e); // darkStep3 → backgroundElement / backgroundMenu
        let s6 = Color::Rgb(0x3c, 0x3c, 0x3c); // darkStep6 → borderSubtle
        let s7 = Color::Rgb(0x48, 0x48, 0x48); // darkStep7 → border
        let s8 = Color::Rgb(0x60, 0x60, 0x60); // darkStep8 → borderActive
        let s9 = Color::Rgb(0xfa, 0xb2, 0x83); // darkStep9 → primary / markdown link / syntax function
        let s11 = Color::Rgb(0x80, 0x80, 0x80); // darkStep11 → textMuted / comment
        let s12 = Color::Rgb(0xee, 0xee, 0xee); // darkStep12 → text / code block

        // 语义色（defs）
        let secondary = Color::Rgb(0x5c, 0x9c, 0xf5); // darkSecondary
        let accent = Color::Rgb(0x9d, 0x7c, 0xd8); // darkAccent
        let red = Color::Rgb(0xe0, 0x6c, 0x75); // darkRed → error / syntax-variable
        let orange = Color::Rgb(0xf5, 0xa7, 0x42); // darkOrange → warning / markdown-strong / number
        let green = Color::Rgb(0x7f, 0xd8, 0x8f); // darkGreen → success / markdown-code / string
        let cyan = Color::Rgb(0x56, 0xb6, 0xc2); // darkCyan → info / markdownLinkText / operator
        let yellow = Color::Rgb(0xe5, 0xc0, 0x7b); // darkYellow → markdown blockquote/emph / syntax-type

        // diff 相关（源主题直接给出的 hex）
        let diff_added = Color::Rgb(0x4f, 0xd6, 0xbe);
        let diff_removed = Color::Rgb(0xc5, 0x3b, 0x53);
        let diff_context = Color::Rgb(0x82, 0x8b, 0xb8);
        let diff_highlight_added = Color::Rgb(0xb8, 0xdb, 0x87);
        let diff_highlight_removed = Color::Rgb(0xe2, 0x6a, 0x75);
        let diff_added_bg = Color::Rgb(0x20, 0x30, 0x3b);
        let diff_removed_bg = Color::Rgb(0x37, 0x22, 0x2c);
        let diff_line_number = Color::Rgb(0x8f, 0x8f, 0x8f);
        let diff_added_line_number_bg = Color::Rgb(0x1b, 0x2b, 0x34);
        let diff_removed_line_number_bg = Color::Rgb(0x2d, 0x1f, 0x26);

        Self {
            // 品牌主色：与 vendored themes/opencode.json 的 primary (#409EFF) 保持一致；
            // 这里不再跟随上游 darkStep9（#fab283）——上游的 s9 仍保留给 markdown link 等。
            primary: Color::Rgb(0x40, 0x9e, 0xff),
            secondary,
            accent,
            error: red,
            warning: orange,
            success: green,
            info: cyan,

            text: s12,
            text_muted: s11,
            selected_text: s1, // selectedListItemText = bg

            background: s1,
            background_panel: s2,
            background_element: s3,
            background_menu: s3, // 源主题未定义 backgroundMenu，用 backgroundElement 兜底

            border: s7,
            border_active: s8,
            border_subtle: s6,

            diff_added,
            diff_removed,
            diff_context,
            diff_hunk_header: diff_context,
            diff_highlight_added,
            diff_highlight_removed,
            diff_added_bg,
            diff_removed_bg,
            diff_context_bg: s2,
            diff_line_number,
            diff_added_line_number_bg,
            diff_removed_line_number_bg,

            // Markdown
            md_text: s12,
            md_heading: accent,
            md_link: s9,
            md_link_text: cyan,
            md_code: green,
            md_blockquote: yellow,
            md_emph: yellow,
            md_strong: orange,
            md_horizontal_rule: s11,
            md_list_item: s9,
            md_list_enumeration: cyan,
            md_image: s9,
            md_image_text: cyan,
            md_code_bg: s2, // 代码块背景 = panel
            md_code_block: s12,

            // 语法高亮
            syntax_comment: s11,
            syntax_keyword: accent,
            syntax_function: s9,
            syntax_variable: red,
            syntax_string: green,
            syntax_number: orange,
            syntax_type: yellow,
            syntax_operator: cyan,
            syntax_punctuation: s12,
        }
    }

    /// 内置暗色兜底主题（Ant Design v5 暗黑模式调色板）。
    pub fn ant_design() -> Self {
        Self {
            primary: Color::Rgb(0x16, 0x77, 0xff),
            secondary: Color::Rgb(0x69, 0xb1, 0xff),
            accent: Color::Rgb(0x40, 0x96, 0xff),
            error: Color::Rgb(0xff, 0x4d, 0x4f),
            warning: Color::Rgb(0xfa, 0xad, 0x14),
            success: Color::Rgb(0x52, 0xc4, 0x1a),
            info: Color::Rgb(0x16, 0x77, 0xff),

            text: Color::Rgb(0xe0, 0xe0, 0xe0),
            text_muted: Color::Rgb(0xa6, 0xa6, 0xa6),
            selected_text: Color::Rgb(0x14, 0x14, 0x14),

            background: Color::Rgb(0x14, 0x14, 0x14),
            background_panel: Color::Rgb(0x1f, 0x1f, 0x1f),
            background_element: Color::Rgb(0x26, 0x26, 0x26),
            background_menu: Color::Rgb(0x1f, 0x1f, 0x1f),

            border: Color::Rgb(0x42, 0x42, 0x42),
            border_active: Color::Rgb(0x16, 0x77, 0xff),
            border_subtle: Color::Rgb(0x30, 0x30, 0x30),

            diff_added: Color::Rgb(0x52, 0xc4, 0x1a),
            diff_removed: Color::Rgb(0xff, 0x4d, 0x4f),
            diff_context: Color::Rgb(0x73, 0x73, 0x73),
            diff_hunk_header: Color::Rgb(0x69, 0xb1, 0xff),
            diff_highlight_added: Color::Rgb(0x26, 0x4d, 0x1a),
            diff_highlight_removed: Color::Rgb(0x4d, 0x1a, 0x1d),
            diff_added_bg: Color::Rgb(0x1a, 0x2e, 0x14),
            diff_removed_bg: Color::Rgb(0x2e, 0x14, 0x17),
            diff_context_bg: Color::Rgb(0x1f, 0x1f, 0x1f),
            diff_line_number: Color::Rgb(0x73, 0x73, 0x73),
            diff_added_line_number_bg: Color::Rgb(0x14, 0x24, 0x10),
            diff_removed_line_number_bg: Color::Rgb(0x24, 0x10, 0x12),

            md_text: Color::Rgb(0xe0, 0xe0, 0xe0),
            md_heading: Color::Rgb(0x16, 0x77, 0xff),
            md_link: Color::Rgb(0x16, 0x77, 0xff),
            md_link_text: Color::Rgb(0x69, 0xb1, 0xff),
            md_code: Color::Rgb(0xfa, 0xad, 0x14),
            md_blockquote: Color::Rgb(0x73, 0x73, 0x73),
            md_emph: Color::Rgb(0xfa, 0xad, 0x14),
            md_strong: Color::Rgb(0x40, 0x96, 0xff),
            md_horizontal_rule: Color::Rgb(0x30, 0x30, 0x30),
            md_list_item: Color::Rgb(0x16, 0x77, 0xff),
            md_list_enumeration: Color::Rgb(0x13, 0xc2, 0xc2),
            md_image: Color::Rgb(0x16, 0x77, 0xff),
            md_image_text: Color::Rgb(0x69, 0xb1, 0xff),
            md_code_bg: Color::Rgb(0x26, 0x26, 0x26),
            md_code_block: Color::Rgb(0x26, 0x26, 0x26),

            syntax_comment: Color::Rgb(0x73, 0x73, 0x73),
            syntax_keyword: Color::Rgb(0xeb, 0x2f, 0x96),
            syntax_function: Color::Rgb(0x16, 0x77, 0xff),
            syntax_variable: Color::Rgb(0xe0, 0xe0, 0xe0),
            syntax_string: Color::Rgb(0x52, 0xc4, 0x1a),
            syntax_number: Color::Rgb(0xfa, 0xad, 0x14),
            syntax_type: Color::Rgb(0x13, 0xc2, 0xc2),
            syntax_operator: Color::Rgb(0xa6, 0xa6, 0xa6),
            syntax_punctuation: Color::Rgb(0xa6, 0xa6, 0xa6),
        }
    }

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

    /// 主题名列表（主要供测试断言；运行时不再有主题选择器）。
    #[allow(dead_code)]
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.themes.iter().map(|(n, _)| n.as_str())
    }

    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.themes.iter().find(|(n, _)| n == name).map(|(_, t)| t)
    }

    #[allow(dead_code)]
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
    let mut theme = Theme::opencode();

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
        assert_eq!(t.primary, Theme::opencode().primary);
    }

    #[test]
    fn cyclic_defs_do_not_panic() {
        let json = r#"{
            "defs": {"a": "b", "b": "a"},
            "theme": {"primary": "a"}
        }"#;
        let t = parse(json, Mode::Dark).unwrap();
        assert_eq!(t.primary, Theme::opencode().primary);
    }

    #[test]
    fn all_bundled_themes_parse_dark_and_light() {
        let dark = ThemeSet::bundled(Mode::Dark);
        let light = ThemeSet::bundled(Mode::Light);
        // 只内置 opencode 一个主题
        assert_eq!(dark.len(), 1);
        assert_eq!(light.len(), 1);
        assert!(dark.names().any(|n| n == "opencode"));
    }
}
