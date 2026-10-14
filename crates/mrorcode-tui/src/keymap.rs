//! 键位系统：base/modal 两层模式 + leader 前缀 + 用户覆盖。
//!
//! - 紧凑键位串解析：`ctrl+p` / `f2` / `enter` / `shift+k` / `leader n`。
//! - 默认绑定对齐 opencode 习惯且保留 M1 键位；用户 KV 覆盖同触发器的默认。
//! - leader（默认 ctrl+x，由 app 做 2s 限时窗口）后的下一个键组合成命令。
//! - modal 层（对话框/审批）的键由组件自己处理，本模块只解析 base 层。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::storage::KeybindEntry;

/// 键位所在模式层。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Base,
    Modal,
}

/// leader 后续窗口时长。
pub const LEADER_TIMEOUT_MS: u64 = 2000;

/// 归一化后的按键（字母一律小写，shift 单独标记）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    pub code: PressCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressCode {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    F(u8),
}

/// 全部可绑定动作（命令面板动作 + 纯 UI 动作）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    // 命令面板同款
    NewSession,
    ListSessions,
    DeleteSession,
    ToggleSidebar,
    OpenModelPicker,
    OpenCommandPalette,
    OpenHelp,
    Quit,
    Interrupt,
    /// leader 1..9 切换会话（M2-10 接线）
    SwitchSession(u8),
    // 编辑/滚动/工具卡
    Submit,
    Newline,
    CancelOrClear,
    ScrollUp,
    ScrollDown,
    HistoryPrev,
    HistoryNext,
    FocusNextTool,
    FocusPrevTool,
    ToggleFocusedTool,
    /// 在 agent（build/plan）间循环（对齐 opencode `tab` / `shift+tab`）。
    CycleAgent,
    CycleAgentReverse,
    /// 在最近使用模型间循环（对齐 opencode `f2` / `shift+f2`）。
    CycleModel,
    CycleModelReverse,
    /// 开关 which-key 面板 / 切换 dock-overlay 布局。
    ToggleWhichKey,
    ToggleWhichKeyLayout,
    /// which-key 面板导航：分组、滚动、翻页、首尾（对齐 opencode 默认键位）。
    WhichKeyGroupPrev,
    WhichKeyGroupNext,
    WhichKeyScrollUp,
    WhichKeyScrollDown,
    WhichKeyPageUp,
    WhichKeyPageDown,
    WhichKeyHome,
    WhichKeyEnd,
    /// 展开/收起最近一条思考过程（M4-1）。
    ToggleReasoning,
    /// 打开当前聚焦工具卡的 diff viewer（M4-3）。
    OpenDiffViewer,
    /// 前后切换会话标签（M4-4）。
    NextSession,
    PrevSession,
    /// 进入选择模式 / 复制 / 取消（M4-6）。
    ToggleSelect,
    CopySelection,
    /// 回退会话到历史某轮之前（M4-9）。
    Rewind,
    /// 固定/取消固定当前会话。
    PinSession,
    /// 重命名当前会话（对齐 opencode ctrl+r）。
    RenameSession,
    /// 后台运行当前任务（ctrl+b）：agent 运行时新建会话，当前任务在后台继续。
    BackgroundTask,
    /// 打开消息时间线对话框（P2-9）。
    OpenTimeline,
    /// 导出当前会话为 Markdown 文件。
    ExportTranscript,
    /// 加宽/缩窄侧栏。
    WidenSidebar,
    NarrowSidebar,
    /// 在外部编辑器中打开当前输入。
    OpenExternalEditor,
    /// 跨 provider 切换（重建 runtime，M5-e）。
    SwitchProvider,
}

