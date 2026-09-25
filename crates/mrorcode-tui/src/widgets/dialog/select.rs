//! 通用 fuzzy Select 对话框。
//!
//! 顶部过滤输入（实时 fuzzy）、分组标题、●/○ 当前项、底部键位提示；
//! 60% 居中面板 + 深色近似遮罩。数据纯值（SelectPurpose + value），
//! 接受后由 app 统一分派，组件不持有任何状态闭包。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::fuzzy;
use crate::render::{str_width, take_by_width, truncate_end};
use crate::theme::Theme;

/// 遮罩底色（ratatui 无 alpha，用深色块近似半透明压暗）。
pub(crate) const OVERLAY: ratatui::style::Color = ratatui::style::Color::Rgb(0x0d, 0x0f, 0x17);

/// 选择接受后要执行的动作类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectPurpose {
    Theme,
    Model,
    Session,
    Command,
    /// 回退到某轮之前（M4-9）。
    Rewind,
}

#[derive(Clone, Debug)]
pub struct SelectItem {
    pub label: String,
    /// 右对齐辅助文本（如真实快捷键）。
    pub hint: String,
    /// 分组名；空串不渲染分组标题。首次出现的顺序即分组顺序。
    pub group: String,
    /// 接受时回传的值（通常等于 label 或 id）。
    pub value: String,
}

#[derive(Debug)]
pub enum Dialog {
    Select(SelectState),
    Confirm(crate::widgets::dialog::forms::ConfirmState),
    Alert(crate::widgets::dialog::forms::AlertState),
    Help(crate::widgets::dialog::forms::HelpState),
    /// 独立 diff viewer（M4-3）。
    Diff(crate::widgets::dialog::diff::DiffState),
}

#[derive(Clone, Debug)]
pub enum DialogOutcome {
    /// 按键被消费但无结果（继续停留在对话框）。
    None,
    /// 用户按 Esc/Ctrl+C 关闭。
    Closed,
    /// Enter 确认（purpose, value）。
    Accepted(SelectPurpose, String),
    /// Confirm 确认。
    Confirmed(crate::widgets::dialog::forms::ConfirmAction),
}

#[derive(Debug)]
pub struct SelectState {
    pub title: String,
    purpose: SelectPurpose,
    items: Vec<SelectItem>,
    query: String,
    /// (items 中的索引, score)
    filtered: Vec<(usize, u32)>,
    cursor: usize,
}

impl SelectState {
    pub fn new(title: impl Into<String>, purpose: SelectPurpose, items: Vec<SelectItem>) -> Self {
        let filtered = (0..items.len()).map(|i| (i, 0u32)).collect();
        Self {
            title: title.into(),
            purpose,
            items,
            query: String::new(),
            filtered,
            cursor: 0,
        }
    }

    pub fn purpose(&self) -> SelectPurpose {
        self.purpose
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// 过滤后的条目数。
    pub fn len(&self) -> usize {
        self.filtered.len()
    }

    /// 过滤后是否无结果。
    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    /// modal 层按键处理。
    pub fn on_key(&mut self, key: &KeyEvent) -> DialogOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => DialogOutcome::Closed,
            KeyCode::Char('c') if ctrl => DialogOutcome::Closed,
            KeyCode::Enter => self.accept(),
            KeyCode::Up => {
                self.move_cursor(-1);
                DialogOutcome::None
            }
            KeyCode::Down => {
                self.move_cursor(1);
                DialogOutcome::None
            }
            KeyCode::Char(c)
                if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) && c != '\t' =>
            {
                self.query.push(c);
                self.refilter();
                DialogOutcome::None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                DialogOutcome::None
            }
            _ => DialogOutcome::None,
        }
    }

    fn accept(&self) -> DialogOutcome {
        match self.filtered.get(self.cursor) {
            Some((idx, _)) => match self.items.get(*idx) {
                Some(item) => DialogOutcome::Accepted(self.purpose, item.value.clone()),
                None => DialogOutcome::Closed,
            },
            None => DialogOutcome::Closed,
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        let n = self.filtered.len();
        if n == 0 {
            self.cursor = 0;
            return;
        }
        let cur = self.cursor as i32;
        let next = (cur + delta).rem_euclid(n as i32) as usize;
        self.cursor = next;
    }

    fn refilter(&mut self) {
        self.filtered = fuzzy::filter(&self.query, self.items.iter().map(|i| i.label.as_str()));
        self.cursor = self.cursor.min(self.filtered.len().saturating_sub(1));
    }
}

