//! 编辑结果的可读呈现：把「旧内容 → 新内容」渲染为 unified diff 文本。
//!
//! 产出嵌在 ToolResult.content 里，TUI（widgets/diff.rs）按行首标记着色。

use similar::{ChangeTag, TextDiff};

/// 生成统一 diff。`old_label`/`new_label` 出现在 `---`/`+++` 行。
///
/// 只在有实际变化时返回 `Some`；无变化返回 `None`（调用方据此说明"内容未变"）。
pub fn unified_diff(old: &str, new: &str, old_label: &str, new_label: &str) -> Option<String> {
    let diff = TextDiff::from_lines(old, new);
    if diff.ratio() == 1.0 {
        return None;
    }
    let mut out = String::new();
    out.push_str(&format!("--- {old_label}\n+++ {new_label}\n"));
    for group in diff.grouped_ops(3) {
        let (first, last) = (group.first()?, group.last()?);
        let old_start = first.old_range().start + 1;
        let old_end = last.old_range().end;
        let new_start = first.new_range().start + 1;
        let new_end = last.new_range().end;
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start,
            old_end - first.old_range().start,
            new_start,
            new_end - first.new_range().start,
        ));
        for op in &group {
            for change in diff.iter_changes(op) {
                let sign = match change.tag() {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                out.push(sign);
                out.push_str(change.value().trim_end_matches('\n'));
                out.push('\n');
            }
        }
    }
    Some(out)
}

/// 统计新增/删除行数（UI 摘要用）。
pub fn diff_stats(old: &str, new: &str) -> (usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_returns_none() {
        assert!(unified_diff("a\nb\n", "a\nb\n", "old", "new").is_none());
    }

    #[test]
    fn detects_change_with_markers() {
        let d = unified_diff("a\nb\nc\n", "a\nB\nc\n", "old", "new").unwrap();
        assert!(d.contains("-b"));
        assert!(d.contains("+B"));
        assert!(d.contains("--- old"));
        assert!(d.contains("+++ new"));
    }

    #[test]
    fn stats_count_added_and_removed() {
        let (a, r) = diff_stats("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(a, 2);
        assert_eq!(r, 1);
    }
}
