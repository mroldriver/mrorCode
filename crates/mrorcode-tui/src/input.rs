//! 输入层增强：`@` 文件补全与 `/` 斜杠命令补全。
//!
//! 纯 UI 态：补全候选由 `build_items` 计算，接受后仅改写输入框文本 +
//! 用 tui-textarea 的 atomic range 让整段作为不可分割的虚拟片段（退格整体删除）。
//! 提交前由 `submit_prompt` 直接读原始文本，无需展开（虚拟片段即真实文本）。

use std::path::Path;

use crate::fuzzy;
use crate::theme::Theme;

/// 补全类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompletionKind {
    File,
    Command,
}

/// 单个候选。
#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    /// 接受后写入输入框的文本（仅该项替换 token 部分）。
    pub insert: String,
    pub hint: String,
}

/// 补全浮层状态。
pub struct Completion {
    pub kind: CompletionKind,
    /// 触发符所在行。
    pub row: usize,
    /// 触发符所在列（`@`/`/` 本身）。
    pub start_col: usize,
    /// 光标列（token 结束位置）。
    pub end_col: usize,
    pub query: String,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

impl Completion {
    pub fn visible(&self) -> bool {
        !self.items.is_empty()
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        let mut idx = self.selected as isize + delta;
        if idx < 0 {
            idx = len - 1;
        } else if idx >= len {
            idx = 0;
        }
        self.selected = idx as usize;
    }

    pub fn current(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }
}

/// 从光标位置反推是否处于补全触发态（`@xxx` 或行首 `/xxx`）。
///
/// 返回 (kind, start_col, query)。`@path` 里 path 允许含 `/`；
/// 斜杠命令仅在整行以 `/` 开头、且光标后无空格时触发。
pub fn detect(line: &str, cursor_col: usize) -> Option<(CompletionKind, usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    if cursor_col > chars.len() {
        return None;
    }
    let before = &chars[..cursor_col];
    // 从光标向左找最近的触发符，遇到空格即停止。
    let mut idx = None;
    for i in (0..before.len()).rev() {
        let c = before[i];
        if c == ' ' || c == '\t' {
            break;
        }
        if c == '@' || c == '/' {
            idx = Some(i);
            break;
        }
    }
    let start = idx?;
    let trigger = before[start];
    if trigger == '/' && start != 0 {
        return None;
    }
    let query: String = before[start + 1..].iter().collect();
    let kind = if trigger == '@' {
        CompletionKind::File
    } else {
        CompletionKind::Command
    };
    Some((kind, start, query))
}

/// 构造候选（file：遍历工作目录；command：命令注册表）。
pub fn build_items(kind: CompletionKind, query: &str, working_dir: &Path) -> Vec<CompletionItem> {
    let raw = match kind {
        CompletionKind::File => file_candidates(working_dir, query),
        CompletionKind::Command => command_candidates(),
    };
    // fuzzy 过滤 + 排序
    let mut scored: Vec<(u32, CompletionItem)> = raw
        .into_iter()
        .filter_map(|item| {
            let score = fuzzy::score(query, &item.label)
                .map(|(s, _)| s)
                .unwrap_or(0);
            (query.is_empty() || score > 0).then_some((score, item))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored.into_iter().map(|(_, i)| i).take(50).collect()
}

fn command_candidates() -> Vec<CompletionItem> {
    crate::commands::registry()
        .iter()
        .map(|c| CompletionItem {
            label: c.name.to_string(),
            insert: format!("/{}", c.name.replace(' ', "-")),
            hint: c.desc.to_string(),
        })
        .collect()
}

/// 简单工作目录遍历（跳过隐藏目录与常见重目录），上限 500 条。
fn file_candidates(root: &Path, query: &str) -> Vec<CompletionItem> {
    const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".venv", "dist", ".tools"];
    let mut out = Vec::new();
    let base = if query.contains('/') {
        // query 带路径分支时，从 root 直接拼接搜索
        root.to_path_buf()
    } else {
        root.to_path_buf()
    };
    let mut stack = vec![base.clone()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && name != "." {
                continue;
            }
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path.clone());
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let label = if is_dir { format!("{rel}/") } else { rel };
            out.push(CompletionItem {
                insert: format!("@{label}"),
                label,
                hint: if is_dir { "dir".into() } else { String::new() },
            });
            if out.len() >= 500 {
                return out;
            }
        }
    }
    out
}