// ---------------- 渲染 ----------------

/// 列表中的一行：分组标题或条目。
enum Row {
    Header(String),
    Item(usize), // filtered 数组的索引
}

/// 计算全部展示行（标题 + 条目），并返回当前光标所在行。
fn build_rows(sel: &SelectState) -> (Vec<Row>, Option<usize>) {
    let mut rows = Vec::new();
    let mut last_group = String::new();
    let mut cursor_row = None;
    for (fi, (idx, _)) in sel.filtered.iter().enumerate() {
        let item = &sel.items[*idx];
        if !item.group.is_empty() && item.group != last_group {
            rows.push(Row::Header(item.group.clone()));
            last_group = item.group.clone();
        }
        if fi == sel.cursor {
            cursor_row = Some(rows.len());
        }
        rows.push(Row::Item(fi));
    }
    (rows, cursor_row)
}

pub fn render(sel: &SelectState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    // 1) 遮罩压暗
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(OVERLAY));
            }
        }
    }

    // 2) 面板尺寸（60%，最小 40x14）
    let w = (area.width * 3 / 5).clamp(40, area.width.saturating_sub(2));
    let h = (area.height * 3 / 5).clamp(14, area.height.saturating_sub(2));
    let px = area.x + (area.width - w) / 2;
    let py = area.y + (area.height - h) / 2;
    let panel = Rect {
        x: px,
        y: py,
        width: w,
        height: h,
    };

    // 面板底色
    fill(panel, buf, theme.background_menu);
    // 边框
    draw_border_static(panel, buf, theme);

    let inner_w = w as usize - 2;
    let inner_x = px + 1;
    let inner_h = h as usize - 2;

    // 标题
    set_line(
        inner_x,
        py + 1,
        &Line::from(Span::styled(
            truncate_end(&sel.title, inner_w),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        w - 2,
        buf,
    );

    // 过滤输入
    let mut filter_spans = vec![Span::styled(
        "\u{276F} ", // ❯
        Style::default().fg(theme.primary),
    )];
    filter_spans.push(Span::styled(
        take_by_width(&sel.query, inner_w.saturating_sub(4)).to_string(),
        Style::default().fg(theme.selected_text),
    ));
    filter_spans.push(Span::styled(
        "\u{2588}".to_string(), // █ 光标
        Style::default().fg(theme.primary),
    ));
    set_line(inner_x, py + 2, &Line::from(filter_spans), w - 2, buf);

    // 分隔线
    let sep: String = "\u{2500}".repeat(inner_w); // ─
    set_line(
        inner_x,
        py + 3,
        &Line::from(Span::styled(sep, Style::default().fg(theme.border_subtle))),
        w - 2,
        buf,
    );

    // 列表视口
    let (rows, cursor_row) = build_rows(sel);
    let view_h = inner_h.saturating_sub(4); // 去掉标题/输入/分隔/footer
    let mut offset = 0usize;
    if let Some(cr) = cursor_row {
        if cr >= view_h {
            offset = (cr - view_h + 1).min(rows.len().saturating_sub(view_h));
        }
    }

    let list_y0 = py + 4;
    for (visible, row) in rows.iter().enumerate().skip(offset).take(view_h) {
        let y = list_y0 + (visible - offset) as u16;
        match row {
            Row::Header(name) => {
                let text = format!(" {name}");
                set_line(
                    inner_x,
                    y,
                    &Line::from(Span::styled(
                        truncate_end(&text, inner_w),
                        Style::default()
                            .fg(theme.secondary)
                            .add_modifier(Modifier::BOLD),
                    )),
                    w - 2,
                    buf,
                );
            }
            Row::Item(fi) => {
                let (idx, _) = sel.filtered[*fi];
                let item = &sel.items[idx];
                let selected = *fi == sel.cursor;
                let line = item_line(item, selected, &sel.query, inner_w, theme);
                if selected {
                    let row_rect = Rect {
                        x: inner_x,
                        y,
                        width: w - 2,
                        height: 1,
                    };
                    fill(row_rect, buf, theme.background_element);
                }
                set_line(inner_x, y, &line, w - 2, buf);
            }
        }
    }

    // 空结果提示
    if sel.filtered.is_empty() {
        set_line(
            inner_x,
            list_y0,
            &Line::from(Span::styled(
                "(无匹配项)",
                Style::default().fg(theme.text_muted),
            )),
            w - 2,
            buf,
        );
    }

    // footer
    let footer = "\u{2191}\u{2193} 移动 \u{00B7} enter 选择 \u{00B7} esc 关闭";
    set_line(
        inner_x,
        py + h - 2,
        &Line::from(Span::styled(
            take_by_width(footer, inner_w).to_string(),
            Style::default().fg(theme.text_muted),
        )),
        w - 2,
        buf,
    );
}

fn item_line(
    item: &SelectItem,
    selected: bool,
    query: &str,
    inner_w: usize,
    theme: &Theme,
) -> Line<'static> {
    let marker = if selected {
        Span::styled("\u{25CF} ", Style::default().fg(theme.primary)) // ●
    } else {
        Span::styled("\u{25CB} ", Style::default().fg(theme.text_muted)) // ○
    };

    // 右对齐 hint，label 占剩余宽度
    let hint_w = if item.hint.is_empty() {
        0
    } else {
        str_width(&item.hint) + 1
    };
    let label_max = inner_w.saturating_sub(2 + hint_w);

    // fuzzy 命中位置高亮（按 char 索引），同时按显示宽度裁剪到 label_max
    let hit: Vec<usize> = fuzzy::score(query, &item.label)
        .map(|(_, p)| p)
        .unwrap_or_default();
    let mut spans = vec![marker];
    let mut current = String::new();
    let mut current_hit = false;
    let mut used = 0usize;
    for (ci, c) in item.label.chars().enumerate() {
        let cw = crate::render::char_width(c);
        if used + cw > label_max {
            break;
        }
        used += cw;
        let is_hit = hit.contains(&ci);
        if is_hit != current_hit && !current.is_empty() {
            spans.push(label_span(&current, current_hit, theme));
            current.clear();
        }
        current.push(c);
        current_hit = is_hit;
    }
    if !current.is_empty() {
        spans.push(label_span(&current, current_hit, theme));
    }

    let mut line = Line::from(spans);
    if !item.hint.is_empty() {
        let pad = label_max.saturating_sub(used) + 1;
        line.spans.push(Span::raw(" ".repeat(pad)));
        line.spans.push(Span::styled(
            truncate_end(&item.hint, hint_w.saturating_sub(1).max(1)),
            Style::default().fg(theme.text_muted),
        ));
    }
    line
}

