//! which-key 面板：对齐 opencode——默认关闭（`ctrl+alt+k` 开关，持久化），
//! 底部固定高度面板（屏幕 30%，钳制 8..16），按命令分组以页签切换，
//! `ctrl+alt+shift+k` 在 dock（内容让位）/ overlay（浮于内容之上）间切换。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Widget};

use crate::keymap::{Action, Keymap};
use crate::theme::Theme;

/// 面板高度占屏幕高度的百分比（opencode `PANEL_HEIGHT_RATIO`）。
const PANEL_HEIGHT_RATIO: u16 = 30;
const MIN_PANEL_HEIGHT: u16 = 8;
const MAX_PANEL_HEIGHT: u16 = 16;
/// 固定行：顶部留白 + 页签 + 页签下留白 + 页脚留白/页脚。
const TOP_PADDING: u16 = 1;
const HEADER_H: u16 = 1;
const TAB_GAP: u16 = 1;
const FOOTER_H: u16 = 2;
/// 分栏：最多 3 列，列间距 4，单列宽度 28..44（opencode 同值）。
const MAX_COLUMNS: u16 = 3;
const COLUMN_GAP: u16 = 4;
const MIN_COLUMN_WIDTH: u16 = 28;
const MAX_COLUMN_WIDTH: u16 = 44;

/// 面板布局：dock=底部内嵌（内容让位）；overlay=浮于内容之上。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WkLayout {
    Dock,
    Overlay,
}

impl WkLayout {
    /// 面板关闭时下一次切换将进入的布局名（页脚提示用）。
    pub fn other(self) -> &'static str {
        match self {
            WkLayout::Dock => "overlay",
            WkLayout::Overlay => "dock",
        }
    }
}

/// 一个键位条目：第二键 + 说明 + 所属分组。
pub struct WkItem {
    pub key: String,
    pub desc: &'static str,
    pub group: &'static str,
}

/// 面板交互状态：当前分组下标与组内滚动偏移（不持久化）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WkState {
    pub group: Option<usize>,
    pub offset: usize,
}

/// 界面说明文案。
pub fn action_label(a: Action) -> &'static str {
    match a {
        Action::NewSession => "新会话",
        Action::ListSessions => "会话列表",
        Action::DeleteSession => "删除会话",
        Action::ToggleSidebar => "切换侧边栏",
        Action::OpenModelPicker => "模型选择",
        Action::OpenCommandPalette => "命令面板",
        Action::OpenHelp => "帮助",
        Action::Quit => "退出",
        Action::Interrupt => "中断",
        Action::SwitchSession(_) => "切换会话",
        Action::Submit => "发送",
        Action::Newline => "换行",
        Action::CancelOrClear => "取消/清空",
        Action::ScrollUp | Action::ScrollDown => "滚动",
        Action::HistoryPrev | Action::HistoryNext => "历史",
        Action::FocusNextTool | Action::FocusPrevTool => "工具焦点",
        Action::ToggleFocusedTool => "展开工具卡",
        Action::CycleAgent | Action::CycleAgentReverse => "切换 agent",
        Action::CycleModel | Action::CycleModelReverse => "循环模型",
        Action::ToggleWhichKey => "键位面板",
        Action::ToggleWhichKeyLayout => "面板布局",
        Action::WhichKeyGroupPrev | Action::WhichKeyGroupNext => "上/下一分组",
        Action::WhichKeyScrollUp | Action::WhichKeyScrollDown => "上滚/下滚",
        Action::WhichKeyPageUp | Action::WhichKeyPageDown => "上翻页/下翻页",
        Action::WhichKeyHome | Action::WhichKeyEnd => "首/尾键位",
        Action::ToggleReasoning => "思考折叠",
        Action::OpenDiffViewer => "diff 查看",
        Action::NextSession => "下一会话",
        Action::PrevSession => "上一会话",
        Action::ToggleSelect => "选择文本",
        Action::CopySelection => "复制选区",
        Action::Rewind => "回退会话",
        Action::PinSession => "固定会话",
        Action::RenameSession => "重命名会话",
        Action::BackgroundTask => "后台运行任务",
        Action::OpenTimeline => "消息时间线",
        Action::ExportTranscript => "导出会话",
        Action::WidenSidebar => "加宽侧栏",
        Action::NarrowSidebar => "缩窄侧栏",
        Action::OpenExternalEditor => "外部编辑器",
        Action::SwitchProvider => "切换 provider",
    }
}

