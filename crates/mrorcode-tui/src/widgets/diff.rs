//! unified diff 解析与着色（M4-3 diff viewer 数据源）。
//!
//! write_file/edit_file 的结果文本形如：
//! ```text
//! Edited src/main.rs (1 replacement(s))
//! --- src/main.rs
//! +++ src/main.rs
//! @@ -1,3 +1,3 @@
//! -old
//! +new
//!  context
//! ```
//! 解析为 [`DiffDoc`]（文件 → hunk → 带行号的行），供内联着色与独立 viewer 共用。

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// 输出是否包含统一 diff（用于决定是否走 diff 着色/查看路径）。
pub fn looks_like_diff(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("@@ "))
}

/// diff 行的类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Add,
    Remove,
}

/// 一个 hunk 内的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// 去掉行首标记后的文本。
    pub text: String,
    /// 旧文件行号（Add 行为 None）。
    pub old_no: Option<u32>,
    /// 新文件行号（Remove 行为 None）。
    pub new_no: Option<u32>,
}

/// 一个 hunk（`@@ ... @@` 起）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// 单个文件的 diff。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    pub old_label: String,
    pub new_label: String,
    pub hunks: Vec<DiffHunk>,
}

/// 解析后的完整 diff。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffDoc {
    pub files: Vec<DiffFile>,
}

impl DiffDoc {
    /// 全部 hunk 行数（viewer 滚动范围用）。
    pub fn line_count(&self) -> usize {
        self.files
            .iter()
            .map(|f| f.hunks.iter().map(|h| h.lines.len() + 1).sum::<usize>())
            .sum()
    }

    /// 增/删行统计。
    pub fn stats(&self) -> (usize, usize) {
        let mut add = 0;
        let mut del = 0;
        for f in &self.files {
            for h in &f.hunks {
                for l in &h.lines {
                    match l.kind {
                        DiffKind::Add => add += 1,
                        DiffKind::Remove => del += 1,
                        DiffKind::Context => {}
                    }
                }
            }
        }
        (add, del)
    }
}