fn label_span(text: &str, hit: bool, theme: &Theme) -> Span<'static> {
    let style = if hit {
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text)
    };
    Span::styled(text.to_string(), style)
}

/// 对话框面板底色填充（forms 复用）。
pub(crate) fn fill(rect: Rect, buf: &mut Buffer, bg: ratatui::style::Color) {
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_style(Style::default().bg(bg));
            }
        }
    }
}

/// 对话框边框（forms 复用）。
pub(crate) fn draw_border_static(rect: Rect, buf: &mut Buffer, theme: &Theme) {
    let style = Style::default()
        .fg(theme.border_active)
        .bg(theme.background_menu);
    let set = |x: u16, y: u16, sym: &str, buf: &mut Buffer| {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(sym);
            cell.set_style(style);
        }
    };
    for x in rect.x + 1..rect.x + rect.width - 1 {
        set(x, rect.y, "\u{2500}", buf); // ─
        set(x, rect.y + rect.height - 1, "\u{2500}", buf);
    }
    for y in rect.y + 1..rect.y + rect.height - 1 {
        set(rect.x, y, "\u{2502}", buf); // │
        set(rect.x + rect.width - 1, y, "\u{2502}", buf);
    }
    set(rect.x, rect.y, "\u{250C}", buf); // ┌
    set(rect.x + rect.width - 1, rect.y, "\u{2510}", buf); // ┐
    set(rect.x, rect.y + rect.height - 1, "\u{2514}", buf); // └
    set(
        rect.x + rect.width - 1,
        rect.y + rect.height - 1,
        "\u{2518}",
        buf,
    ); // ┘
}