/// 分组名（与命令面板 `CommandCategory` 同名，opencode 按 category 分组）。
pub fn action_group(a: Action) -> &'static str {
    match a {
        Action::NewSession
        | Action::ListSessions
        | Action::DeleteSession
        | Action::SwitchSession(_)
        | Action::OpenModelPicker
        | Action::SwitchProvider
        | Action::Rewind
        | Action::PinSession
        | Action::RenameSession
        | Action::BackgroundTask
        | Action::OpenTimeline
        | Action::ExportTranscript => "Session",
        Action::ToggleSidebar
        | Action::NextSession
        | Action::PrevSession
        | Action::ToggleReasoning
        | Action::OpenDiffViewer
        | Action::ToggleSelect
        | Action::CopySelection
        | Action::ScrollUp
        | Action::ScrollDown
        | Action::HistoryPrev
        | Action::HistoryNext
        | Action::FocusNextTool
        | Action::FocusPrevTool
        | Action::ToggleFocusedTool
        | Action::WidenSidebar
        | Action::NarrowSidebar => "View",
        Action::ToggleWhichKey
        | Action::ToggleWhichKeyLayout
        | Action::WhichKeyGroupPrev
        | Action::WhichKeyGroupNext
        | Action::WhichKeyScrollUp
        | Action::WhichKeyScrollDown
        | Action::WhichKeyPageUp
        | Action::WhichKeyPageDown
        | Action::WhichKeyHome
        | Action::WhichKeyEnd => "System",
        Action::OpenCommandPalette
        | Action::OpenHelp
        | Action::Quit
        | Action::Interrupt
        | Action::Submit
        | Action::Newline
        | Action::CancelOrClear
        | Action::CycleAgent
        | Action::CycleAgentReverse
        | Action::CycleModel
        | Action::CycleModelReverse
        | Action::OpenExternalEditor => "App",
    }
}

/// leader 第二键列表（数据源含用户覆盖，按键排序）。
pub fn leader_items(km: &Keymap) -> Vec<WkItem> {
    km.leader_bindings()
        .into_iter()
        .map(|(key, action)| WkItem {
            key,
            desc: action_label(action),
            group: action_group(action),
        })
        .collect()
}

/// 分组标签（去重并按字典序）。
pub fn group_labels(items: &[WkItem]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = items.iter().map(|i| i.group).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// 生效分组：按下标取，失效时回退首个分组。
pub fn resolve_group(state: &WkState, items: &[WkItem]) -> Option<&'static str> {
    let labels = group_labels(items);
    let idx = state
        .group
        .filter(|i| *i < labels.len())
        .unwrap_or_default();
    labels.get(idx).copied()
}

/// 指定分组的条目。
pub fn group_entries<'a>(items: &'a [WkItem], group: &str) -> Vec<&'a WkItem> {
    items.iter().filter(|i| i.group == group).collect()
}

/// 面板高度（屏幕 30%，钳制 8..16，且不超过屏幕）。
pub fn height(screen_height: u16) -> u16 {
    let ratio = screen_height.saturating_mul(PANEL_HEIGHT_RATIO) / 100;
    ratio
        .clamp(MIN_PANEL_HEIGHT, MAX_PANEL_HEIGHT)
        .min(screen_height)
}

/// 可用内容宽度（左右各留 1）。
fn content_width(width: u16) -> u16 {
    width.saturating_sub(2)
}

/// 列数（最多 3 列）。
pub fn columns(width: u16) -> u16 {
    let cw = content_width(width);
    ((cw + COLUMN_GAP) / (MAX_COLUMN_WIDTH + COLUMN_GAP)).clamp(1, MAX_COLUMNS)
}

/// 单列宽度。
pub fn column_width(width: u16) -> u16 {
    let cw = content_width(width);
    let cols = columns(width);
    let avail = cw.saturating_sub((cols - 1) * COLUMN_GAP) / cols;
    avail
        .clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
        .min(cw)
        .max(1)
}

/// 条目行数（扣掉固定行）。
pub fn rows(panel_height: u16) -> u16 {
    panel_height
        .saturating_sub(TOP_PADDING + HEADER_H + TAB_GAP + FOOTER_H)
        .max(1)
}

/// 一页条目数（行 × 列）。
pub fn page_size(width: u16, panel_height: u16) -> usize {
    rows(panel_height) as usize * columns(width) as usize
}

/// 当前分组可滚动到的最大偏移。
pub fn max_offset(items: &[WkItem], group: &str, width: u16, panel_height: u16) -> usize {
    group_entries(items, group)
        .len()
        .saturating_sub(page_size(width, panel_height))
}

