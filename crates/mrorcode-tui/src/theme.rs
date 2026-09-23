//! 主题：M1 内置一套（tokyonight 取向）。
//!
//! 字段命名刻意对齐 opencode 主题 JSON token，M2 可直接反序列化用户主题文件。

use ratatui::style::Color;

/// 字段与 opencode 主题 JSON token 对齐；M1 部分色值暂未使用（M2 启用）。
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

    pub md_heading: Color,
    pub md_link: Color,
    pub md_code: Color,
    pub md_blockquote: Color,
    pub md_code_bg: Color,

    pub syntax_comment: Color,
    pub syntax_keyword: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_function: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::tokyonight()
    }
}

impl Theme {
    /// 内置暗色主题（参考 tokyonight night 调色板）。
    pub fn tokyonight() -> Self {
        Self {
            primary: Color::Rgb(0x7a, 0xa2, 0xf7),   // blue
            secondary: Color::Rgb(0xbb, 0x9a, 0xf7), // purple
            accent: Color::Rgb(0x7d, 0xcf, 0xff),    // cyan
            error: Color::Rgb(0xf7, 0x76, 0x8e),     // red
            warning: Color::Rgb(0xe0, 0xaf, 0x68),   // yellow
            success: Color::Rgb(0x9e, 0xce, 0x6a),   // green
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

            md_heading: Color::Rgb(0x7a, 0xa2, 0xf7),
            md_link: Color::Rgb(0x7d, 0xcf, 0xff),
            md_code: Color::Rgb(0xe0, 0xaf, 0x68),
            md_blockquote: Color::Rgb(0x56, 0x5f, 0x89),
            md_code_bg: Color::Rgb(0x24, 0x28, 0x3b),

            syntax_comment: Color::Rgb(0x56, 0x5f, 0x89),
            syntax_keyword: Color::Rgb(0xbb, 0x9a, 0xf7),
            syntax_string: Color::Rgb(0x9e, 0xce, 0x6a),
            syntax_number: Color::Rgb(0xff, 0x9e, 0x64),
            syntax_function: Color::Rgb(0x7d, 0xcf, 0xff),
        }
    }
}
