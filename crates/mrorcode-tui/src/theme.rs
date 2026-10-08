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
        Self::opencode()
    }
}

impl Theme {
    /// 内置暗色兜底主题（严格参照 opencode TUI generateSystem，纯黑终端背景）。
    ///
    /// 色值来源：`packages/opencode/src/cli/cmd/run/theme.ts`
    /// - `generateSystem()` 第 382–466 行：从终端调色板派生主题
    /// - `generateGrayScale()` 第 328–367 行：纯黑背景 lum<10 时 grays[n]=floor(n/12*0.4*255)
    /// - `generateMutedTextColor()` 第 369–380 行：纯黑背景时 gray=180
    /// - `RUN_THEME_FALLBACK` 第 605–654 行：ANSI 颜色 fallback seed
    pub fn opencode() -> Self {
        // 纯黑背景下的灰度阶（generateGrayScale，lum=0 < 10）
        // grays[n] = floor(n/12 * 0.4 * 255) = floor(n * 8.5)
        let gray2 = Color::Rgb(0x11, 0x11, 0x11); // grays[2] = 17
        let gray3 = Color::Rgb(0x19, 0x19, 0x19); // grays[3] = 25
        let gray6 = Color::Rgb(0x33, 0x33, 0x33); // grays[6] = 51
        let gray7 = Color::Rgb(0x3b, 0x3b, 0x3b); // grays[7] = 59
        let gray8 = Color::Rgb(0x44, 0x44, 0x44); // grays[8] = 68

        // ANSI 颜色（fallback seed，RUN_THEME_FALLBACK）
        let ansi_cyan = Color::Rgb(0x38, 0xbd, 0xf8); // index 6 → primary/info
        let ansi_magenta = Color::Rgb(0xc0, 0x84, 0xfc); // index 5 → secondary/syntax-keyword
        let ansi_blue = Color::Rgb(0x3b, 0x82, 0xf6); // index 4 → markdown-link/syntax-function
        let ansi_red = Color::Rgb(0xef, 0x44, 0x44); // index 1 → error
        let ansi_green = Color::Rgb(0x22, 0xc5, 0x5e); // index 2 → success
        let ansi_yellow = Color::Rgb(0xf5, 0x9e, 0x0b); // index 3 → warning
        let ansi_green_bright = Color::Rgb(0x4a, 0xde, 0x80); // index 10
        let ansi_red_bright = Color::Rgb(0xf8, 0x71, 0x71); // index 9

        let text = Color::Rgb(0xf8, 0xfa, 0xfc); // seed.text → default foreground
        let text_muted = Color::Rgb(0xb4, 0xb4, 0xb4); // generateMutedTextColor, gray=180

        // diff 背景：tint(bg, color, 0.22)，纯黑背景
        let diff_added_bg = Color::Rgb(0x07, 0x2b, 0x15);
        let diff_removed_bg = Color::Rgb(0x35, 0x0f, 0x0f);
        let diff_added_line_bg = Color::Rgb(0x15, 0x39, 0x22); // tint(grays[2], green, 0.22)
        let diff_removed_line_bg = Color::Rgb(0x42, 0x1c, 0x1c); // tint(grays[2], red, 0.22)

        Self {
            primary: ansi_cyan,
            secondary: ansi_magenta,
            accent: ansi_cyan, // generateSystem: accent = primary
            error: ansi_red,
            warning: ansi_yellow,
            success: ansi_green,
            info: ansi_cyan,

            text,
            text_muted,
            selected_text: Color::Rgb(0x00, 0x00, 0x00), // selectedListItemText = bg

            background: Color::Rgb(0x00, 0x00, 0x00), // 终端原生背景（纯黑）
            background_panel: gray2,    // grays[2]
            background_element: gray3,  // grays[3]
            background_menu: gray3,     // grays[3]

            border: gray7,              // grays[7]
            border_active: gray8,       // grays[8]
            border_subtle: gray6,       // grays[6]

            diff_added: ansi_green,
            diff_removed: ansi_red,
            diff_context: gray7,        // grays[7]
            diff_hunk_header: gray7,    // grays[7]
            diff_highlight_added: ansi_green_bright,
            diff_highlight_removed: ansi_red_bright,
            diff_added_bg,
            diff_removed_bg,
            diff_context_bg: gray2,     // grays[2]
            diff_line_number: text_muted,
            diff_added_line_number_bg: diff_added_line_bg,
            diff_removed_line_number_bg: diff_removed_line_bg,

            // Markdown（generateSystem 第 441–454 行）
            md_text: text,
            md_heading: text,
            md_link: ansi_blue,
            md_link_text: ansi_cyan,
            md_code: ansi_green,
            md_blockquote: ansi_yellow,
            md_emph: ansi_yellow,
            md_strong: text,
            md_horizontal_rule: gray7,
            md_list_item: ansi_blue,
            md_list_enumeration: ansi_cyan,
            md_image: ansi_blue,
            md_image_text: ansi_cyan,
            md_code_bg: gray2,          // 代码块背景 = panel
            md_code_block: text,        // markdownCodeBlock = fg

            // 语法高亮（generateSystem 第 455–463 行）
            syntax_comment: text_muted,
            syntax_keyword: ansi_magenta,
            syntax_function: ansi_blue,
            syntax_variable: text,
            syntax_string: ansi_green,
            syntax_number: ansi_yellow,
            syntax_type: ansi_cyan,
            syntax_operator: ansi_cyan,
            syntax_punctuation: text,
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
        assert_eq!(dark.len(), BUNDLED.len());
        assert_eq!(light.len(), BUNDLED.len());
        assert!(dark.names().any(|n| n == "tokyonight"));
    }
}
