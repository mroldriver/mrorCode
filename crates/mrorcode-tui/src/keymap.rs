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
    OpenThemePicker,
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
}

impl Action {
    /// KV 中使用的稳定 snake_case 标识。
    pub fn id(self) -> &'static str {
        match self {
            Action::NewSession => "new_session",
            Action::ListSessions => "list_sessions",
            Action::DeleteSession => "delete_session",
            Action::OpenThemePicker => "switch_theme",
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
        }
    }

    fn from_id(id: &str) -> Option<Action> {
        Some(match id {
            "new_session" => Action::NewSession,
            "list_sessions" => Action::ListSessions,
            "delete_session" => Action::DeleteSession,
            "switch_theme" => Action::OpenThemePicker,
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
        b(false, key(PressCode::Esc), Action::CancelOrClear),
        b(false, ctrl(PressCode::Char('c')), Action::Quit),
        b(
            false,
            ctrl(PressCode::Char('p')),
            Action::OpenCommandPalette,
        ),
        b(false, ctrl(PressCode::Char('n')), Action::NewSession),
        b(false, ctrl(PressCode::Char('l')), Action::ListSessions),
        b(false, key(PressCode::F(2)), Action::ToggleSidebar),
        b(false, key(PressCode::PageUp), Action::ScrollUp),
        b(false, key(PressCode::PageDown), Action::ScrollDown),
        b(false, key(PressCode::Up), Action::HistoryPrev),
        b(false, key(PressCode::Down), Action::HistoryNext),
        b(false, key(PressCode::Tab), Action::FocusNextTool),
        b(
            false,
            KeyPress {
                code: PressCode::Tab,
                ctrl: false,
                alt: false,
                shift: true,
            },
            Action::FocusPrevTool,
        ),
    ];

    let leaders = vec![
        leader(PressCode::Char('p')),
        leader(PressCode::Char('n')),
        leader(PressCode::Char('l')),
        leader(PressCode::Char('b')),
        leader(PressCode::Char('t')),
        leader(PressCode::Char('m')),
    ];
    let leader_actions = [
        Action::OpenCommandPalette,
        Action::NewSession,
        Action::ListSessions,
        Action::ToggleSidebar,
        Action::OpenThemePicker,
        Action::OpenModelPicker,
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
        assert!(km.keys_for(Action::ToggleSidebar).iter().any(|k| k == "f2"));
        assert!(km
            .keys_for(Action::OpenThemePicker)
            .iter()
            .any(|k| k == "leader t"));

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
        ] {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert_eq!(Action::from_id("switch_session"), None);
    }
}
