//! 命令注册表：命令面板的唯一数据源。
//!
//! 每条命令是纯数据（AppCommand 枚举），按键接受后由 app 统一 apply，
//! 闭包不进注册表（可测、无生命周期纠缠）。真实默认键位串一并登记在此，
//! help 对话框从这里生成，杜绝手写键位表过期（M2-8 keymap 接管解析）。

use crate::widgets::dialog::{Dialog, SelectItem, SelectPurpose, SelectState};

/// 命令面板触发后要执行的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCommand {
    NewSession,
    ListSessions,
    DeleteSession,
    OpenThemePicker,
    ToggleSidebar,
    OpenModelPicker,
    OpenCommandPalette,
    OpenHelp,
    TogglePlanMode,
    Quit,
    Interrupt,
}

/// 命令元数据。
pub struct CommandSpec {
    pub command: AppCommand,
    /// 面板中显示的名称（小写英文，fuzzy 匹配对象）。
    pub name: &'static str,
    /// 中文说明（M2-11 help 对话框展示）。
    #[allow(dead_code)]
    pub desc: &'static str,
    /// 分组（Suggested 置顶）。
    pub category: CommandCategory,
    /// 默认键位串（紧凑格式，keymap M2-8 解析）；无直接绑定为空。
    pub default_keys: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandCategory {
    Suggested,
    Session,
    View,
    App,
}

impl CommandCategory {
    pub fn label(self) -> &'static str {
        match self {
            CommandCategory::Suggested => "Suggested",
            CommandCategory::Session => "Session",
            CommandCategory::View => "View",
            CommandCategory::App => "App",
        }
    }
}

/// M2 首批命令（顺序即面板分组/组内顺序；Suggested 必须排在最前）。
pub fn registry() -> &'static [CommandSpec] {
    &[
        CommandSpec {
            command: AppCommand::OpenCommandPalette,
            name: "command palette",
            desc: "打开命令面板",
            category: CommandCategory::Suggested,
            default_keys: &["ctrl+p", "leader p"],
        },
        CommandSpec {
            command: AppCommand::NewSession,
            name: "new session",
            desc: "新建会话",
            category: CommandCategory::Suggested,
            default_keys: &["ctrl+n", "leader n"],
        },
        CommandSpec {
            command: AppCommand::ListSessions,
            name: "list sessions",
            desc: "会话列表",
            category: CommandCategory::Suggested,
            default_keys: &["ctrl+l", "leader l"],
        },
        CommandSpec {
            command: AppCommand::OpenThemePicker,
            name: "switch theme",
            desc: "切换主题",
            category: CommandCategory::Suggested,
            default_keys: &["leader t"],
        },
        CommandSpec {
            command: AppCommand::OpenModelPicker,
            name: "switch model",
            desc: "切换模型",
            category: CommandCategory::Session,
            default_keys: &["leader m"],
        },
        CommandSpec {
            command: AppCommand::DeleteSession,
            name: "delete session",
            desc: "删除当前会话",
            category: CommandCategory::Session,
            default_keys: &[],
        },
        CommandSpec {
            command: AppCommand::ToggleSidebar,
            name: "toggle sidebar",
            desc: "切换侧栏",
            category: CommandCategory::View,
            default_keys: &["f2", "leader b"],
        },
        CommandSpec {
            command: AppCommand::OpenHelp,
            name: "help",
            desc: "键位帮助",
            category: CommandCategory::App,
            default_keys: &[],
        },
        CommandSpec {
            command: AppCommand::TogglePlanMode,
            name: "toggle plan mode",
            desc: "切换 plan 模式（只读）",
            category: CommandCategory::App,
            default_keys: &["shift+tab"],
        },
        CommandSpec {
            command: AppCommand::Interrupt,
            name: "interrupt",
            desc: "中断当前请求",
            category: CommandCategory::App,
            default_keys: &["esc esc"],
        },
        CommandSpec {
            command: AppCommand::Quit,
            name: "quit",
            desc: "退出",
            category: CommandCategory::App,
            default_keys: &["ctrl+c"],
        },
    ]
}

/// 稳定字符串 id（Select 回传值）。
pub fn command_id(cmd: AppCommand) -> &'static str {
    match cmd {
        AppCommand::NewSession => "new_session",
        AppCommand::ListSessions => "list_sessions",
        AppCommand::DeleteSession => "delete_session",
        AppCommand::OpenThemePicker => "switch_theme",
        AppCommand::ToggleSidebar => "toggle_sidebar",
        AppCommand::OpenModelPicker => "switch_model",
        AppCommand::OpenCommandPalette => "command_palette",
        AppCommand::OpenHelp => "help",
        AppCommand::TogglePlanMode => "toggle_plan_mode",
        AppCommand::Quit => "quit",
        AppCommand::Interrupt => "interrupt",
    }
}

/// id 反查命令。
pub fn command_by_id(id: &str) -> Option<AppCommand> {
    registry()
        .iter()
        .find_map(|c| (command_id(c.command) == id).then_some(c.command))
}

/// 命令映射到 keymap 动作（help 反查真实键位用）。
pub fn to_action(cmd: AppCommand) -> crate::keymap::Action {
    use crate::keymap::Action as A;
    match cmd {
        AppCommand::NewSession => A::NewSession,
        AppCommand::ListSessions => A::ListSessions,
        AppCommand::DeleteSession => A::DeleteSession,
        AppCommand::OpenThemePicker => A::OpenThemePicker,
        AppCommand::ToggleSidebar => A::ToggleSidebar,
        AppCommand::OpenModelPicker => A::OpenModelPicker,
        AppCommand::OpenHelp => A::OpenHelp,
        AppCommand::TogglePlanMode => A::TogglePlanMode,
        AppCommand::Quit => A::Quit,
        AppCommand::Interrupt => A::Interrupt,
        AppCommand::OpenCommandPalette => A::OpenCommandPalette,
    }
}

/// 构造命令面板对话框。
pub fn palette_dialog() -> Dialog {
    let items: Vec<SelectItem> = registry()
        .iter()
        .map(|c| SelectItem {
            label: c.name.to_string(),
            hint: c.default_keys.first().copied().unwrap_or("").to_string(),
            group: c.category.label().to_string(),
            value: command_id(c.command).to_string(),
        })
        .collect();
    Dialog::Select(SelectState::new(
        "command palette",
        SelectPurpose::Command,
        items,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_roundtrip_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in registry() {
            let id = command_id(c.command);
            assert_eq!(command_by_id(id), Some(c.command));
            assert!(seen.insert(id), "duplicate command id: {id}");
        }
    }

    #[test]
    fn suggested_group_is_first() {
        assert_eq!(registry()[0].category, CommandCategory::Suggested);
    }

    #[test]
    fn every_command_has_a_name() {
        assert!(registry().iter().all(|c| !c.name.is_empty()));
    }

    #[test]
    fn palette_items_carry_ids_and_keybind_hints() {
        let dialog = palette_dialog();
        if let Dialog::Select(sel) = dialog {
            assert!(sel.len() >= 9);
        } else {
            panic!("palette must be a Select dialog");
        }
    }

    #[test]
    fn unknown_id_returns_none() {
        assert!(command_by_id("nope").is_none());
    }
}
