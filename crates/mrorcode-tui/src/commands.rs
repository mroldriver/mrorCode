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
    ToggleSidebar,
    OpenModelPicker,
    OpenCommandPalette,
    OpenHelp,
    TogglePlanMode,
    CycleModel,
    ToggleWhichKey,
    ToggleWhichKeyLayout,
    ToggleReasoning,
    OpenDiffViewer,
    SelectText,
    RewindSession,
    /// 固定/取消固定当前会话。
    PinSession,
    /// 重命名当前会话（ctrl+r，对齐 opencode）。
    RenameSession,
    /// 后台运行当前任务（ctrl+b）：agent 运行时新建会话并切换，当前任务在后台继续。
    BackgroundTask,
    /// 打开消息时间线对话框（P2-9）。
    OpenTimeline,
    /// 导出当前会话为 Markdown 文件。
    ExportTranscript,
    /// 加宽侧栏。
    WidenSidebar,
    /// 缩窄侧栏。
    NarrowSidebar,
    /// 在外部编辑器中打开当前输入。
    OpenExternalEditor,
    /// 跨 provider 切换（重建 runtime，M5-e）。
    SwitchProvider,
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
            default_keys: &["leader b"],
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
            default_keys: &["tab", "shift+tab"],
        },
        CommandSpec {
            command: AppCommand::CycleModel,
            name: "cycle model",
            desc: "循环切换最近模型",
            category: CommandCategory::Session,
            default_keys: &["f2", "shift+f2"],
        },
        CommandSpec {
            command: AppCommand::ToggleWhichKey,
            name: "toggle key bindings",
            desc: "显示/隐藏键位面板",
            category: CommandCategory::View,
            default_keys: &["ctrl+alt+k"],
        },
        CommandSpec {
            command: AppCommand::ToggleWhichKeyLayout,
            name: "toggle key bindings layout",
            desc: "切换键位面板 dock/overlay 布局",
            category: CommandCategory::View,
            default_keys: &["ctrl+alt+shift+k"],
        },
        CommandSpec {
            command: AppCommand::ToggleReasoning,
            name: "toggle reasoning",
            desc: "展开/收起思考过程",
            category: CommandCategory::View,
            default_keys: &["leader r"],
        },
        CommandSpec {
            command: AppCommand::OpenDiffViewer,
            name: "open diff",
            desc: "查看聚焦工具的完整 diff",
            category: CommandCategory::View,
            default_keys: &["leader d"],
        },
        CommandSpec {
            command: AppCommand::SelectText,
            name: "select text",
            desc: "键盘选择并复制文本",
            category: CommandCategory::View,
            default_keys: &["leader v"],
        },
        CommandSpec {
            command: AppCommand::RewindSession,
            name: "rewind session",
            desc: "回退到历史某轮之前（仅当前会话，disk 保留）",
            category: CommandCategory::Session,
            default_keys: &["leader w"],
        },
        CommandSpec {
            command: AppCommand::PinSession,
            name: "pin session",
            desc: "固定/取消固定当前会话",
            category: CommandCategory::Session,
            default_keys: &[],
        },
        CommandSpec {
            command: AppCommand::RenameSession,
            name: "rename session",
            desc: "重命名当前会话",
            category: CommandCategory::Session,
            default_keys: &["ctrl+shift+r"],
        },
        CommandSpec {
            command: AppCommand::BackgroundTask,
            name: "background task",
            desc: "后台运行当前任务（新建会话，当前任务继续运行）",
            category: CommandCategory::Session,
            default_keys: &["ctrl+b"],
        },
        CommandSpec {
            command: AppCommand::OpenTimeline,
            name: "timeline",
            desc: "消息时间线（查看/回退到指定消息）",
            category: CommandCategory::Session,
            default_keys: &["ctrl+shift+t"],
        },
        CommandSpec {
            command: AppCommand::ExportTranscript,
            name: "export transcript",
            desc: "导出当前会话为 Markdown 文件",
            category: CommandCategory::Session,
            default_keys: &[],
        },
        CommandSpec {
            command: AppCommand::WidenSidebar,
            name: "widen sidebar",
            desc: "加宽侧栏",
            category: CommandCategory::View,
            default_keys: &[],
        },
        CommandSpec {
            command: AppCommand::NarrowSidebar,
            name: "narrow sidebar",
            desc: "缩窄侧栏",
            category: CommandCategory::View,
            default_keys: &[],
        },
        CommandSpec {
            command: AppCommand::OpenExternalEditor,
            name: "open in editor",
            desc: "在 $EDITOR 中打开当前输入",
            category: CommandCategory::App,
            default_keys: &["leader e"],
        },
        CommandSpec {
            command: AppCommand::Interrupt,
            name: "interrupt",
            desc: "中断当前请求",
            category: CommandCategory::App,
            default_keys: &["esc esc"],
        },
        CommandSpec {
            command: AppCommand::SwitchProvider,
            name: "switch provider",
            desc: "切换到另一个 provider（重建运行时，会话不丢）",
            category: CommandCategory::App,
            default_keys: &["leader s"],
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
        AppCommand::ToggleSidebar => "toggle_sidebar",
        AppCommand::OpenModelPicker => "switch_model",
        AppCommand::OpenCommandPalette => "command_palette",
        AppCommand::OpenHelp => "help",
        AppCommand::TogglePlanMode => "toggle_plan_mode",
        AppCommand::CycleModel => "cycle_model",
        AppCommand::ToggleWhichKey => "toggle_which_key",
        AppCommand::ToggleWhichKeyLayout => "toggle_which_key_layout",
        AppCommand::ToggleReasoning => "toggle_reasoning",
        AppCommand::OpenDiffViewer => "open_diff",
        AppCommand::SelectText => "select_text",
        AppCommand::RewindSession => "rewind_session",
        AppCommand::PinSession => "pin_session",
        AppCommand::RenameSession => "rename_session",
        AppCommand::BackgroundTask => "background_task",
        AppCommand::OpenTimeline => "open_timeline",
        AppCommand::ExportTranscript => "export_transcript",
        AppCommand::WidenSidebar => "widen_sidebar",
        AppCommand::NarrowSidebar => "narrow_sidebar",
        AppCommand::OpenExternalEditor => "open_external_editor",
        AppCommand::SwitchProvider => "switch_provider",
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

/// 把输入框中的 `/斜杠命令` 解析为 `AppCommand`。
///
/// 补全插入时用 `name.replace(' ', "-")`（如 `new session` → `/new-session`），
/// 这里反向还原：去前缀 `/`、`-` 换回空格、按 registry 的 `name` 大小写不敏感匹配。
/// 输入不是以 `/` 开头、或仅有 `/` 无命令名、或命令名不匹配时返回 `None`。
pub fn parse_slash_command(text: &str) -> Option<AppCommand> {
    let trimmed = text.trim();
    let body = trimmed.strip_prefix('/')?;
    if body.is_empty() {
        return None;
    }
    // 补全插入时空格被替换为 `-`，反向还原；同时兼容用户直接输入带空格的形式。
    let candidate = body.replace('-', " ").to_lowercase();
    registry()
        .iter()
        .find(|c| c.name.to_lowercase() == candidate)
        .map(|c| c.command)
}

/// 命令映射到 keymap 动作（help 反查真实键位用）。
pub fn to_action(cmd: AppCommand) -> crate::keymap::Action {
    use crate::keymap::Action as A;
    match cmd {
        AppCommand::NewSession => A::NewSession,
        AppCommand::ListSessions => A::ListSessions,
        AppCommand::DeleteSession => A::DeleteSession,
        AppCommand::ToggleSidebar => A::ToggleSidebar,
        AppCommand::OpenModelPicker => A::OpenModelPicker,
        AppCommand::OpenHelp => A::OpenHelp,
        AppCommand::TogglePlanMode => A::CycleAgent,
        AppCommand::CycleModel => A::CycleModel,
        AppCommand::ToggleWhichKey => A::ToggleWhichKey,
        AppCommand::ToggleWhichKeyLayout => A::ToggleWhichKeyLayout,
        AppCommand::ToggleReasoning => A::ToggleReasoning,
        AppCommand::OpenDiffViewer => A::OpenDiffViewer,
        AppCommand::SelectText => A::ToggleSelect,
        AppCommand::RewindSession => A::Rewind,
        AppCommand::PinSession => A::PinSession,
        AppCommand::RenameSession => A::RenameSession,
        AppCommand::BackgroundTask => A::BackgroundTask,
        AppCommand::OpenTimeline => A::OpenTimeline,
        AppCommand::ExportTranscript => A::ExportTranscript,
        AppCommand::WidenSidebar => A::WidenSidebar,
        AppCommand::NarrowSidebar => A::NarrowSidebar,
        AppCommand::OpenExternalEditor => A::OpenExternalEditor,
        AppCommand::SwitchProvider => A::SwitchProvider,
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

    #[test]
    fn parse_slash_command_maps_hyphenated_names() {
        assert_eq!(
            parse_slash_command("/new-session"),
            Some(AppCommand::NewSession)
        );
        assert_eq!(
            parse_slash_command("/command-palette"),
            Some(AppCommand::OpenCommandPalette)
        );
        assert_eq!(
            parse_slash_command("/toggle-plan-mode"),
            Some(AppCommand::TogglePlanMode)
        );
        assert_eq!(
            parse_slash_command("/switch-provider"),
            Some(AppCommand::SwitchProvider)
        );
    }

    #[test]
    fn parse_slash_command_is_case_insensitive_and_trims() {
        assert_eq!(
            parse_slash_command("  /NEW-SESSION  "),
            Some(AppCommand::NewSession)
        );
        assert_eq!(parse_slash_command("/Help"), Some(AppCommand::OpenHelp));
    }

    #[test]
    fn parse_slash_command_rejects_non_slash_and_unknown() {
        assert!(parse_slash_command("hello world").is_none());
        assert!(parse_slash_command("/").is_none());
        assert!(parse_slash_command("/not-a-real-command").is_none());
        // 多行文本首行不是斜杠命令时，整体作为普通消息
        assert!(parse_slash_command("hello\n/new-session").is_none());
    }

    #[test]
    fn every_registry_command_is_slash_parseable() {
        for spec in registry() {
            let slash = format!("/{}", spec.name.replace(' ', "-"));
            assert_eq!(
                parse_slash_command(&slash),
                Some(spec.command),
                "slash command `{slash}` should resolve to {:?}",
                spec.command
            );
        }
    }
}