impl Action {
    /// KV 中使用的稳定 snake_case 标识。
    pub fn id(self) -> &'static str {
        match self {
            Action::NewSession => "new_session",
            Action::ListSessions => "list_sessions",
            Action::DeleteSession => "delete_session",
            Action::ToggleSidebar => "toggle_sidebar",
            Action::OpenModelPicker => "switch_model",
            Action::OpenCommandPalette => "command_palette",
            Action::OpenHelp => "help",
            Action::Quit => "quit",
            Action::Interrupt => "interrupt",
            Action::SwitchSession(_) => "switch_session",
            Action::Submit => "submit",
            Action::Newline => "newline",
            Action::CancelOrClear => "cancel_or_clear",
            Action::ScrollUp => "scroll_up",
            Action::ScrollDown => "scroll_down",
            Action::HistoryPrev => "history_prev",
            Action::HistoryNext => "history_next",
            Action::FocusNextTool => "tool_focus_next",
            Action::FocusPrevTool => "tool_focus_prev",
            Action::ToggleFocusedTool => "tool_toggle",
            Action::CycleAgent => "cycle_agent",
            Action::CycleAgentReverse => "cycle_agent_reverse",
            Action::CycleModel => "cycle_model",
            Action::CycleModelReverse => "cycle_model_reverse",
            Action::ToggleWhichKey => "toggle_which_key",
            Action::ToggleWhichKeyLayout => "toggle_which_key_layout",
            Action::WhichKeyGroupPrev => "which_key_group_prev",
            Action::WhichKeyGroupNext => "which_key_group_next",
            Action::WhichKeyScrollUp => "which_key_scroll_up",
            Action::WhichKeyScrollDown => "which_key_scroll_down",
            Action::WhichKeyPageUp => "which_key_page_up",
            Action::WhichKeyPageDown => "which_key_page_down",
            Action::WhichKeyHome => "which_key_home",
            Action::WhichKeyEnd => "which_key_end",
            Action::ToggleReasoning => "toggle_reasoning",
            Action::OpenDiffViewer => "open_diff",
            Action::NextSession => "next_session",
            Action::PrevSession => "prev_session",
            Action::ToggleSelect => "toggle_select",
            Action::CopySelection => "copy_selection",
            Action::Rewind => "rewind",
            Action::PinSession => "pin_session",
            Action::RenameSession => "rename_session",
            Action::BackgroundTask => "background_task",
            Action::OpenTimeline => "open_timeline",
            Action::ExportTranscript => "export_transcript",
            Action::WidenSidebar => "widen_sidebar",
            Action::NarrowSidebar => "narrow_sidebar",
            Action::OpenExternalEditor => "open_external_editor",
            Action::SwitchProvider => "switch_provider",
        }
    }

    fn from_id(id: &str) -> Option<Action> {
        Some(match id {
            "new_session" => Action::NewSession,
            "list_sessions" => Action::ListSessions,
            "delete_session" => Action::DeleteSession,
            "toggle_sidebar" => Action::ToggleSidebar,
            "switch_model" => Action::OpenModelPicker,
            "command_palette" => Action::OpenCommandPalette,
            "help" => Action::OpenHelp,
            "quit" => Action::Quit,
            "interrupt" => Action::Interrupt,
            "submit" => Action::Submit,
            "newline" => Action::Newline,
            "cancel_or_clear" => Action::CancelOrClear,
            "scroll_up" => Action::ScrollUp,
            "scroll_down" => Action::ScrollDown,
            "history_prev" => Action::HistoryPrev,
            "history_next" => Action::HistoryNext,
            "tool_focus_next" => Action::FocusNextTool,
            "tool_focus_prev" => Action::FocusPrevTool,
            "tool_toggle" => Action::ToggleFocusedTool,
            "cycle_agent" => Action::CycleAgent,
            "cycle_agent_reverse" => Action::CycleAgentReverse,
            "cycle_model" => Action::CycleModel,
            "cycle_model_reverse" => Action::CycleModelReverse,
            "toggle_which_key" => Action::ToggleWhichKey,
            "toggle_which_key_layout" => Action::ToggleWhichKeyLayout,
            "which_key_group_prev" => Action::WhichKeyGroupPrev,
            "which_key_group_next" => Action::WhichKeyGroupNext,
            "which_key_scroll_up" => Action::WhichKeyScrollUp,
            "which_key_scroll_down" => Action::WhichKeyScrollDown,
            "which_key_page_up" => Action::WhichKeyPageUp,
            "which_key_page_down" => Action::WhichKeyPageDown,
            "which_key_home" => Action::WhichKeyHome,
            "which_key_end" => Action::WhichKeyEnd,
            "toggle_reasoning" => Action::ToggleReasoning,
            "open_diff" => Action::OpenDiffViewer,
            "next_session" => Action::NextSession,
            "prev_session" => Action::PrevSession,
            "toggle_select" => Action::ToggleSelect,
            "copy_selection" => Action::CopySelection,
            "rewind" => Action::Rewind,
            "pin_session" => Action::PinSession,
            "rename_session" => Action::RenameSession,
            "background_task" => Action::BackgroundTask,
            "open_timeline" => Action::OpenTimeline,
            "export_transcript" => Action::ExportTranscript,
            "widen_sidebar" => Action::WidenSidebar,
            "narrow_sidebar" => Action::NarrowSidebar,
            "open_external_editor" => Action::OpenExternalEditor,
            "switch_provider" => Action::SwitchProvider,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Binding {
    leader: bool,
    press: KeyPress,
    action: Action,
}

/// 每个模式层一张绑定表（M2 仅 Base 有绑定；modal 键由对话框自管）。
pub struct Keymap {
    base: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new(&[]).0
    }
}

impl Keymap {
    /// 默认表 + 用户覆盖；无法解析的条目进 warnings（不影响其他绑定）。
    pub fn new(overrides: &[KeybindEntry]) -> (Self, Vec<String>) {
        let mut base = default_bindings();
        let mut warnings = Vec::new();

        for entry in overrides {
            let trigger = match parse_binding(&entry.keys) {
                Ok(t) => t,
                Err(e) => {
                    warnings.push(format!("ignoring keybind {:?}: {e}", entry.keys));
                    continue;
                }
            };
            let Some(action) = Action::from_id(&entry.action) else {
                warnings.push(format!(
                    "ignoring keybind {:?}: unknown action {:?}",
                    entry.keys, entry.action
                ));
                continue;
            };
            // 同触发器覆盖默认；重复用户条目后者覆盖前者
            base.retain(|b| (b.leader, b.press) != (trigger.0, trigger.1));
            base.push(Binding {
                leader: trigger.0,
                press: trigger.1,
                action,
            });
        }

        (Self { base }, warnings)
    }

    /// 解析一个按键；`leader` 表示当前处于 leader 等待窗口。
    pub fn resolve(&self, layer: Layer, key: &KeyEvent, leader: bool) -> Option<Action> {
        let press = KeyPress::from_event(key)?;
        let table = match layer {
            Layer::Base => &self.base,
            Layer::Modal => return None,
        };
        table
            .iter()
            .find(|b| b.leader == leader && b.press == press)
            .map(|b| b.action)
    }

    /// 反查某动作当前生效的全部键位串（help 对话框数据源，含用户覆盖）。
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        self.base
            .iter()
            .filter(|b| b.action == action)
            .map(|b| format_binding(b.leader, b.press))
            .collect()
    }

    /// 当前生效的全部 leader 第二键绑定（which-key 浮层数据源，含用户覆盖）。
    pub fn leader_bindings(&self) -> Vec<(String, Action)> {
        let mut out: Vec<(String, Action)> = self
            .base
            .iter()
            .filter(|b| b.leader)
            .map(|b| {
                let key = format_binding(false, b.press);
                (key, b.action)
            })
            .collect();
        // SwitchSession(1..9) 共享 id，但键位不同，需去重保留各键
        out.sort_by(|a, b| a.0.cmp(&b.0));
        // 同键不同动作（用户覆盖后不应发生，防御去重）
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }
}

/// 触发器反向格式化为紧凑键位串（parse_binding 的逆操作）。
pub fn format_binding(leader: bool, press: KeyPress) -> String {
    let mut parts: Vec<String> = Vec::new();
    if press.ctrl {
        parts.push("ctrl".to_string());
    }
    if press.alt {
        parts.push("alt".to_string());
    }
    if press.shift {
        parts.push("shift".to_string());
    }
    parts.push(match press.code {
        PressCode::Char(c) => c.to_string(),
        PressCode::Enter => "enter".to_string(),
        PressCode::Esc => "esc".to_string(),
        PressCode::Tab => "tab".to_string(),
        PressCode::Backspace => "backspace".to_string(),
        PressCode::Up => "up".to_string(),
        PressCode::Down => "down".to_string(),
        PressCode::Left => "left".to_string(),
        PressCode::Right => "right".to_string(),
        PressCode::PageUp => "pageup".to_string(),
        PressCode::PageDown => "pagedown".to_string(),
        PressCode::Home => "home".to_string(),
        PressCode::End => "end".to_string(),
        PressCode::F(n) => format!("f{n}"),
    });
    let key = parts.join("+");
    if leader {
        format!("leader {key}")
    } else {
        key
    }
}

impl KeyPress {
    /// 把 crossterm 事件归一化；不参与绑定的键（CapsLock 等）返回 None。
    pub fn from_event(key: &KeyEvent) -> Option<KeyPress> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let mut shift = key.modifiers.contains(KeyModifiers::SHIFT);

        let code = match key.code {
            KeyCode::Char(c) => {
                // 大写字母归一为小写 + shift
                let lc = c.to_ascii_lowercase();
                if c.is_ascii_uppercase() {
                    shift = true;
                }
                // ctrl 组合在终端里可能是控制字符（0x01 等），归一回字母
                let lc = if ctrl && ('\u{01}'..='\u{1a}').contains(&lc) {
                    char::from_u32(lc as u32 - 0x01 + b'a' as u32)?
                } else {
                    lc
                };
                PressCode::Char(lc)
            }
            KeyCode::Enter => PressCode::Enter,
            KeyCode::Esc => PressCode::Esc,
            KeyCode::Tab => PressCode::Tab,
            KeyCode::BackTab => {
                shift = true;
                PressCode::Tab
            }
            KeyCode::Backspace => PressCode::Backspace,
            KeyCode::Up => PressCode::Up,
            KeyCode::Down => PressCode::Down,
            KeyCode::Left => PressCode::Left,
            KeyCode::Right => PressCode::Right,
            KeyCode::PageUp => PressCode::PageUp,
            KeyCode::PageDown => PressCode::PageDown,
            KeyCode::Home => PressCode::Home,
            KeyCode::End => PressCode::End,
            KeyCode::F(n) => PressCode::F(n),
            _ => return None,
        };
        Some(KeyPress {
            code,
            ctrl,
            alt,
            shift,
        })
    }
}

