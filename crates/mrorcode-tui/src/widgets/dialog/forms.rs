//! Confirm 等表单式对话框（M2-10 先上 Confirm；Alert/Help 在 M2-11）。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::render::truncate_end;
use crate::theme::Theme;
use crate::widgets::dialog::select::{fill, OVERLAY};

/// Confirm 确认后执行的动作（纯数据）。
#[derive(Clone, Debug)]
pub enum ConfirmAction {
    /// 删除指定会话。
    DeleteSession(u64),
}

#[derive(Debug)]
pub struct ConfirmState {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
}

impl ConfirmState {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        action: ConfirmAction,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            action,
        }
    }

    pub fn on_key(&mut self, key: &KeyEvent) -> crate::widgets::dialog::DialogOutcome {
        use crate::widgets::dialog::DialogOutcome;
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => DialogOutcome::Confirmed(self.action.clone()),
            KeyCode::Esc | KeyCode::Char('n') => DialogOutcome::Closed,
            _ => DialogOutcome::None,
        }
    }
}

pub fn render(confirm: &ConfirmState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    // 遮罩
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(OVERLAY));
            }
        }
    }

    // 紧凑居中框：宽 60%（最小 36，最大 72），高固定 7 行
    let w = (area.width * 3 / 5).clamp(36, 72.min(area.width.saturating_sub(2)));
    let h = 7u16.min(area.height.saturating_sub(2));
    let px = area.x + (area.width - w) / 2;
    let py = area.y + (area.height - h) / 2;
    let panel = Rect {
        x: px,
        y: py,
        width: w,
        height: h,
    };
    fill(panel, buf, theme.background_menu);
    crate::widgets::dialog::select::draw_border_static(panel, buf, theme);

    let inner_w = w as usize - 4;

    set(
        px + 2,
        py + 1,
        &Line::from(Span::styled(
            truncate_end(&confirm.title, inner_w),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )),
        w - 4,
        buf,
    );

    // message 最多两行
    for (i, line) in confirm.message.lines().take(2).enumerate() {
        set(
            px + 2,
            py + 2 + i as u16,
            &Line::from(Span::styled(
                truncate_end(line, inner_w),
                Style::default().fg(theme.text),
            )),
            w - 4,
            buf,
        );
    }

    set(
        px + 2,
        py + h - 2,
        &Line::from(Span::styled(
            "y/enter 确认 \u{00B7} n/esc 取消",
            Style::default().fg(theme.text_muted),
        )),
        w - 4,
        buf,
    );
}

fn set(x: u16, y: u16, line: &Line<'_>, width: u16, buf: &mut Buffer) {
    buf.set_line(x, y, line, width);
}

/// 单按钮提示框（仅有“知道了”）。
#[derive(Debug)]
pub struct AlertState {
    pub title: String,
    pub message: String,
}

impl AlertState {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
        }
    }

    pub fn on_key(&mut self, key: &KeyEvent) -> crate::widgets::dialog::DialogOutcome {
        use crate::widgets::dialog::DialogOutcome;
        match key.code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') | KeyCode::Char('o') => {
                DialogOutcome::Closed
            }
            _ => DialogOutcome::None,
        }
    }
}

pub fn render_alert(alert: &AlertState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    render(
        &ConfirmState::new(
            alert.title.clone(),
            alert.message.clone(),
            ConfirmAction::DeleteSession(0),
        ),
        area,
        buf,
        theme,
    );
    // 覆盖底部提示为单按钮文案
    let w = (area.width * 3 / 5).clamp(36, 72.min(area.width.saturating_sub(2)));
    let h = 7u16.min(area.height.saturating_sub(2));
    let px = area.x + (area.width - w) / 2;
    let py = area.y + (area.height - h) / 2;
    set(
        px + 2,
        py + h - 2,
        &Line::from(Span::styled(
            "enter/esc 知道了",
            Style::default().fg(theme.text_muted),
        )),
        w - 4,
        buf,
    );
}

/// help 对话框一行（命令 + 当前真实键位，由 keymap 反查生成）。
#[derive(Clone, Debug)]
pub struct HelpRow {
    pub group: String,
    pub name: String,
    pub desc: String,
    pub keys: Vec<String>,
}

#[derive(Debug)]
pub struct HelpState {
    rows: Vec<HelpRow>,
    scroll: usize,
}

impl HelpState {
    pub fn new(rows: Vec<HelpRow>) -> Self {
        Self { rows, scroll: 0 }
    }

    pub fn on_key(&mut self, key: &KeyEvent) -> crate::widgets::dialog::DialogOutcome {
        use crate::widgets::dialog::DialogOutcome;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => DialogOutcome::Closed,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                DialogOutcome::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                DialogOutcome::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                DialogOutcome::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                DialogOutcome::None
            }
            _ => DialogOutcome::None,
        }
    }
}

pub fn render_help(help: &HelpState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    // 遮罩
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(OVERLAY));
            }
        }
    }

    let w = (area.width * 7 / 10).clamp(48, area.width.saturating_sub(2));
    let h = (area.height * 4 / 5).clamp(16, area.height.saturating_sub(2));
    let px = area.x + (area.width - w) / 2;
    let py = area.y + (area.height - h) / 2;
    let panel = Rect {
        x: px,
        y: py,
        width: w,
        height: h,
    };
    fill(panel, buf, theme.background_menu);
    crate::widgets::dialog::select::draw_border_static(panel, buf, theme);

    let inner_w = w as usize - 4;

    set(
        px + 2,
        py + 1,
        &Line::from(Span::styled(
            "help \u{00B7} keybindings",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        w - 4,
        buf,
    );

    let list_top = py + 3;
    let list_h = h as usize - 5; // 标题/分隔/footer

    // 构造展示行（分组标题 + 命令行）
    let mut display: Vec<Line<'static>> = Vec::new();
    let mut last_group = String::new();
    for row in &help.rows {
        if row.group != last_group {
            display.push(Line::from(Span::styled(
                format!(" {} ", row.group),
                Style::default()
                    .fg(theme.secondary)
                    .add_modifier(Modifier::BOLD),
            )));
            last_group = row.group.clone();
        }
        let keys = if row.keys.is_empty() {
            "\u{2014}".to_string() // —
        } else {
            row.keys.join(" / ")
        };
        let keys_w = crate::render::str_width(&keys);
        let name_w = inner_w.saturating_sub(keys_w + 2);
        let name = crate::render::truncate_end(&row.name, name_w);
        let pad = name_w.saturating_sub(crate::render::str_width(&name)) + 1;
        display.push(Line::from(vec![
            Span::styled(name, Style::default().fg(theme.text)),
            Span::raw(" ".repeat(pad)),
            Span::styled(keys, Style::default().fg(theme.text_muted)),
        ]));
    }

    let max_scroll = display.len().saturating_sub(list_h);
    let scroll = help.scroll.min(max_scroll);
    for (i, line) in display.into_iter().skip(scroll).take(list_h).enumerate() {
        set(px + 2, list_top + i as u16, &line, w - 4, buf);
    }

    set(
        px + 2,
        py + h - 2,
        &Line::from(Span::styled(
            "\u{2191}\u{2193} 滚动 \u{00B7} q/esc 关闭（键位含用户覆盖）",
            Style::default().fg(theme.text_muted),
        )),
        w - 4,
        buf,
    );
}
