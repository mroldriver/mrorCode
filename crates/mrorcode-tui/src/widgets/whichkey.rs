//! which-key 浮层：leader 窗口内列出全部可用第二键（M4-7）。
//! 纯渲染函数 + 数据源 `Keymap::leader_bindings()`，无状态；测试用 TestBackend 快照。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use crate::keymap::{Action, Keymap};
use crate::theme::Theme;

/// 每个第二键 + 人类可读描述。
pub struct LeaderItem {
    pub key: String,
    pub desc: &'static str,
}

/// 界面说明文案（与 help 一致）。
pub fn action_label(a: Action) -> &'static str {
    match a {
        Action::NewSession => "新会话",
        Action::ListSessions => "会话列表",
        Action::DeleteSession => "删除会话",
        Action::OpenThemePicker => "主题选择",
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
        Action::TogglePlanMode => "plan 模式",
        Action::ToggleReasoning => "思考折叠",
        Action::OpenDiffViewer => "diff 查看",
        Action::ToggleTabs => "切换标签栏",
        Action::NextSession => "下一会话",
        Action::PrevSession => "上一会话",
        Action::ToggleSelect => "选择文本",
        Action::CopySelection => "复制选区",
        Action::Rewind => "回退会话",
        Action::SwitchProvider => "切换 provider",
    }
}

/// 当前 leader 窗口内受支持的第二键列表（数据源合并，去重）。
pub fn leader_items(km: &Keymap) -> Vec<LeaderItem> {
    km.leader_bindings()
        .into_iter()
        .map(|(key, action)| LeaderItem {
            key,
            desc: action_label(action),
        })
        .collect()
}

/// 渲染 which-key 浮层到底部；返回所需高度（0 表示未激活）。
pub fn height(km: &Keymap, width: u16) -> u16 {
    let items = leader_items(km);
    if items.is_empty() {
        return 0;
    }
    // 两列布局：每项 `key  desc`，按项数估算行数
    let _ = width;
    (items.len() as u16).saturating_add(1).min(8)
}

pub fn render(km: &Keymap, area: Rect, buf: &mut Buffer, theme: &Theme) {
    let items = leader_items(km);
    if items.is_empty() {
        return;
    }
    let boxed = area.height.saturating_sub(2);
    let mut lines: Vec<Line> = Vec::new();
    for (i, it) in items.iter().take(boxed.max(1) as usize).enumerate() {
        let tag = format!("{:>2}", it.key);
        let spans = vec![
            Span::styled(
                tag,
                Style::default()
                    .fg(theme.border)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(it.desc, Style::default().fg(theme.text)),
        ];
        let _ = i;
        lines.push(Line::from(spans));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title("leader")
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    block.render(area, buf);
    let start_y = inner.top();
    for (idx, line) in lines.iter().enumerate() {
        if start_y + idx as u16 >= inner.bottom() {
            break;
        }
        buf.set_line(inner.left(), start_y + idx as u16, line, inner.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Mode, ThemeSet};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn test_theme() -> Theme {
        let name = ThemeSet::bundled(Mode::Dark)
            .names()
            .next()
            .expect("at least one bundled theme")
            .to_string();
        ThemeSet::bundled(Mode::Dark)
            .get(&name)
            .expect("bundled theme")
            .clone()
    }

    #[test]
    fn leader_bindings_list_has_expectations() {
        let km = Keymap::default();
        let items = leader_items(&km);
        assert!(items.iter().any(|i| i.key == "n" && i.desc == "新会话"));
        assert!(items.iter().any(|i| i.key == "d" && i.desc == "diff 查看"));
        assert!(items.iter().any(|i| i.key == "v" && i.desc == "选择文本"));
        // 数字键去重且排序
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"1") && keys.contains(&"9"));
        assert!(keys.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn switch_session_carries_static_label() {
        assert_eq!(action_label(Action::SwitchSession(3)), "切换会话");
    }

    #[test]
    fn renders_within_screen_and_no_panic() {
        let km = Keymap::default();
        let theme = test_theme();
        let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
        let mut h = 0;
        let mut area = Rect::ZERO;
        term.draw(|f| {
            h = height(&km, f.area().width);
            assert!(h > 0 && h <= 8);
            area = Rect::new(0, f.area().height - h, f.area().width, h);
            render(&km, area, f.buffer_mut(), &theme);
        })
        .unwrap();
        // 边框左上角应属于块描边
        let buf = term.backend().buffer();
        let corner = buf.cell((area.x, area.y)).expect("cell").symbol();
        assert_eq!(corner, "┌");
    }
}