/// 解析紧凑键位串为 (leader, press)。
pub fn parse_binding(s: &str) -> Result<(bool, KeyPress), String> {
    let raw = s.trim().to_ascii_lowercase();
    let mut parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.is_empty() {
        return Err("empty binding".into());
    }

    // leader 前缀："leader n"
    let leader = if parts[0] == "leader" {
        if parts.len() != 2 {
            return Err("leader binding expects exactly one key".into());
        }
        parts.remove(0);
        true
    } else {
        false
    };

    let mut it = parts[0].split('+');
    let key_tok = it.next_back().ok_or("missing key")?;
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    for modifier in it {
        match modifier {
            "ctrl" | "control" | "c" => ctrl = true,
            "alt" | "a" => alt = true,
            "shift" | "s" => shift = true,
            other => return Err(format!("unknown modifier: {other}")),
        }
    }

    let code = match key_tok {
        "enter" | "return" => PressCode::Enter,
        "esc" | "escape" => PressCode::Esc,
        "tab" => PressCode::Tab,
        "backspace" => PressCode::Backspace,
        "up" => PressCode::Up,
        "down" => PressCode::Down,
        "left" => PressCode::Left,
        "right" => PressCode::Right,
        "pageup" => PressCode::PageUp,
        "pagedown" => PressCode::PageDown,
        "home" => PressCode::Home,
        "end" => PressCode::End,
        t if t.len() >= 2 && t.starts_with('f') && t[1..].chars().all(|c| c.is_ascii_digit()) => {
            let n: u8 = t[1..]
                .parse()
                .map_err(|_| "function key out of range".to_string())?;
            if !(1..=24).contains(&n) {
                return Err("function key must be f1..f24".into());
            }
            PressCode::F(n)
        }
        t if t.chars().count() == 1 => {
            let c = t.chars().next().unwrap();
            if c.is_whitespace() {
                return Err("invalid char key".into());
            }
            PressCode::Char(c)
        }
        other => return Err(format!("unknown key: {other}")),
    };

    Ok((
        leader,
        KeyPress {
            code,
            ctrl,
            alt,
            shift,
        },
    ))
}