fn set_line(x: u16, y: u16, line: &Line<'_>, width: u16, buf: &mut Buffer) {
    buf.set_line(x, y, line, width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample() -> Vec<SelectItem> {
        vec![
            SelectItem {
                label: "new session".into(),
                hint: "ctrl+n".into(),
                group: "Suggested".into(),
                value: "new".into(),
            },
            SelectItem {
                label: "switch theme".into(),
                hint: "leader t".into(),
                group: "View".into(),
                value: "theme".into(),
            },
            SelectItem {
                label: "switch model".into(),
                hint: "leader m".into(),
                group: "View".into(),
                value: "model".into(),
            },
        ]
    }

    fn render_to(sel: &SelectState, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(sel, f.area(), f.buffer_mut(), &Theme::tokyonight()))
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
    fn renders_title_items_and_hints() {
        let sel = SelectState::new("command palette", SelectPurpose::Command, sample());
        let term = render_to(&sel, 100, 40);
        assert!(contains(&term, "command palette"));
        assert!(contains(&term, "new session"));
        assert!(contains(&term, "switch model"));
        assert!(contains(&term, "ctrl+n"));
        assert!(contains(&term, "Suggested"));
    }

    #[test]
    fn typing_filters_fuzzy() {
        let mut sel = SelectState::new("p", SelectPurpose::Command, sample());
        // "theme" 含 t-h-e-m-e，输入 "them" 只留 switch theme
        for c in "them".chars() {
            sel.on_key(&k(KeyCode::Char(c)));
        }
        assert_eq!(sel.len(), 1);
        let term = render_to(&sel, 100, 40);
        assert!(contains(&term, "switch theme"));
        assert!(!contains(&term, "new session"));
        assert!(!contains(&term, "switch model"));
    }

    #[test]
    fn backspace_restores_results() {
        let mut sel = SelectState::new("p", SelectPurpose::Command, sample());
        sel.on_key(&k(KeyCode::Char('z')));
        assert_eq!(sel.len(), 0);
        sel.on_key(&k(KeyCode::Backspace));
        assert_eq!(sel.len(), 3);
    }

    #[test]
    fn arrows_enter_returns_value_and_esc_closes() {
        let mut sel = SelectState::new("p", SelectPurpose::Theme, sample());
        sel.on_key(&k(KeyCode::Down));
        sel.on_key(&k(KeyCode::Down));
        let accepted = sel.on_key(&k(KeyCode::Enter));
        assert!(matches!(
            accepted,
            DialogOutcome::Accepted(SelectPurpose::Theme, v) if v == "model"
        ));

        let closed = sel.on_key(&k(KeyCode::Esc));
        assert!(matches!(closed, DialogOutcome::Closed));
    }

    #[test]
    fn cursor_wraps_around() {
        let mut sel = SelectState::new("p", SelectPurpose::Command, sample());
        sel.on_key(&k(KeyCode::Up)); // 0 向上 → 末尾
        let accepted = sel.on_key(&k(KeyCode::Enter));
        assert!(matches!(
            accepted,
            DialogOutcome::Accepted(SelectPurpose::Command, v) if v == "model"
        ));
    }

    #[test]
    fn enter_on_empty_result_closes() {
        let mut sel = SelectState::new("p", SelectPurpose::Command, sample());
        sel.on_key(&k(KeyCode::Char('z')));
        assert!(matches!(
            sel.on_key(&k(KeyCode::Enter)),
            DialogOutcome::Closed
        ));
    }
}
