//! diff viewer 对话框（M4-3）：完整 unified diff 的独立大面板。
//!
//! 作为 [`crate::widgets::dialog::Dialog`] 栈成员，支持滚动与 unified/split 切换；
//! 复用 `widgets::diff::DiffDoc` 解析结果，行着色走 theme 的 diff_* token。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::render::{str_width, truncate_end};
use crate::theme::Theme;
use crate::widgets::dialog::select::{draw_border_static, fill, OVERLAY};
use crate::widgets::dialog::DialogOutcome;
use crate::widgets::diff::{kind_style, DiffDoc, DiffKind};

/// 视图模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffView {
    Unified,
    Split,
}

#[derive(Debug)]
pub struct DiffState {
    pub title: String,
    doc: DiffDoc,
    view: DiffView,
    scroll: usize,
}

impl DiffState {
    pub fn new(title: impl Into<String>, doc: DiffDoc) -> Self {
        Self {
            title: title.into(),
            doc,
            view: DiffView::Unified,
            scroll: 0,
        }
    }

    /// 从工具输出解析并构造；无 diff 内容时返回 None。
    pub fn from_output(title: impl Into<String>, output: &str) -> Option<Self> {
        let doc = crate::widgets::diff::parse(output);
        if doc.files.is_empty() {
            None
        } else {
            Some(Self::new(title, doc))
        }
    }

    pub fn view(&self) -> DiffView {
        self.view
    }

    pub fn doc(&self) -> &DiffDoc {
        &self.doc
    }

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            DiffView::Unified => DiffView::Split,
            DiffView::Split => DiffView::Unified,
        };
    }

    pub fn on_key(&mut self, key: &KeyEvent) -> DialogOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => DialogOutcome::Closed,
            KeyCode::Char('c') if ctrl => DialogOutcome::Closed,
            KeyCode::Char('t') => {
                self.toggle_view();
                DialogOutcome::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                DialogOutcome::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                DialogOutcome::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(20);
                DialogOutcome::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(20);
                DialogOutcome::None
            }
            KeyCode::Home => {
                self.scroll = 0;
                DialogOutcome::None
            }
            KeyCode::End => {
                self.scroll = usize::MAX;
                DialogOutcome::None
            }
            _ => DialogOutcome::None,
        }
    }
}

/// 一行可渲染内容（预展开，便于窗口化滚动）。
enum Row {
    HunkHeader(String),
    /// (旧行号, 新行号, 类别, 文本)
    Line(Option<u32>, Option<u32>, DiffKind, String),
}

/// 把文档展开为统一视图行序列。
fn unified_rows(doc: &DiffDoc) -> Vec<Row> {
    let mut rows = Vec::new();
    for file in &doc.files {
        rows.push(Row::HunkHeader(format!(
            "  {} \u{2192} {}",
            file.old_label, file.new_label
        )));
        for hunk in &file.hunks {
            rows.push(Row::HunkHeader(hunk.header.clone()));
            for l in &hunk.lines {
                rows.push(Row::Line(l.old_no, l.new_no, l.kind, l.text.clone()));
            }
        }
    }
    rows
}