/// 解析工具输出为 [`DiffDoc`]；无可识别 diff 时返回空文档。
pub fn parse(text: &str) -> DiffDoc {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut current: Option<DiffFile> = None;
    let mut hunk: Option<DiffHunk> = None;
    let mut old_no = 0u32;
    let mut new_no = 0u32;

    for raw in text.lines() {
        if let Some(label) = raw.strip_prefix("--- ") {
            // 新文件头：冲掉上一个
            flush_hunk(&mut hunk, &mut current);
            flush_file(&mut current, &mut files);
            current = Some(DiffFile {
                old_label: label.to_string(),
                new_label: String::new(),
                hunks: Vec::new(),
            });
            continue;
        }
        if let Some(label) = raw.strip_prefix("+++ ") {
            if let Some(f) = current.as_mut() {
                f.new_label = label.to_string();
            }
            continue;
        }
        if raw.starts_with("@@ ") {
            flush_hunk(&mut hunk, &mut current);
            if let Some((o, n)) = parse_hunk_header(raw) {
                old_no = o;
                new_no = n;
            }
            hunk = Some(DiffHunk {
                header: raw.to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(h) = hunk.as_mut() else { continue };
        // `\ No newline at end of file` 标记：忽略
        if raw.starts_with('\\') {
            continue;
        }
        let (kind, text) = if let Some(rest) = raw.strip_prefix('+') {
            (DiffKind::Add, rest)
        } else if let Some(rest) = raw.strip_prefix('-') {
            (DiffKind::Remove, rest)
        } else if let Some(rest) = raw.strip_prefix(' ') {
            (DiffKind::Context, rest)
        } else if raw.is_empty() {
            (DiffKind::Context, "")
        } else {
            // hunk 内出现非 diff 前缀（如工具摘要混入）：按 context 处理并停止
            continue;
        };
        let (old, new) = match kind {
            DiffKind::Add => {
                let n = Some(new_no);
                new_no += 1;
                (None, n)
            }
            DiffKind::Remove => {
                let o = Some(old_no);
                old_no += 1;
                (o, None)
            }
            DiffKind::Context => {
                let o = Some(old_no);
                let n = Some(new_no);
                old_no += 1;
                new_no += 1;
                (o, n)
            }
        };
        h.lines.push(DiffLine {
            kind,
            text: text.to_string(),
            old_no: old,
            new_no: new,
        });
    }
    flush_hunk(&mut hunk, &mut current);
    flush_file(&mut current, &mut files);
    DiffDoc { files }
}

fn flush_hunk(hunk: &mut Option<DiffHunk>, current: &mut Option<DiffFile>) {
    if let (Some(h), Some(f)) = (hunk.take(), current.as_mut()) {
        f.hunks.push(h);
    }
}

fn flush_file(current: &mut Option<DiffFile>, files: &mut Vec<DiffFile>) {
    if let Some(f) = current.take() {
        // 只有含 hunk 的才算一个文件
        if !f.hunks.is_empty() {
            files.push(f);
        }
    }
}

/// 解析 `@@ -old,count +new,count @@` 的起始行号。
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let body = line.strip_prefix("@@ ")?;
    let end = body.find(" @@")?;
    let spec = &body[..end];
    let mut parts = spec.split(' ');
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let old_start = old.split(',').next()?.parse().ok()?;
    let new_start = new.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

/// 单行 diff 的样式；非 diff 行返回 `None`（由调用方走普通输出色）。
pub fn line_style(line: &str, theme: &Theme) -> Option<(Color, Color)> {
    if line.starts_with("@@ ") {
        Some((theme.diff_hunk_header, theme.background))
    } else if line.starts_with("+++ ") || line.starts_with("--- ") {
        Some((theme.text_muted, theme.background))
    } else if line.starts_with('+') {
        Some((theme.diff_added, theme.diff_added_bg))
    } else if line.starts_with('-') {
        Some((theme.diff_removed, theme.diff_removed_bg))
    } else if line.starts_with(' ') {
        Some((theme.diff_context, theme.diff_context_bg))
    } else {
        None
    }
}

/// 按类别取前景/背景色（viewer 用）。
pub fn kind_style(kind: DiffKind, theme: &Theme) -> (Color, Color) {
    match kind {
        DiffKind::Add => (theme.diff_added, theme.diff_added_bg),
        DiffKind::Remove => (theme.diff_removed, theme.diff_removed_bg),
        DiffKind::Context => (theme.diff_context, theme.diff_context_bg),
    }
}

/// 把一行 diff（或普通输出行）渲染为带缩进的 Span 列表。
pub fn render_line(text: &str, indent: &str, theme: &Theme) -> Line<'static> {
    match line_style(text, theme) {
        Some((fg, bg)) => Line::from(Span::styled(
            format!("{indent}{text}"),
            Style::default().fg(fg).bg(bg),
        )),
        None => Line::from(Span::raw(format!("{indent}{text}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"Edited src/main.rs (1 replacement(s))
--- src/main.rs
+++ src/main.rs
@@ -1,3 +1,3 @@
 fn main() {
-    let x = 1;
+    let x = 2;
 }
"#;

    #[test]
    fn detects_diff_by_hunk_header() {
        assert!(looks_like_diff("+++ a\n--- a\n@@ -1 +1 @@\n-x\n+y"));
        assert!(!looks_like_diff("just output\n"));
    }

    #[test]
    fn parses_single_file_with_numbers() {
        let doc = parse(SAMPLE);
        assert_eq!(doc.files.len(), 1);
        let f = &doc.files[0];
        assert_eq!(f.old_label, "src/main.rs");
        assert_eq!(f.new_label, "src/main.rs");
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[0].kind, DiffKind::Context);
        assert_eq!((h.lines[0].old_no, h.lines[0].new_no), (Some(1), Some(1)));
        assert_eq!(h.lines[1].kind, DiffKind::Remove);
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[1].new_no, None);
        assert_eq!(h.lines[2].kind, DiffKind::Add);
        assert_eq!(h.lines[2].old_no, None);
        assert_eq!(h.lines[2].new_no, Some(2));
        assert_eq!(doc.stats(), (1, 1));
    }

    #[test]
    fn parses_multiple_files() {
        let text = "--- a.txt\n+++ a.txt\n@@ -1 +1 @@\n-x\n+y\n\
--- b.txt\n+++ b.txt\n@@ -5,1 +5,1 @@\n-p\n+q\n";
        let doc = parse(text);
        assert_eq!(doc.files.len(), 2);
        assert_eq!(doc.files[1].old_label, "b.txt");
        assert_eq!(doc.files[1].hunks[0].lines[0].old_no, Some(5));
    }

    #[test]
    fn ignores_no_newline_marker() {
        let text = "--- a\n+++ a\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n";
        let doc = parse(text);
        assert_eq!(doc.files[0].hunks[0].lines.len(), 2);
    }

    #[test]
    fn empty_on_non_diff() {
        assert_eq!(parse("hello\nworld").files.len(), 0);
        assert_eq!(parse("hello").line_count(), 0);
    }

    #[test]
    fn assigns_expected_colors() {
        let theme = Theme::tokyonight();
        assert_eq!(line_style("+added", &theme).unwrap().0, theme.diff_added);
        assert_eq!(
            line_style("-removed", &theme).unwrap().0,
            theme.diff_removed
        );
        assert_eq!(
            line_style("@@ -1 +1 @@", &theme).unwrap().0,
            theme.diff_hunk_header
        );
        assert!(line_style("plain", &theme).is_none());
    }

    #[test]
    fn parser_classification_matches_line_style() {
        // 回归基线：解析器的类别着色与旧 line_style 对同一行一致
        let theme = Theme::tokyonight();
        for (raw, kind) in [
            ("+new", DiffKind::Add),
            ("-old", DiffKind::Remove),
            (" ctx", DiffKind::Context),
        ] {
            let (fg, bg) = kind_style(kind, &theme);
            assert_eq!(line_style(raw, &theme), Some((fg, bg)));
        }
    }
}