/// 默认键位（leader 第二键见 LEADER_*）。
fn default_bindings() -> Vec<Binding> {
    fn b(leader: bool, press: KeyPress, action: Action) -> Binding {
        Binding {
            leader,
            press,
            action,
        }
    }
    fn key(code: PressCode) -> KeyPress {
        KeyPress {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    fn ctrl(code: PressCode) -> KeyPress {
        KeyPress {
            code,
            ctrl: true,
            alt: false,
            shift: false,
        }
    }
    fn leader(code: PressCode) -> (bool, KeyPress) {
        (true, key(code))
    }
    fn ctrl_alt(code: PressCode) -> KeyPress {
        KeyPress {
            code,
            ctrl: true,
            alt: true,
            shift: false,
        }
    }
    fn ctrl_alt_shift(code: PressCode) -> KeyPress {
        KeyPress {
            code,
            ctrl: true,
            alt: true,
            shift: true,
        }
    }

    let plain = vec![
        b(false, key(PressCode::Enter), Action::Submit),
        // M1 习惯：Alt/Shift/Ctrl+Enter 换行
        b(
            false,
            KeyPress {
                code: PressCode::Enter,
                ctrl: false,
                alt: true,
                shift: false,
            },
            Action::Newline,
        ),
        b(
            false,
            KeyPress {
                code: PressCode::Enter,
                ctrl: false,
                alt: false,
                shift: true,
            },
            Action::Newline,
        ),
        b(
            false,
            KeyPress {
                code: PressCode::Enter,
                ctrl: true,
                alt: false,
                shift: false,
            },
            Action::Newline,
        ),
        b(false, ctrl(PressCode::Char('j')), Action::Newline),
        // 选择模式复制键（M4-6）：ctrl+shift+c
        b(
            false,
            KeyPress {
                code: PressCode::Char('c'),
                ctrl: true,
                alt: false,
                shift: true,
            },
            Action::CopySelection,
        ),
        b(false, key(PressCode::Esc), Action::CancelOrClear),
        b(false, ctrl(PressCode::Char('c')), Action::Quit),
        b(
            false,
            ctrl(PressCode::Char('p')),
            Action::OpenCommandPalette,
        ),
        b(false, ctrl(PressCode::Char('n')), Action::NewSession),
        b(false, ctrl(PressCode::Char('l')), Action::ListSessions),
        // 后台运行当前任务（ctrl+b）：agent 运行时新建会话，当前任务在后台继续
        b(false, ctrl(PressCode::Char('b')), Action::BackgroundTask),
        // 消息时间线（ctrl+shift+t）
        b(
            false,
            KeyPress {
                code: PressCode::Char('t'),
                ctrl: true,
                alt: false,
                shift: true,
            },
            Action::OpenTimeline,
        ),
        // 重命名当前会话（用 ctrl+shift+r 避免与 textarea 的 ctrl+r redo 冲突）
        b(
            false,
            KeyPress {
                code: PressCode::Char('r'),
                ctrl: true,
                alt: false,
                shift: true,
            },
            Action::RenameSession,
        ),
        // 对齐 opencode：F2/Shift+F2 循环最近模型
        b(false, key(PressCode::F(2)), Action::CycleModel),
        b(
            false,
            KeyPress {
                code: PressCode::F(2),
                ctrl: false,
                alt: false,
                shift: true,
            },
            Action::CycleModelReverse,
        ),
        b(false, key(PressCode::PageUp), Action::ScrollUp),
        b(false, key(PressCode::PageDown), Action::ScrollDown),
        // ↑↓：空输入框滚聊天，有内容时 textarea 移光标（apply_action 内判断）
        b(false, key(PressCode::Up), Action::ScrollUp),
        b(false, key(PressCode::Down), Action::ScrollDown),
        // 对齐 opencode：Tab/Shift+Tab 循环 agent（build/plan）
        b(false, key(PressCode::Tab), Action::CycleAgent),
        b(
            false,
            KeyPress {
                code: PressCode::Tab,
                ctrl: false,
                alt: false,
                shift: true,
            },
            Action::CycleAgentReverse,
        ),
        // 对齐 opencode：ctrl+alt+k 开关 which-key；加 shift 切布局
        b(
            false,
            ctrl_alt(PressCode::Char('k')),
            Action::ToggleWhichKey,
        ),
        b(
            false,
            ctrl_alt_shift(PressCode::Char('k')),
            Action::ToggleWhichKeyLayout,
        ),
        // which-key 面板导航（对齐 opencode 默认键位）
        b(false, ctrl_alt(PressCode::Left), Action::WhichKeyGroupPrev),
        b(false, ctrl_alt(PressCode::Right), Action::WhichKeyGroupNext),
        b(false, ctrl_alt(PressCode::Up), Action::WhichKeyScrollUp),
        b(
            false,
            ctrl_alt(PressCode::Char('p')),
            Action::WhichKeyScrollUp,
        ),
        b(false, ctrl_alt(PressCode::Down), Action::WhichKeyScrollDown),
        b(
            false,
            ctrl_alt(PressCode::Char('n')),
            Action::WhichKeyScrollDown,
        ),
        b(false, ctrl_alt(PressCode::PageUp), Action::WhichKeyPageUp),
        b(
            false,
            ctrl_alt(PressCode::PageDown),
            Action::WhichKeyPageDown,
        ),
        b(false, ctrl_alt(PressCode::Home), Action::WhichKeyHome),
        b(false, ctrl_alt(PressCode::End), Action::WhichKeyEnd),
        // Ctrl+Tab 前后切换会话标签；Ctrl+Shift+Tab 反向
        b(
            false,
            KeyPress {
                code: PressCode::Tab,
                ctrl: true,
                alt: false,
                shift: false,
            },
            Action::NextSession,
        ),
        b(
            false,
            KeyPress {
                code: PressCode::Tab,
                ctrl: true,
                alt: false,
                shift: true,
            },
            Action::PrevSession,
        ),
    ];

    let leaders = vec![
        leader(PressCode::Char('p')),
        leader(PressCode::Char('n')),
        leader(PressCode::Char('l')),
        leader(PressCode::Char('b')),
        leader(PressCode::Char('m')),
        leader(PressCode::Char('r')),
        leader(PressCode::Char('d')),
        leader(PressCode::Char('[')),
        leader(PressCode::Char(']')),
        leader(PressCode::Char('v')),
        leader(PressCode::Char('w')),
        leader(PressCode::Char('s')),
        leader(PressCode::Char('e')),
    ];
    let leader_actions = [
        Action::OpenCommandPalette,
        Action::NewSession,
        Action::ListSessions,
        Action::ToggleSidebar,
        Action::OpenModelPicker,
        Action::ToggleReasoning,
        Action::OpenDiffViewer,
        Action::PrevSession,
        Action::NextSession,
        Action::ToggleSelect,
        Action::Rewind,
        Action::SwitchProvider,
        Action::OpenExternalEditor,
    ];

    let mut all = plain;
    for ((is_leader, press), action) in leaders.into_iter().zip(leader_actions) {
        all.push(b(is_leader, press, action));
    }
    // leader 1..9 切换会话
    for n in 1..=9u8 {
        let (is_leader, press) = leader(PressCode::Char((b'0' + n) as char));
        all.push(b(is_leader, press, Action::SwitchSession(n)));
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_key_shapes() {
        assert!(parse_binding("ctrl+p").is_ok());
        let (leader, press) = parse_binding("leader n").unwrap();
        assert!(leader);
        assert_eq!(press.code, PressCode::Char('n'));

        let (_, f2) = parse_binding("f2").unwrap();
        assert_eq!(f2.code, PressCode::F(2));

        let (_, enter) = parse_binding("ctrl+alt+enter").unwrap();
        assert!(enter.ctrl && enter.alt);
        assert_eq!(enter.code, PressCode::Enter);

        let (_, shifted) = parse_binding("shift+k").unwrap();
        assert!(shifted.shift);
    }

    #[test]
    fn rejects_bad_bindings() {
        assert!(parse_binding("").is_err());
        assert!(parse_binding("leader").is_err());
        assert!(parse_binding("leader a b").is_err());
        assert!(parse_binding("meta+p").is_err());
        assert!(parse_binding("f99").is_err());
        assert!(parse_binding("ab").is_err());
    }

    #[test]
    fn defaults_resolve() {
        let km = Keymap::default();
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(Layer::Base, &ctrl_p, false),
            Some(Action::OpenCommandPalette)
        );
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(km.resolve(Layer::Base, &enter, false), Some(Action::Submit));
        // leader 后的裸键：非 leader 状态不触发
        let n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(km.resolve(Layer::Base, &n, false), None);
        assert_eq!(km.resolve(Layer::Base, &n, true), Some(Action::NewSession));
    }

    #[test]
    fn modal_layer_never_resolves_base_bindings() {
        let km = Keymap::default();
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(Layer::Modal, &ctrl_p, false), None);
    }

    #[test]
    fn user_override_replaces_default() {
        let overrides = vec![KeybindEntry {
            keys: "ctrl+k".into(),
            action: "command_palette".into(),
        }];
        let (km, warnings) = Keymap::new(&overrides);
        assert!(warnings.is_empty());
        let ctrl_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(Layer::Base, &ctrl_k, false),
            Some(Action::OpenCommandPalette)
        );
        // ctrl+p 默认仍在
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(Layer::Base, &ctrl_p, false),
            Some(Action::OpenCommandPalette)
        );
    }

    #[test]
    fn user_can_rebind_existing_trigger() {
        let overrides = vec![KeybindEntry {
            keys: "ctrl+p".into(),
            action: "toggle_sidebar".into(),
        }];
        let (km, _) = Keymap::new(&overrides);
        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(Layer::Base, &ctrl_p, false),
            Some(Action::ToggleSidebar)
        );
    }

    #[test]
    fn bad_entries_collect_warnings_not_panic() {
        let overrides = vec![
            KeybindEntry {
                keys: "bogus!!".into(),
                action: "quit".into(),
            },
            KeybindEntry {
                keys: "ctrl+k".into(),
                action: "no_such_action".into(),
            },
        ];
        let (km, warnings) = Keymap::new(&overrides);
        assert_eq!(warnings.len(), 2);
        let ctrl_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(Layer::Base, &ctrl_k, false), None);
    }

    #[test]
    fn leader_number_binds_switch_session() {
        let km = Keymap::default();
        let three = KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE);
        assert_eq!(
            km.resolve(Layer::Base, &three, true),
            Some(Action::SwitchSession(3))
        );
    }

    #[test]
    fn uppercase_char_normalizes_to_shift_lower() {
        let kp =
            KeyPress::from_event(&KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(kp.code, PressCode::Char('k'));
        assert!(kp.shift);
    }

    #[test]
    fn format_parse_roundtrip() {
        let samples = [
            (
                false,
                KeyPress {
                    code: PressCode::Char('p'),
                    ctrl: true,
                    alt: false,
                    shift: false,
                },
            ),
            (
                true,
                KeyPress {
                    code: PressCode::Char('n'),
                    ctrl: false,
                    alt: false,
                    shift: false,
                },
            ),
            (
                false,
                KeyPress {
                    code: PressCode::F(2),
                    ctrl: false,
                    alt: false,
                    shift: false,
                },
            ),
            (
                false,
                KeyPress {
                    code: PressCode::Enter,
                    ctrl: true,
                    alt: true,
                    shift: false,
                },
            ),
        ];
        for (leader, press) in samples {
            let text = format_binding(leader, press);
            let (l2, p2) = parse_binding(&text).unwrap();
            assert_eq!((l2, p2), (leader, press), "roundtrip {text}");
        }
    }

    #[test]
    fn keys_for_reflects_overrides() {
        let km = Keymap::default();
        assert!(km
            .keys_for(Action::OpenCommandPalette)
            .iter()
            .any(|k| k == "ctrl+p"));
        assert!(km
            .keys_for(Action::ToggleSidebar)
            .iter()
            .any(|k| k == "leader b"));

        let (km2, _) = Keymap::new(&[KeybindEntry {
            keys: "ctrl+k".into(),
            action: "command_palette".into(),
        }]);
        assert!(km2
            .keys_for(Action::OpenCommandPalette)
            .iter()
            .any(|k| k == "ctrl+k"));
    }

    #[test]
    fn all_action_ids_roundtrip() {
        // 保证 id/from_id 双向一致（SwitchSession 的 id 不可反查具体序号，单独处理）
        for a in [
            Action::NewSession,
            Action::Quit,
            Action::Submit,
            Action::ScrollUp,
            Action::FocusNextTool,
            Action::CycleModel,
            Action::CycleAgent,
            Action::ToggleWhichKey,
            Action::ToggleWhichKeyLayout,
            Action::WhichKeyGroupPrev,
            Action::WhichKeyGroupNext,
            Action::WhichKeyScrollUp,
            Action::WhichKeyScrollDown,
            Action::WhichKeyPageUp,
            Action::WhichKeyPageDown,
            Action::WhichKeyHome,
            Action::WhichKeyEnd,
        ] {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert_eq!(Action::from_id("switch_session"), None);
    }

    #[test]
    fn opencode_parity_defaults() {
        let km = Keymap::default();
        let has = |a: Action, k: &str| km.keys_for(a).iter().any(|x| x == k);
        // F2 循环模型、Tab 循环 agent
        assert!(has(Action::CycleModel, "f2"));
        assert!(has(Action::CycleModelReverse, "shift+f2"));
        assert!(has(Action::CycleAgent, "tab"));
        assert!(has(Action::CycleAgentReverse, "shift+tab"));
        // which-key 开关/布局/导航
        assert!(has(Action::ToggleWhichKey, "ctrl+alt+k"));
        assert!(has(Action::ToggleWhichKeyLayout, "ctrl+alt+shift+k"));
        assert!(has(Action::WhichKeyGroupPrev, "ctrl+alt+left"));
        assert!(has(Action::WhichKeyGroupNext, "ctrl+alt+right"));
        assert!(has(Action::WhichKeyScrollUp, "ctrl+alt+up"));
        assert!(has(Action::WhichKeyScrollDown, "ctrl+alt+down"));
        assert!(has(Action::WhichKeyPageUp, "ctrl+alt+pageup"));
        assert!(has(Action::WhichKeyPageDown, "ctrl+alt+pagedown"));
        assert!(has(Action::WhichKeyHome, "ctrl+alt+home"));
        assert!(has(Action::WhichKeyEnd, "ctrl+alt+end"));
        // 侧栏改由 leader 触发；工具卡聚焦无默认键（opencode 亦无）
        assert!(has(Action::ToggleSidebar, "leader b"));
        assert!(!km.keys_for(Action::ToggleSidebar).iter().any(|k| k == "f2"));
        assert!(km.keys_for(Action::FocusNextTool).is_empty());
        assert!(km.keys_for(Action::FocusPrevTool).is_empty());
    }

    #[test]
    fn default_bindings_have_no_duplicate_triggers() {
        let km = Keymap::default();
        let mut seen: Vec<String> = Vec::new();
        for a in [
            Action::CycleModel,
            Action::CycleModelReverse,
            Action::CycleAgent,
            Action::CycleAgentReverse,
            Action::ToggleWhichKey,
            Action::ToggleWhichKeyLayout,
            Action::NextSession,
            Action::PrevSession,
            Action::ScrollUp,
            Action::ScrollDown,
        ] {
            for k in km.keys_for(a) {
                assert!(!seen.contains(&k), "duplicate trigger: {k}");
                seen.push(k);
            }
        }
    }
}