/// 某个动作的首个生效键位串（页脚提示用）。
fn first_key(km: &Keymap, action: Action) -> String {
    km.keys_for(action)
        .into_iter()
        .next()
        .unwrap_or_else(|| "-".to_string())
}

/// 渲染面板。`area` 为底部全宽的固定高度矩形。
pub fn render(
    km: &Keymap,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    layout: WkLayout,
    state: &WkState,
) {
    let items = leader_items(km);
    let Some(group) = resolve_group(state, &items) else {
        return;
    };
    Block::default()
        .style(Style::default().bg(theme.background_menu))
        .render(area, buf);

    let rows_n = rows(area.height);
    let cols_n = columns(area.width);
    let col_w = column_width(area.width);
    let entries = group_entries(&items, group);
    let max_off = entries
        .len()
        .saturating_sub(rows_n as usize * cols_n as usize);

    // 页脚：左侧开关提示，右侧下一布局提示
    let footer_y = area.bottom().saturating_sub(1);
    let inner_w = content_width(area.width);
    if footer_y >= area.y && inner_w > 0 {
        let left = format!("toggle {}", first_key(km, Action::ToggleWhichKey));
        let right = format!(
            "{} {}",
            layout.other(),
            first_key(km, Action::ToggleWhichKeyLayout)
        );
        let pad = inner_w
            .saturating_sub(str_width(&left) as u16)
            .saturating_sub(str_width(&right) as u16);
        let mut spans = vec![Span::styled(left, Style::default().fg(theme.text))];
        spans.push(Span::raw(" ".repeat(pad as usize)));
        spans.push(Span::styled(right, Style::default().fg(theme.text)));
        buf.set_line(area.x + 1, footer_y, &Line::from(spans), inner_w);
    }

    // 分组页签：当前页签用主色底，末尾附可滚动指示
    let header_y = area.y + TOP_PADDING;
    if header_y < area.bottom() {
        let mut spans: Vec<Span> = Vec::new();
        for (i, label) in group_labels(&items).iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            let text = format!(" {label} ");
            if *label == group {
                spans.push(Span::styled(
                    text,
                    Style::default()
                        .fg(theme.selected_text)
                        .bg(theme.primary)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(text, Style::default().fg(theme.text_muted)));
            }
        }
        if max_off > 0 {
            let up = if state.offset > 0 {
                theme.text
            } else {
                theme.text_muted
            };
            let down = if state.offset < max_off {
                theme.text
            } else {
                theme.text_muted
            };
            spans.push(Span::raw(" "));
            spans.push(Span::styled("↑", Style::default().fg(up)));
            spans.push(Span::raw(" "));
            spans.push(Span::styled("↓", Style::default().fg(down)));
        }
        let line = Line::from(spans);
        let line_w = str_width(&line.to_string()) as u16;
        let inner_w = content_width(area.width);
        if inner_w == 0 {
            return;
        }
        let x = area.x + 1 + inner_w.saturating_sub(line_w) / 2;
        buf.set_line(x, header_y, &line, inner_w.saturating_sub(x - area.x - 1));
    }

    // 条目：按列填充，说明靠左、键位靠右
    let inner_w = content_width(area.width);
    let col_w = col_w.min(inner_w);
    if col_w == 0 {
        return;
    }
    let grid_w = cols_n * col_w + (cols_n - 1) * COLUMN_GAP;
    let grid_x = area.x + 1 + inner_w.saturating_sub(grid_w) / 2;
    for col in 0..cols_n {
        for row in 0..rows_n {
            let Some(item) =
                entries.get(state.offset + col as usize * rows_n as usize + row as usize)
            else {
                continue;
            };
            let key_w = str_width(&item.key) as u16;
            let label_w = col_w.saturating_sub(key_w + 1);
            let label = crate::render::truncate_end(item.desc, label_w as usize);
            let gap = col_w
                .saturating_sub(key_w)
                .saturating_sub(str_width(&label) as u16 + 1);
            let line = Line::from(vec![
                Span::styled(label, Style::default().fg(theme.text_muted)),
                Span::raw(" ".repeat(gap as usize)),
                Span::styled(
                    item.key.clone(),
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
            ]);
            let x = grid_x + col * (col_w + COLUMN_GAP);
            let y = area.y + TOP_PADDING + HEADER_H + TAB_GAP + row;
            if y >= footer_y {
                break;
            }
            buf.set_line(x, y, &line, col_w);
        }
    }
}

fn str_width(s: &str) -> usize {
    crate::render::str_width(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn buffer_text(term: &Terminal<TestBackend>) -> String {
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn leader_bindings_list_has_expectations() {
        let km = Keymap::default();
        let items = leader_items(&km);
        assert!(items.iter().any(|i| i.key == "n" && i.desc == "新会话"));
        assert!(items.iter().any(|i| i.key == "d" && i.desc == "diff 查看"));
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"1") && keys.contains(&"9"));
    }

    #[test]
    fn items_are_grouped_into_categories() {
        let km = Keymap::default();
        let items = leader_items(&km);
        let labels = group_labels(&items);
        assert!(labels.contains(&"Session"));
        assert!(labels.contains(&"View"));
        // 同键只属于一个分组
        for (i, a) in items.iter().enumerate() {
            for b in &items[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
    }

    #[test]
    fn panel_height_is_thirty_percent_clamped() {
        assert_eq!(height(80), 16);
        assert_eq!(height(40), 12);
        assert_eq!(height(20), 8);
        assert_eq!(height(4), 4);
    }

    #[test]
    fn columns_grow_with_width_but_cap_at_three() {
        assert_eq!(columns(30), 1);
        assert_eq!(columns(120), 2);
        assert_eq!(columns(300), 3);
    }

    #[test]
    fn out_of_range_group_falls_back_to_first() {
        let km = Keymap::default();
        let items = leader_items(&km);
        let first = group_labels(&items)[0];
        let state = WkState {
            group: Some(99),
            offset: 0,
        };
        assert_eq!(resolve_group(&state, &items), Some(first));
        assert_eq!(resolve_group(&WkState::default(), &items), Some(first));
    }

    #[test]
    fn max_offset_is_zero_when_page_fits() {
        let km = Keymap::default();
        let items = leader_items(&km);
        let group = resolve_group(&WkState::default(), &items).unwrap();
        // 首组条目数不超过 3 列 × (12-5) 行
        assert_eq!(max_offset(&items, group, 200, 12), 0);
    }

    #[test]
    fn renders_group_tabs_entries_and_footer() {
        let km = Keymap::default();
        let items = leader_items(&km);
        let group = resolve_group(&WkState::default(), &items).unwrap();
        let first_key = group_entries(&items, group)[0].key.clone();
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let h = height(30);
        let area = Rect {
            x: 0,
            y: 30 - h,
            width: 120,
            height: h,
        };
        term.draw(|f| {
            render(
                &km,
                area,
                f.buffer_mut(),
                &Theme::default(),
                WkLayout::Dock,
                &WkState::default(),
            )
        })
        .unwrap();
        let text = buffer_text(&term);
        for label in group_labels(&items) {
            assert!(text.contains(label), "group tab {label} missing");
        }
        // 当前分组首个键位可见（CJK 说明为双宽，占位 cell 会打断整串匹配）
        assert!(text.contains(&first_key), "entry key {first_key} missing");
        assert!(text.contains("toggle ctrl+alt+k"), "footer hint missing");
        assert!(
            text.contains("overlay ctrl+alt+shift+k"),
            "footer layout missing"
        );
    }

    #[test]
    fn scroll_indicator_hidden_when_group_fits() {
        let km = Keymap::default();
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let h = height(30);
        let area = Rect {
            x: 0,
            y: 30 - h,
            width: 120,
            height: h,
        };
        term.draw(|f| {
            render(
                &km,
                area,
                f.buffer_mut(),
                &Theme::default(),
                WkLayout::Dock,
                &WkState::default(),
            )
        })
        .unwrap();
        assert!(!buffer_text(&term).contains('↓'));
    }

    #[test]
    fn footer_shows_dock_when_currently_overlay() {
        let km = Keymap::default();
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let h = height(30);
        let area = Rect {
            x: 0,
            y: 30 - h,
            width: 120,
            height: h,
        };
        term.draw(|f| {
            render(
                &km,
                area,
                f.buffer_mut(),
                &Theme::default(),
                WkLayout::Overlay,
                &WkState::default(),
            )
        })
        .unwrap();
        assert!(buffer_text(&term).contains("dock ctrl+alt+shift+k"));
    }

    #[test]
    fn narrow_panel_does_not_panic() {
        let km = Keymap::default();
        for (w, h) in [(1u16, 1u16), (3, 2), (10, 5), (40, 8)] {
            let mut term = Terminal::new(TestBackend::new(w.max(1), h.max(1))).unwrap();
            let area = Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            };
            term.draw(|f| {
                render(
                    &km,
                    area,
                    f.buffer_mut(),
                    &Theme::default(),
                    WkLayout::Dock,
                    &WkState {
                        group: Some(0),
                        offset: 99,
                    },
                )
            })
            .unwrap();
        }
    }
}