/// 渲染补全浮层：输入区上方的一块列表（最多 8 行）。
pub fn render(
    comp: &Completion,
    prompt_area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    theme: &Theme,
) {
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Widget;

    const MAX_ROWS: usize = 8;
    let shown = comp.items.len().min(MAX_ROWS);
    if shown == 0 {
        return;
    }
    let height = shown as u16;
    let width = 60.min(prompt_area.width.saturating_sub(4)).max(20);
    let y = prompt_area.y.saturating_sub(height);
    let x = prompt_area.x + 2;
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    ratatui::widgets::Block::default()
        .style(Style::default().bg(theme.background_panel))
        .render(area, buf);

    // 让选中项可见：以 selected 为中心的窗口
    let start = if comp.selected >= MAX_ROWS {
        comp.selected + 1 - MAX_ROWS
    } else {
        0
    };
    for (row, item) in comp.items.iter().skip(start).take(MAX_ROWS).enumerate() {
        let selected = start + row == comp.selected;
        let marker = if selected { "❯ " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let mut spans = vec![
            Span::styled(marker, style),
            Span::styled(item.label.clone(), style),
        ];
        if !item.hint.is_empty() {
            spans.push(Span::styled(
                format!("  {}", item.hint),
                Style::default().fg(theme.text_muted),
            ));
        }
        let line = Line::from(spans);
        buf.set_line(area.x, area.y + row as u16, &line, area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detect_at_trigger() {
        let (kind, start, q) = detect("hello @sr", 9).unwrap();
        assert_eq!(kind, CompletionKind::File);
        assert_eq!(start, 6);
        assert_eq!(q, "sr");
    }

    #[test]
    fn detect_slash_only_at_line_start() {
        assert_eq!(detect("/he", 3).unwrap().0, CompletionKind::Command);
        assert!(detect("abc /he", 7).is_none());
    }

    #[test]
    fn detect_stops_at_space() {
        assert!(detect("@a b@c", 6).is_some()); // 最近的 @ 在 b 后
        assert!(detect("a @b c", 6).is_none()); // 光标在 c 前，最近 token 无触发符
    }

    #[test]
    fn command_candidates_include_palette() {
        let items = build_items(CompletionKind::Command, "pal", Path::new("."));
        assert!(items.iter().any(|i| i.label.contains("command palette")));
    }

    #[test]
    fn file_candidates_find_file() {
        let dir = std::env::temp_dir().join(format!("mrorcode-input-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        let items = build_items(CompletionKind::File, "main", &dir);
        assert!(items.iter().any(|i| i.label.contains("main.rs")));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn fake_completion(n: usize) -> Completion {
        Completion {
            kind: CompletionKind::Command,
            row: 0,
            start_col: 0,
            end_col: 1,
            query: String::new(),
            items: (0..n)
                .map(|i| CompletionItem {
                    label: format!("item-{i}"),
                    insert: format!("/item-{i}"),
                    hint: String::new(),
                })
                .collect(),
            selected: 0,
        }
    }

    #[test]
    fn move_selection_advances_and_wraps() {
        let mut c = fake_completion(3);
        c.move_selection(1);
        assert_eq!(c.selected, 1);
        c.move_selection(1);
        assert_eq!(c.selected, 2);
        c.move_selection(1); // 越过末尾回到开头
        assert_eq!(c.selected, 0);
        c.move_selection(-1); // 越过开头回到末尾
        assert_eq!(c.selected, 2);
    }

    #[test]
    fn move_selection_empty_is_noop() {
        let mut c = fake_completion(0);
        c.move_selection(1);
        assert_eq!(c.selected, 0);
        assert!(c.current().is_none());
    }

    #[test]
    fn empty_completion_not_visible() {
        assert!(!fake_completion(0).visible());
        assert!(fake_completion(1).visible());
    }
}
