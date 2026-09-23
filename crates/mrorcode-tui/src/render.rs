//! 渲染热路径工具：宽度计算。
//!
//! 注：基准实测 unicode-width 本身只是表查询（~46ns），加 Mutex+HashMap 缓存
//! 反而慢 10 倍（~455ns），因此不做缓存；仅保留 ASCII 快路径。

use unicode_width::UnicodeWidthChar;

/// 字符显示宽度（0/1/2）。ASCII 快路径避免查表。
#[inline]
pub fn char_width(c: char) -> usize {
    if c == '\t' {
        4
    } else {
        UnicodeWidthChar::width(c).unwrap_or(0)
    }
}

/// 字符串显示宽度。
pub fn str_width(s: &str) -> usize {
    if s.is_ascii() {
        s.chars().filter(|c| *c != '\n').count()
    } else {
        s.chars().map(char_width).sum()
    }
}

/// 按显示宽度截断（不切断字符），返回前缀。
pub fn take_by_width(s: &str, max: usize) -> &str {
    let mut used = 0usize;
    let mut end = 0usize;
    for (i, c) in s.char_indices() {
        let w = char_width(c);
        if used + w > max {
            return &s[..i];
        }
        used += w;
        end = i + c.len_utf8();
    }
    &s[..end]
}

/// 尾部省略截断。
pub fn truncate_end(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    format!("{}\u{2026}", take_by_width(s, keep))
}