pub fn render(state: &DiffState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(OVERLAY));
            }
        }
    }

    let w = (area.width * 9 / 10).clamp(48, area.width.saturating_sub(2));
    let h = (area.height * 9 / 10).clamp(12, area.height.saturating_sub(2));
    let px = area.x + (area.width - w) / 2;
    let py = area.y + (area.height - h) / 2;
    let panel = Rect {
        x: px,
        y: py,
        width: w,
        height: h,
    };
    fill(panel, buf, theme.background_menu);
    draw_border_static(panel, buf, theme);

    let inner_w = w as usize - 4;
    let (added, removed) = state.doc.stats();
    let view_label = match state.view {
        DiffView::Unified => "unified",
        DiffView::Split => "split",
    };
    let title = format!(
        "{}  +{} -{}  [{}]",
        truncate_end(&state.title, inner_w.saturating_sub(24)),
        added,
        removed,
        view_label
    );
    set(
        px + 2,
        py + 1,
        &Line::from(Span::styled(
            title,
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        w - 4,
        buf,
    );

    let list_top = py + 2;
    let list_h = h as usize - 4;
    let rows = unified_rows(&state.doc);
    let max_scroll = rows.len().saturating_sub(list_h);
    let scroll = state.scroll.min(max_scroll);

    match state.view {
        DiffView::Unified => {
            render_unified_rows(&rows, scroll, list_h, list_top, px + 2, inner_w, buf, theme)
        }
        DiffView::Split => render_split(
            &state.doc,
            scroll,
            list_h,
            list_top,
            px + 2,
            inner_w,
            buf,
            theme,
        ),
    }

    set(
        px + 2,
        py + h - 2,
        &Line::from(Span::styled(
            "\u{2191}\u{2193} 滚动 \u{00B7} t 切换视图 \u{00B7} q/esc 关闭",
            Style::default().fg(theme.text_muted),
        )),
        w - 4,
        buf,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_unified_rows(
    rows: &[Row],
    scroll: usize,
    list_h: usize,
    list_top: u16,
    x: u16,
    inner_w: usize,
    buf: &mut Buffer,
    theme: &Theme,
) {
    // 行号列固定宽度（旧/新各 4 位），与 header 对齐
    for (i, row) in rows.iter().skip(scroll).take(list_h).enumerate() {
        let y = list_top + i as u16;
        match row {
            Row::HunkHeader(text) => {
                let line = Line::from(Span::styled(
                    truncate_end(text, inner_w),
                    Style::default().fg(theme.secondary),
                ));
                set(x, y, &line, inner_w as u16, buf);
            }
            Row::Line(old, new, kind, text) => {
                let (fg, bg) = kind_style(*kind, theme);
                let gut = format!(
                    "{:>4} {:>4} ",
                    old.map(|v| v.to_string()).unwrap_or_default(),
                    new.map(|v| v.to_string()).unwrap_or_default()
                );
                let avail = inner_w.saturating_sub(str_width(&gut) + 1);
                let marker = match kind {
                    DiffKind::Add => "+",
                    DiffKind::Remove => "-",
                    DiffKind::Context => " ",
                };
                let body = truncate_end(text, avail);
                let line = Line::from(vec![
                    Span::styled(
                        gut,
                        Style::default()
                            .fg(theme.diff_line_number)
                            .bg(theme.background),
                    ),
                    Span::styled(format!("{marker}{body}"), Style::default().fg(fg).bg(bg)),
                ]);
                set(x, y, &line, inner_w as u16, buf);
            }
        }
    }
}

/// split 视图：hunk 内按行号把删除/新增配对到左右两栏。
#[allow(clippy::too_many_arguments)]
fn render_split(
    doc: &DiffDoc,
    scroll: usize,
    list_h: usize,
    list_top: u16,
    x: u16,
    inner_w: usize,
    buf: &mut Buffer,
    theme: &Theme,
) {
    // 展开为 (old_side, new_side) 行对
    let mut pairs: Vec<SplitRow> = Vec::new();
    for file in &doc.files {
        pairs.push(SplitRow::Header(format!(
            "{} \u{2192} {}",
            file.old_label, file.new_label
        )));
        for hunk in &file.hunks {
            pairs.push(SplitRow::Header(hunk.header.clone()));
            let mut old_buf: Vec<&crate::widgets::diff::DiffLine> = Vec::new();
            let mut new_buf: Vec<&crate::widgets::diff::DiffLine> = Vec::new();
            let flush = |pairs: &mut Vec<SplitRow>,
                         o: &mut Vec<&crate::widgets::diff::DiffLine>,
                         n: &mut Vec<&crate::widgets::diff::DiffLine>| {
                let len = o.len().max(n.len());
                for i in 0..len {
                    let left = o.get(i).map(|l| (l.old_no, l.text.clone()));
                    let right = n.get(i).map(|l| (l.new_no, l.text.clone()));
                    pairs.push(SplitRow::Pair(left, right));
                }
                o.clear();
                n.clear();
            };
            for l in &hunk.lines {
                match l.kind {
                    DiffKind::Remove => old_buf.push(l),
                    DiffKind::Add => new_buf.push(l),
                    DiffKind::Context => {
                        flush(&mut pairs, &mut old_buf, &mut new_buf);
                        pairs.push(SplitRow::Pair(
                            Some((l.old_no, l.text.clone())),
                            Some((l.new_no, l.text.clone())),
                        ));
                    }
                }
            }
            flush(&mut pairs, &mut old_buf, &mut new_buf);
        }
    }

    let col_w = (inner_w.saturating_sub(3)) / 2;
    let gutter = 6usize;
    for (i, row) in pairs.iter().skip(scroll).take(list_h).enumerate() {
        let y = list_top + i as u16;
        match row {
            SplitRow::Header(text) => {
                set(
                    x,
                    y,
                    &Line::from(Span::styled(
                        truncate_end(text, inner_w),
                        Style::default().fg(theme.secondary),
                    )),
                    inner_w as u16,
                    buf,
                );
            }
            SplitRow::Pair(left, right) => {
                let left_line = split_cell(left, DiffKind::Remove, col_w, gutter, theme);
                let right_line = split_cell(right, DiffKind::Add, col_w, gutter, theme);
                let mut spans = left_line.spans;
                spans.push(Span::styled(
                    " \u{2502} ",
                    Style::default().fg(theme.border),
                ));
                spans.extend(right_line.spans);
                set(x, y, &Line::from(spans), inner_w as u16, buf);
            }
        }
    }
}

enum SplitRow {
    Header(String),
    /// 左（旧）与右（新）单元格内容 + 行号。
    Pair(Option<(Option<u32>, String)>, Option<(Option<u32>, String)>),
}

fn split_cell(
    cell: &Option<(Option<u32>, String)>,
    kind: DiffKind,
    col_w: usize,
    gutter: usize,
    theme: &Theme,
) -> Line<'static> {
    match cell {
        Some((no, text)) => {
            let (fg, bg) = kind_style(kind, theme);
            let no_s = no
                .map(|v| format!("{v:>4} "))
                .unwrap_or_else(|| "     ".to_string());
            let avail = col_w.saturating_sub(gutter);
            let body = truncate_end(text, avail);
            Line::from(vec![
                Span::styled(
                    no_s,
                    Style::default()
                        .fg(theme.diff_line_number)
                        .bg(theme.background),
                ),
                Span::styled(body, Style::default().fg(fg).bg(bg)),
            ])
        }
        None => Line::from(Span::raw(" ".repeat(col_w))),
    }
}

fn set(x: u16, y: u16, line: &Line<'_>, width: u16, buf: &mut Buffer) {
    buf.set_line(x, y, line, width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    const SAMPLE: &str = r#"Edited a.rs (1 replacement(s))
--- a.rs
+++ a.rs
@@ -1,3 +1,3 @@
 fn main() {
-    old();
+    new();
 }
"#;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn render_to(state: &DiffState) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        term.draw(|f| render(state, f.area(), f.buffer_mut(), &Theme::tokyonight()))
            .unwrap();
        term
    }

    fn contains(term: &Terminal<TestBackend>, needle: &str) -> bool {
        let buf = term.backend().buffer();
        let area = buf.area();
        for y in area.top()..area.bottom() {
            let mut row = String::new();
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell((x, y)) {
                    row.push_str(cell.symbol());
                }
            }
            if row.contains(needle) {
                return true;
            }
        }
        false
    }

    #[test]
    fn parses_and_renders_unified() {
        let state = DiffState::from_output("a.rs", SAMPLE).unwrap();
        let term = render_to(&state);
        assert!(contains(&term, "a.rs"));
        assert!(contains(&term, "new();"));
        assert!(contains(&term, "old();"));
        assert!(contains(&term, "unified"));
    }

    #[test]
    fn toggle_views() {
        let mut state = DiffState::from_output("a.rs", SAMPLE).unwrap();
        assert_eq!(state.view(), DiffView::Unified);
        state.on_key(&k(KeyCode::Char('t')));
        assert_eq!(state.view(), DiffView::Split);
        let term = render_to(&state);
        assert!(contains(&term, "split"));
    }

    #[test]
    fn non_diff_output_returns_none() {
        assert!(DiffState::from_output("x", "just text").is_none());
    }

    #[test]
    fn esc_closes() {
        let mut state = DiffState::from_output("a.rs", SAMPLE).unwrap();
        assert!(matches!(
            state.on_key(&k(KeyCode::Esc)),
            DialogOutcome::Closed
        ));
    }
}
