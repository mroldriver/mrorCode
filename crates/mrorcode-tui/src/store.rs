//! Elm 式状态层：权威 agent 状态只在 runtime，这里只存投影与纯 UI 态。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tui_textarea::TextArea;

use mrorcode_agent::{PermissionMode, RuntimeHandle, SessionStore};
use mrorcode_kernel::event::AgentEvent;
use mrorcode_kernel::message::{Conversation, Message, ToolCall};
use mrorcode_kernel::session::{SessionRecord, SessionSummary};

use crate::commands;
use crate::keymap::Keymap;
use crate::storage::Store;
use crate::theme::{Mode, Theme, ThemeSet};
use crate::widgets::dialog::{
    AlertState, ConfirmAction, ConfirmState, Dialog, HelpRow, HelpState, InputPurpose, InputState,
    SelectItem, SelectPurpose, SelectState, TimelineState,
};
use crate::widgets::markdown::MarkdownView;
use crate::widgets::toast::ToastLevel;
use crate::widgets::tool_card;

/// 侧栏会话列表条目（进程内；磁盘持久化归 M3）。
pub struct SessionMeta {
    pub id: u64,
    pub title: String,
    pub phase: Phase,
    /// 固定会话：pin 后排在侧栏最前（对齐 opencode 的 session pin）。
    pub pinned: bool,
}

/// 首个会话的固定 id；后续由 next_session_id 递增。
pub const DEFAULT_SESSION: u64 = 1;

/// 最近使用模型列表上限（对齐 opencode 的 recent 列表）。
pub const RECENT_MODELS_MAX: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Home,
    Session,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Working,
    WaitingApproval,
}

impl Phase {
    pub fn is_busy(self) -> bool {
        matches!(self, Phase::Working | Phase::WaitingApproval)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolStatus {
    Running,
    /// M2 审批前置态（工具创建于 ToolStarted，目前未使用）
    #[allow(dead_code)]
    Waiting,
    Ok,
    Failed,
}

pub struct ToolView {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub status: ToolStatus,
    pub output: String,
    /// 子 agent 实时文本投影（task 工具专用，流式追加）。
    pub sub_agent_text: String,
    /// 子 agent 工具调用记录（task 工具专用，用于显示子 agent 的活动）。
    pub sub_agent_tools: Vec<SubAgentToolView>,
}

/// 子 agent 内部工具调用的投影记录。
#[derive(Clone, Debug)]
pub struct SubAgentToolView {
    pub name: String,
    pub status: ToolStatus,
}

/// 可展开工具在消息列表中的定位（Tab 焦点遍历用）。
pub struct ToolEntry {
    pub msg: usize,
    pub tool: usize,
    pub id: String,
}

pub struct MessageView {
    /// 进程内稳定 id（M4）：投影按位置寻址会因切会话/流式追加而变，
    /// 展开态、rewind 边界、选区锚点都按 uid 记忆。
    pub uid: u64,
    pub role: Role,
    pub text: String,
    pub doc: MarkdownView,
    pub reasoning: String,
    /// 思考过程的增量 markdown 视图（M4-1，与正文同款缓存）。
    pub reasoning_doc: MarkdownView,
    pub tools: Vec<ToolView>,
    pub is_error: bool,
    pub streaming: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone)]
pub struct PermissionRequest {
    pub id: u64,
    pub tool: String,
    pub arguments: String,
}

pub struct AppState {
    pub route: Route,
    pub phase: Phase,
    pub should_quit: bool,
    pub model: String,
    /// 同 provider 下可选模型（config `models = []`，空则只有当前模型）。
    pub available_models: Vec<String>,
    /// 当前 provider 条目名（M5-e）。
    pub current_provider: String,
    /// 完整配置快照（provider 选择器 + 重建 runtime 用，M5-e）。
    pub config: mrorcode_config::Config,
    /// 装配选项（重建 runtime 时复用，M5-e）。
    pub assemble_options: mrorcode_agent::AssembleOptions,
    /// 已选定待执行的 provider 切换（在事件循环顶部处理，M5-e）。
    pub pending_provider_switch: Option<String>,
    /// 待打开外部编辑器（在事件循环顶部处理，需先恢复终端）。
    pub pending_external_editor: bool,
    pub working_dir: PathBuf,
    pub sidebar: bool,
    /// 侧栏宽度（列数）。
    pub sidebar_width: u16,

    pub messages: Vec<MessageView>,
    /// 进程内单调消息 id 分配器（M4：投影按 uid 记忆展开态等）。
    pub next_message_uid: u64,
    /// 会话元数据列表（新建顺序即侧栏顺序）。
    pub sessions: Vec<SessionMeta>,
    /// 当前活动会话 id（kernel 全链路一致）。
    pub active_session: u64,
    next_session_id: u64,
    /// 非活动会话的消息投影（切回即恢复；M2 仅进程内）。
    pub buckets: HashMap<u64, Vec<MessageView>>,
    /// 每个会话的输入草稿（切会话时暂存/恢复）。
    drafts: HashMap<u64, String>,
    pub prompt: TextArea<'static>,
    /// 已提交历史（M1 内存）。
    pub history: Vec<String>,
    pub hist_idx: Option<usize>,
    /// 提交前的草稿（翻历史时暂存）。
    pub draft: String,

    pub permission: Option<PermissionRequest>,
    /// 权限弹窗当前高亮的选项（0=本次允许 / 1=始终允许 / 2=拒绝）。
    pub permission_cursor: usize,
    /// 对话框栈（栈顶吃键；M2-6 起 Select/Confirm/Help 均入栈）。
    pub dialogs: Vec<Dialog>,
    /// 粘底滚动；上翻后脱离。
    pub stick_bottom: bool,
    /// 像素行偏移（从底部计的回看行数）。
    pub backscroll: u16,
    /// 上一帧布局总行数（用于滚动边界）。
    pub last_total_lines: u16,
    pub viewport_lines: u16,

    /// Esc 双击中断的第一次时刻。
    pub esc_first: Option<Instant>,
    /// 单条 toast（对齐 opencode：新 toast 替换当前，5s 过期）。
    pub current_toast: Option<(String, Instant, crate::widgets::toast::ToastLevel)>,

    /// which-key 面板是否启用 / 布局（对齐 opencode，默认关闭 dock）。
    pub which_key_on: bool,
    pub which_key_layout: crate::widgets::whichkey::WkLayout,
    /// which-key 当前分组页签与组内滚动偏移（不持久化）。
    pub which_key: crate::widgets::whichkey::WkState,

    /// 已展开工具卡（按 tool id）；长输出默认折叠。
    pub expanded_tools: HashSet<String>,
    /// 已展开的思考过程块（按消息 uid，M4-1）。
    pub expanded_reasoning: HashSet<u64>,
    /// 最近一次切换的思考块（高亮反馈，M4-1）。
    pub focused_reasoning: Option<u64>,
    /// 当前聚焦工具卡 id（Tab 循环，Enter 展开/收起）。
    pub focused_tool: Option<String>,

    /// spinner 帧计数。
    pub spinner_ticks: u32,
    /// 输入光标当前处于亮帧（应用层驱动闪烁，空闲也翻转）。
    pub cursor_blink_on: bool,

    /// 键位表（默认 + KV 用户覆盖）。
    pub keymap: Keymap,
    /// leader 等待窗口截止时刻。
    pub leader_until: Option<Instant>,

    /// 权限模式（只读 / 工作区读写 / 完全访问）；底栏徽标 + 提交时告知 kernel。
    pub mode: PermissionMode,

    /// `@`/`/` 补全浮层状态（None 表示未激活）。
    pub completion: Option<crate::input::Completion>,

    /// 选区（M4-6）；键盘 `v` 进入。
    pub selection: Option<crate::selection::Selection>,
    /// 键盘选择模式（modal，M4-6）：方向键扩展选区、y 复制、esc 取消。
    pub select_mode: bool,
    /// 上一帧的屏幕文本行（选区提取用，M4-6）。
    pub screen_rows: Vec<String>,
    /// 最近一帧的屏幕区域（which-key 面板几何换算用）。
    pub screen: ratatui::layout::Rect,

    /// 累计 token 用量（M4-8 Context 分区）。
    pub usage_input_tokens: u64,
    pub usage_output_tokens: u64,
    /// 是否发生过上下文压缩（投影里出现 `[compacted` 标记，M4-8）。
    pub compacted: bool,
    /// 终端窗口是否获得焦点（用于失焦时通知）。
    pub terminal_focused: bool,
    /// 读/写/编辑过的文件路径（最近优先、去重、上限 8，M4-8 Files 分区）。
    pub touched_files: Vec<String>,
    /// 侧栏折叠的分区标题（M4-8）。
    pub collapsed_sections: std::collections::HashSet<String>,

    /// 磁盘会话存储（None 表示 home 不可用，仅进程内）。
    pub session_store: Option<SessionStore>,
    /// 磁盘历史会话摘要（侧栏展示）。
    pub history_sessions: Vec<SessionSummary>,

    /// 本地偏好（主题/历史/键位等）。
    pub kv: Store,

    /// 当前生效主题（内置 opencode，固定暗色；不再支持运行时切换）。
    pub theme: Theme,
    /// 主题代际：markdown 高亮缓存失效信号（当前恒为 0，保留以便将来扩展）。
    pub theme_gen: u64,

    /// 已连接 MCP server 名列表（启动时从 Assembly 提取，仅用于展示）。
    pub mcp_servers: Vec<String>,
    /// 当前工作目录的 git 分支名（非 git 仓库时为 None）。
    pub git_branch: Option<String>,
}

impl AppState {
    pub fn new(
        model: String,
        available_models: Vec<String>,
        working_dir: PathBuf,
        kv: Store,
    ) -> Self {
        let sidebar = kv.data.sidebar.unwrap_or(true);
        let sidebar_width = kv.data.sidebar_width.unwrap_or(42).clamp(20, 80);
        let which_key_on = kv.data.which_key.unwrap_or(false);
        let which_key_layout = match kv.data.which_key_layout.as_deref() {
            Some("overlay") => crate::widgets::whichkey::WkLayout::Overlay,
            _ => crate::widgets::whichkey::WkLayout::Dock,
        };
        let history = kv.data.prompt_history.clone();
        // 内置 opencode 暗色主题（唯一；不再从 KV 恢复主题名/模式）
        let theme = ThemeSet::bundled(Mode::Dark)
            .get("opencode")
            .cloned()
            .unwrap_or_else(Theme::opencode);
        let model = match &kv.data.model {
            Some(remembered) if available_models.iter().any(|m| m == remembered) => {
                remembered.clone()
            }
            _ => model,
        };

        let mut prompt = TextArea::new(vec![String::new()]);
        prompt.set_cursor_line_style(ratatui::style::Style::default());

        let (keymap, bind_warnings) = Keymap::new(&kv.data.keybinds);
        for w in bind_warnings {
            tracing::warn!("keymap: {w}");
        }

        Self {
            route: Route::Home,
            phase: Phase::Idle,
            should_quit: false,
            model,
            available_models,
            current_provider: String::new(),
            config: mrorcode_config::Config::default(),
            assemble_options: mrorcode_agent::AssembleOptions::default(),
            pending_provider_switch: None,
            pending_external_editor: false,
            working_dir,
            sidebar,
            sidebar_width,
            messages: Vec::new(),
            next_message_uid: 1,
            sessions: vec![SessionMeta {
                id: DEFAULT_SESSION,
                title: "新会话".to_string(),
                phase: Phase::Idle,
                pinned: false,
            }],
            active_session: DEFAULT_SESSION,
            next_session_id: DEFAULT_SESSION + 1,
            buckets: HashMap::new(),
            drafts: HashMap::new(),
            prompt,
            history,
            hist_idx: None,
            draft: String::new(),
            permission: None,
            permission_cursor: 0,
            dialogs: Vec::new(),
            stick_bottom: true,
            backscroll: 0,
            last_total_lines: 0,
            viewport_lines: 0,
            esc_first: None,
            current_toast: None,
            which_key_on,
            which_key_layout,
            which_key: Default::default(),
            expanded_tools: HashSet::new(),
            expanded_reasoning: HashSet::new(),
            focused_reasoning: None,
            focused_tool: None,
            spinner_ticks: 0,
            cursor_blink_on: true,
            keymap,
            leader_until: None,
            mode: PermissionMode::default(),
            completion: None,
            selection: None,
            select_mode: false,
            screen_rows: Vec::new(),
            screen: ratatui::layout::Rect::new(0, 0, 0, 0),
            usage_input_tokens: 0,
            usage_output_tokens: 0,
            compacted: false,
            terminal_focused: true,
            touched_files: Vec::new(),
            collapsed_sections: HashSet::new(),
            session_store: None,
            history_sessions: Vec::new(),
            kv,
            theme,
            theme_gen: 0,
            mcp_servers: Vec::new(),
            git_branch: None,
        }
    }

    /// 切换侧栏（M4：侧栏开关）。
    pub fn toggle_sidebar(&mut self) {
        self.sidebar = !self.sidebar;
        self.kv.data.sidebar = Some(self.sidebar);
        let _ = self.kv.save();
    }

    /// 调整侧栏宽度（delta 列），持久化。
    pub fn adjust_sidebar_width(&mut self, delta: i16) {
        let new = (self.sidebar_width as i16 + delta).clamp(20, 80) as u16;
        self.sidebar_width = new;
        self.kv.data.sidebar_width = Some(new);
        let _ = self.kv.save();
    }

    /// 循环切换权限模式并通知 kernel（只读 → 工作区读写 → 完全访问）。
    pub fn cycle_mode(&mut self, reverse: bool, handle: &RuntimeHandle) {
        let next = if reverse {
            // 反向：工作区读写 → 只读，完全访问 → 工作区读写，只读 → 完全访问
            match self.mode {
                PermissionMode::ReadOnly => PermissionMode::Full,
                PermissionMode::Workspace => PermissionMode::ReadOnly,
                PermissionMode::Full => PermissionMode::Workspace,
            }
        } else {
            self.mode.next()
        };
        self.set_mode(next, handle);
    }

    /// 直接设置权限模式并通知 kernel（custom 命令 / provider 切换重发用）。
    pub fn set_mode(&mut self, mode: PermissionMode, handle: &RuntimeHandle) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        handle.set_mode(mode);
        self.toast(match mode {
            PermissionMode::ReadOnly => "只读模式：仅只读工具，写操作将被拒绝",
            PermissionMode::Workspace => "工作区读写：工作区写直行，其他写操作需审批",
            PermissionMode::Full => "完全访问：所有工具直接执行，不再弹审批",
        });
    }

    // ---------- 输入补全（@文件 / /命令） ----------

    /// 依据当前光标位置刷新补全浮层；无触发则清空。
    pub fn refresh_completion(&mut self) {
        let (row, col) = self.prompt.cursor();
        let line = self
            .prompt
            .lines()
            .get(row)
            .map(String::as_str)
            .unwrap_or("");
        match crate::input::detect(line, col) {
            Some((kind, start_col, query)) => {
                let items = crate::input::build_items(kind, &query, &self.working_dir);
                if items.is_empty() {
                    self.completion = None;
                } else {
                    self.completion = Some(crate::input::Completion {
                        kind,
                        row,
                        start_col,
                        end_col: col,
                        query,
                        items,
                        selected: 0,
                    });
                }
            }
            None => self.completion = None,
        }
    }

    pub fn clear_completion(&mut self) {
        self.completion = None;
    }

    /// 接受当前选中项：替换 token 段 + 设置 atomic range（整体删除）。
    pub fn accept_completion(&mut self) -> bool {
        let Some(comp) = self.completion.take() else {
            return false;
        };
        let Some(item) = comp.current().cloned() else {
            return false;
        };
        // 定位 token 段：从 start_col 到 end_col（以字符计）
        let (row, _) = self.prompt.cursor();
        let line = self.prompt.lines().get(row).cloned().unwrap_or_default();
        let chars: Vec<char> = line.chars().collect();
        if row != comp.row || comp.start_col > chars.len() || comp.end_col > chars.len() {
            return false;
        }
        let before: String = chars[..comp.start_col].iter().collect();
        let after: String = chars[comp.end_col..].iter().collect();
        let new_line = format!("{before}{}{after}", item.insert);
        let new_col = chars[..comp.start_col].len() + item.insert.chars().count();

        // 用整行替换当前行
        replace_current_line(&mut self.prompt, row, &new_line);
        // 光标移到插入文本末尾
        set_cursor(&mut self.prompt, row, new_col);
        // 虚拟片段：整段 insert 作为 atomic range（退格整体删除）
        let start_col = chars[..comp.start_col].len();
        let _ = self
            .prompt
            .try_set_atomic_ranges([tui_textarea::AtomicRange {
                row,
                start_col,
                end_col: start_col + item.insert.chars().count(),
            }]);
        self.refresh_completion();
        true
    }

    pub fn push_dialog(&mut self, dialog: Dialog) {
        self.dialogs.push(dialog);
    }

    pub fn pop_dialog(&mut self) {
        self.dialogs.pop();
    }

    /// 打开命令面板（base 层才允许，避免叠加）。
    pub fn open_command_palette(&mut self) {
        if self.dialogs.is_empty() {
            self.push_dialog(commands::palette_dialog());
        }
    }

    /// 打开模型选择器（至少列出当前模型）。
    pub fn open_model_picker(&mut self) {
        let mut models = self.available_models.clone();
        if !models.iter().any(|m| m == &self.model) {
            models.insert(0, self.model.clone());
        }
        let items = models
            .into_iter()
            .map(|m| SelectItem {
                hint: if m == self.model {
                    "active".to_string()
                } else {
                    String::new()
                },
                group: "Models".to_string(),
                label: m.clone(),
                value: m,
            })
            .collect();
        self.push_dialog(Dialog::Select(SelectState::new(
            "switch model",
            SelectPurpose::Model,
            items,
        )));
    }

    /// 切换模型：更新本地徽标 + KV 持久化；kernel 在下一轮生效。
    pub fn set_model(&mut self, name: &str) -> bool {
        let known = self.available_models.iter().any(|m| m == name) || name == self.model;
        if !known {
            return false;
        }
        if name != self.model {
            self.model = name.to_string();
        }
        self.kv.data.model = Some(name.to_string());
        self.record_model_use(name);
        let _ = self.kv.save();
        true
    }

    /// 注入完整配置与装配选项（M5-e；启动时调用一次）。
    pub fn attach_config(
        &mut self,
        config: mrorcode_config::Config,
        options: mrorcode_agent::AssembleOptions,
        current_provider: String,
    ) {
        self.config = config;
        self.assemble_options = options;
        self.current_provider = current_provider;
    }

    /// 打开 provider 选择器：列出配置中的全部条目（M5-e）。
    pub fn open_provider_picker(&mut self) -> bool {
        if self.phase.is_busy() {
            return false;
        }
        // providers 为 HashMap，按名称排序保证展示稳定。
        let mut names: Vec<String> = self.config.providers.keys().cloned().collect();
        names.sort();
        if names.len() < 2 {
            return false;
        }
        let items = names
            .into_iter()
            .map(|n| SelectItem {
                hint: if n == self.current_provider {
                    "active".to_string()
                } else {
                    String::new()
                },
                group: "Providers".to_string(),
                label: n.clone(),
                value: n,
            })
            .collect();
        self.push_dialog(Dialog::Select(SelectState::new(
            "switch provider",
            SelectPurpose::Provider,
            items,
        )));
        true
    }

    /// 取当前活动会话的会话体（provider 切换后注入新 runtime 用，M5-e）。
    pub fn active_conversation(&self) -> Conversation {
        conversation_from_view(&self.messages)
    }

    /// 切换完成后更新 provider/模型相关投影（M5-e）。
    pub fn note_provider_switched(
        &mut self,
        provider: String,
        model: String,
        available_models: Vec<String>,
    ) {
        self.current_provider = provider;
        self.model = model.clone();
        self.available_models = available_models;
        self.kv.data.model = Some(model);
        let _ = self.kv.save();
    }

    /// 把模型记到最近使用列表首位（对齐 opencode `model.json` 的 `recent`）。
    fn record_model_use(&mut self, name: &str) {
        let mut recent: Vec<String> = self
            .kv
            .data
            .recent_models
            .take()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m != name)
            .collect();
        recent.insert(0, name.to_string());
        recent.truncate(RECENT_MODELS_MAX);
        self.kv.data.recent_models = Some(recent);
    }

    /// 最近使用模型列表（对齐 opencode F2 的循环范围）。
    /// 优先取持久化的最近列表，未记录的可用模型按 `available_models` 顺序补在后面。
    pub fn recent_models(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for m in self
            .kv
            .data
            .recent_models
            .iter()
            .flatten()
            .chain(self.available_models.iter())
        {
            if self.available_models.iter().any(|a| a == m) && !out.contains(m) {
                out.push(m.clone());
            }
        }
        out
    }

    /// 显示一条信息 toast（新 toast 替换当前，对齐 opencode）。
    pub fn toast(&mut self, text: impl Into<String>) {
        self.show_toast(text.into(), ToastLevel::Info);
    }

    /// 显示一条警告 toast。
    pub fn toast_warn(&mut self, text: impl Into<String>) {
        self.show_toast(text.into(), ToastLevel::Warn);
    }

    /// 显示一条错误 toast。
    pub fn toast_error(&mut self, text: impl Into<String>) {
        self.show_toast(text.into(), ToastLevel::Error);
    }

    fn show_toast(&mut self, text: String, level: ToastLevel) {
        self.current_toast = Some((text, Instant::now(), level));
    }

    /// 清理过期 toast（由慢节拍调用，5s 过期）。
    pub fn gc_toasts(&mut self) {
        let fresh = self
            .current_toast
            .as_ref()
            .is_some_and(|(_, t, _)| t.elapsed() < Duration::from_secs(5));
        if !fresh {
            self.current_toast = None;
        }
    }

    pub fn has_active_toasts(&self) -> bool {
        self.current_toast
            .as_ref()
            .is_some_and(|(_, t, _)| t.elapsed() < Duration::from_secs(5))
    }

    /// 当前帧应显示的 toast（最多一条）。
    pub fn active_toast(&self) -> Option<(ToastLevel, &str)> {
        self.current_toast
            .as_ref()
            .filter(|(_, t, _)| t.elapsed() < Duration::from_secs(5))
            .map(|(s, _, l)| (*l, s.as_str()))
    }

    // ---------- 工具卡折叠/焦点 ----------

    /// 可展开工具（输出超过 1 行）的平面列表。
    pub fn tool_entries(&self) -> Vec<ToolEntry> {
        let mut out = Vec::new();
        for (mi, m) in self.messages.iter().enumerate() {
            for (ti, t) in m.tools.iter().enumerate() {
                if tool_card::is_expandable(t) {
                    out.push(ToolEntry {
                        msg: mi,
                        tool: ti,
                        id: t.id.clone(),
                    });
                }
            }
        }
        out
    }

    /// Tab/Shift+Tab 在可展开工具间移动焦点。
    pub fn cycle_tool_focus(&mut self, next: bool) {
        let entries = self.tool_entries();
        if entries.is_empty() {
            self.focused_tool = None;
            return;
        }
        let len = entries.len();
        let pos = entries
            .iter()
            .position(|e| Some(&e.id) == self.focused_tool.as_ref());
        let idx = match (pos, next) {
            (Some(i), true) => (i + 1) % len,
            (Some(i), false) => (i + len - 1) % len,
            (None, true) => 0,
            (None, false) => len - 1,
        };
        self.focused_tool = Some(entries[idx].id.clone());
    }

    pub fn clear_tool_focus(&mut self) {
        self.focused_tool = None;
    }

    /// 展开/收起当前聚焦工具；无焦点返回 false（按键交给后续逻辑）。
    pub fn toggle_focused_tool(&mut self) -> bool {
        let Some(id) = self.focused_tool.clone() else {
            return false;
        };
        if self.expanded_tools.contains(&id) {
            self.expanded_tools.remove(&id);
        } else {
            self.expanded_tools.insert(id);
        }
        true
    }

    pub fn is_tool_expanded(&self, id: &str) -> bool {
        self.expanded_tools.contains(id)
    }

    // ---------- 模型循环（对齐 opencode F2） ----------

    /// 循环到最近使用模型列表中的上一个/下一个；kernel 下一轮生效。
    pub fn cycle_model(&mut self, reverse: bool, handle: &RuntimeHandle) {
        let models = self.recent_models();
        if models.len() < 2 {
            self.toast("只有一个可用模型");
            return;
        }
        let pos = models.iter().position(|m| m == &self.model);
        let len = models.len();
        let idx = match pos {
            Some(i) => {
                if reverse {
                    (i + len - 1) % len
                } else {
                    (i + 1) % len
                }
            }
            None => {
                if reverse {
                    len - 1
                } else {
                    0
                }
            }
        };
        let name = models[idx].clone();
        self.model = name.clone();
        self.kv.data.model = Some(name.clone());
        self.record_model_use(&name);
        let _ = self.kv.save();
        handle.set_model(name.clone());
        self.toast(format!("模型：{name}（下一轮生效）"));
    }

    // ---------- agent 循环（对齐 opencode Tab：权限模式三档循环） ----------

    /// Tab/Shift+Tab 循环切换权限模式并通知 kernel。
    pub fn cycle_agent(&mut self, reverse: bool, handle: &RuntimeHandle) {
        self.cycle_mode(reverse, handle);
    }

    // ---------- which-key 开关（对齐 opencode） ----------

    /// 开关 which-key 面板并持久化；关闭时重置滚动位置。
    pub fn toggle_which_key(&mut self) {
        self.which_key_on = !self.which_key_on;
        if !self.which_key_on {
            self.which_key.offset = 0;
        }
        self.kv.data.which_key = Some(self.which_key_on);
        let _ = self.kv.save();
    }

    /// 在 dock/overlay 布局间切换并持久化。
    pub fn toggle_which_key_layout(&mut self) {
        self.which_key_layout = match self.which_key_layout {
            crate::widgets::whichkey::WkLayout::Dock => crate::widgets::whichkey::WkLayout::Overlay,
            crate::widgets::whichkey::WkLayout::Overlay => crate::widgets::whichkey::WkLayout::Dock,
        };
        let label = match self.which_key_layout {
            crate::widgets::whichkey::WkLayout::Dock => "dock",
            crate::widgets::whichkey::WkLayout::Overlay => "overlay",
        };
        self.kv.data.which_key_layout = Some(label.to_string());
        let _ = self.kv.save();
    }

    /// which-key 面板是否显示：开关打开即常驻（对齐 opencode 的 pinned 语义，
    /// 不随 leader 窗口消失）。
    pub fn which_key_visible(&self) -> bool {
        self.which_key_on
    }

    /// 切换分组页签（`delta` 为 -1/1），切组后回到组首。
    pub fn which_key_group_delta(&mut self, delta: i32) {
        use crate::widgets::whichkey as wk;
        let n = wk::group_labels(&wk::leader_items(&self.keymap)).len();
        if n == 0 {
            return;
        }
        let n = n as i32;
        let cur = self.which_key.group.unwrap_or_default() as i32;
        let next = ((cur + delta) % n + n) % n;
        self.which_key.group = Some(next as usize);
        self.which_key.offset = 0;
    }

    /// 组内滚动（`delta` 为行数差，负数向上）。
    pub fn which_key_scroll(&mut self, width: u16, panel_h: u16, delta: i32) {
        let max = self.which_key_max_offset(width, panel_h);
        let next = (self.which_key.offset as i32 + delta).clamp(0, max as i32);
        self.which_key.offset = next as usize;
    }

    /// 滚动到组首 / 组尾。
    pub fn which_key_jump(&mut self, width: u16, panel_h: u16, to_end: bool) {
        self.which_key.offset = if to_end {
            self.which_key_max_offset(width, panel_h)
        } else {
            0
        };
    }

    /// 当前分组可滚动到的最大偏移。
    pub fn which_key_max_offset(&self, width: u16, panel_h: u16) -> usize {
        use crate::widgets::whichkey as wk;
        let items = wk::leader_items(&self.keymap);
        let Some(group) = wk::resolve_group(&self.which_key, &items) else {
            return 0;
        };
        wk::max_offset(&items, group, width, panel_h)
    }

    // ---------- reasoning 折叠（M4-1） ----------

    /// 最近一条带思考过程的消息 uid（键位快速展开用）。
    pub fn last_reasoning_uid(&self) -> Option<u64> {
        self.messages
            .iter()
            .rev()
            .find(|m| !m.reasoning.trim().is_empty())
            .map(|m| m.uid)
    }

    pub fn is_reasoning_expanded(&self, uid: u64) -> bool {
        self.expanded_reasoning.contains(&uid)
    }

    /// 展开/收起指定消息的思考过程；返回是否确有此块。
    pub fn toggle_reasoning(&mut self, uid: u64) -> bool {
        if !self
            .messages
            .iter()
            .any(|m| m.uid == uid && !m.reasoning.trim().is_empty())
        {
            return false;
        }
        if !self.expanded_reasoning.remove(&uid) {
            self.expanded_reasoning.insert(uid);
        }
        self.focused_reasoning = Some(uid);
        true
    }

    /// 切换最近一条思考块（键位入口）。
    pub fn toggle_last_reasoning(&mut self) -> bool {
        match self.last_reasoning_uid() {
            Some(uid) => self.toggle_reasoning(uid),
            None => false,
        }
    }

    // ---------- 选区与复制（M4-6） ----------

    /// 从 (x,y) 开始选区（鼠标拖拽/键盘选择模式）。
    pub fn begin_selection(&mut self, x: u16, y: u16) {
        self.selection = Some(crate::selection::Selection::new(crate::selection::Pos {
            x,
            y,
        }));
    }

    /// 扩展选区到 (x,y)。
    pub fn extend_selection(&mut self, x: u16, y: u16) {
        if let Some(sel) = self.selection.as_mut() {
            sel.cursor = crate::selection::Pos { x, y };
        } else {
            self.begin_selection(x, y);
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// 复制当前选区文本到剪贴板；无选区或空文本返回 false。
    pub fn copy_selection(&mut self) -> bool {
        let Some(sel) = self.selection else {
            return false;
        };
        let text = sel.extract(&self.screen_rows);
        if text.trim().is_empty() {
            return false;
        }
        crate::clipboard::copy(&text)
    }

    // ---------- 侧栏分区数据（M4-8） ----------

    /// 累计一次工具用量。
    pub fn note_usage(&mut self, input_tokens: u64, output_tokens: u64) {
        self.usage_input_tokens = self.usage_input_tokens.saturating_add(input_tokens);
        self.usage_output_tokens = self.usage_output_tokens.saturating_add(output_tokens);
    }

    /// 记录一个被读写/编辑的文件：最近优先、去重、上限 8。
    pub fn note_touched_file(&mut self, path: &str) {
        if path.trim().is_empty() {
            return;
        }
        self.touched_files.retain(|p| p != path);
        self.touched_files.insert(0, path.to_string());
        self.touched_files.truncate(8);
    }

    /// 是否触发了上下文压缩（marker 出现即记置位，不回落）。
    pub fn note_compacted_marker(&mut self, text: &str) {
        if text.contains("[compacted") {
            self.compacted = true;
        }
    }

    /// 切换侧栏分区折叠。
    pub fn toggle_section(&mut self, title: &str) {
        let key = title.to_string();
        if !self.collapsed_sections.remove(&key) {
            self.collapsed_sections.insert(key);
        }
    }

    pub fn is_section_collapsed(&self, title: &str) -> bool {
        self.collapsed_sections.contains(title)
    }

    // ---------- leader ----------

    pub fn arm_leader(&mut self, now: Instant) {
        self.leader_until = Some(now + Duration::from_millis(crate::keymap::LEADER_TIMEOUT_MS));
    }

    pub fn clear_leader(&mut self) {
        self.leader_until = None;
    }

    /// leader 窗口是否仍有效。
    pub fn leader_active(&self, now: Instant) -> bool {
        self.leader_until.is_some_and(|until| now <= until)
    }

    pub fn spinner_glyph(&self) -> &'static str {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        FRAMES[(self.spinner_ticks as usize) % FRAMES.len()]
    }

    /// 提交当前输入；返回是否确实发送。
    pub fn submit_prompt(&mut self, handle: &RuntimeHandle) -> bool {
        let text = self.prompt.lines().join("\n").trim().to_string();
        if text.is_empty() || self.phase.is_busy() {
            return false;
        }
        self.kv.push_history(&text);
        self.history = self.kv.data.prompt_history.clone();
        let _ = self.kv.save();
        self.hist_idx = None;
        self.draft.clear();

        self.messages.push(MessageView::user(
            alloc_uid(&mut self.next_message_uid),
            text.clone(),
        ));
        // 首条用户消息成为会话标题（截断 40 字符）
        if let Some(meta) = self
            .sessions
            .iter_mut()
            .find(|s| s.id == self.active_session)
        {
            if meta.title == "新会话" {
                meta.title = crate::render::truncate_end(&text, 40);
            }
        }
        self.route = Route::Session;
        self.set_phase(Phase::Working);
        self.stick_bottom = true;
        self.backscroll = 0;
        self.prompt.move_cursor(tui_textarea::CursorMove::Top);
        self.prompt.select_all();
        self.prompt.delete_char();
        let session_id = self.active_session;
        handle.submit(session_id, text);
        true
    }

    // ---------- 磁盘持久化（M3） ----------

    /// 绑定磁盘会话存储并载入历史列表。
    pub fn attach_session_store(&mut self, store: Option<SessionStore>) {
        if let Some(s) = &store {
            self.history_sessions = s.list();
        }
        self.session_store = store;
    }

    /// 从磁盘加载一个历史会话并切换为活动会话；返回记录供注入 kernel。
    pub fn load_history_session(&mut self, id: u64) -> Option<SessionRecord> {
        let store = self.session_store.as_ref()?;
        let record = store.load(id)?;
        if !self.sessions.iter().any(|s| s.id == id) {
            self.sessions.push(SessionMeta {
                id,
                title: record.title.clone(),
                phase: Phase::Idle,
                pinned: false,
            });
        }
        let projected = project_conversation(&record.conversation, &mut self.next_message_uid);
        for view in &projected {
            for t in [&view.text, &view.reasoning] {
                self.note_compacted_marker(t);
            }
        }
        self.buckets.insert(id, projected);
        // 切到该会话（若已是活动会话则直接替换投影）
        if self.active_session == id {
            self.messages = self.buckets.remove(&id).unwrap_or_default();
            self.route = Route::Session;
            self.reset_session_view();
        } else {
            let _ = self.switch_session(id);
        }
        Some(record)
    }

    /// 把当前活动会话投影转成 Conversation 并落盘（snapshot + 追加的 transcript）。
    ///
    /// 在每轮终态调用；空会话不落盘。
    pub fn persist_active_session(&mut self) {
        let Some(store) = self.session_store.clone() else {
            return;
        };
        let view = match self.buckets.get(&self.active_session) {
            Some(v) => v,
            None => &self.messages,
        };
        if view.is_empty() {
            return;
        }
        let conversation = conversation_from_view(view);
        let title = self
            .sessions
            .iter()
            .find(|s| s.id == self.active_session)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| mrorcode_kernel::session::derive_title(&conversation));
        let record = SessionRecord {
            id: self.active_session,
            title,
            updated_at: 0,
            conversation,
        };
        if let Err(e) = store.save_snapshot(&record) {
            tracing::warn!("save session snapshot failed: {e}");
            return;
        }
        self.history_sessions = store.list();
    }

    /// 删除磁盘历史会话（若为当前进程会话则一并清理内存）。
    pub fn delete_history_session(&mut self, id: u64) {
        if let Some(store) = &self.session_store {
            if let Err(e) = store.delete(id) {
                tracing::warn!("delete session failed: {e}");
            }
            self.history_sessions = store.list();
        }
        self.delete_session(id);
    }

    // ---------- 多会话（M2 进程内） ----------
    /// 新建会话并切换；当前消息投影收桶。返回新 id。
    pub fn new_session(&mut self) -> u64 {
        self.stash_active();
        let id = self.next_session_id;
        self.next_session_id += 1;
        self.sessions.push(SessionMeta {
            id,
            title: "新会话".to_string(),
            phase: Phase::Idle,
            pinned: false,
        });
        self.active_session = id;
        self.messages.clear();
        self.prompt = TextArea::new(vec![String::new()]);
        self.prompt
            .set_cursor_line_style(ratatui::style::Style::default());
        self.route = Route::Home;
        self.reset_session_view();
        id
    }

    pub fn switch_session(&mut self, id: u64) -> bool {
        if id == self.active_session || !self.sessions.iter().any(|s| s.id == id) {
            return false;
        }
        self.stash_active();
        self.active_session = id;
        self.messages = self.buckets.remove(&id).unwrap_or_default();
        self.route = if self.messages.is_empty() {
            Route::Home
        } else {
            Route::Session
        };
        self.restore_prompt_draft(id);
        self.reset_session_view();
        true
    }

    /// 删除会话（仅进程内，M3 接磁盘时会有真正的 unlink 确认）。
    pub fn delete_session(&mut self, id: u64) {
        self.buckets.remove(&id);
        self.drafts.remove(&id);
        self.sessions.retain(|s| s.id != id);
        if id != self.active_session {
            return;
        }
        self.messages.clear();
        match self.sessions.first().map(|s| s.id) {
            Some(target) => {
                self.active_session = target;
                self.messages = self.buckets.remove(&target).unwrap_or_default();
                self.route = if self.messages.is_empty() {
                    Route::Home
                } else {
                    Route::Session
                };
                self.restore_prompt_draft(target);
                self.reset_session_view();
            }
            None => {
                let id = self.next_session_id;
                self.next_session_id += 1;
                self.sessions.push(SessionMeta {
                    id,
                    title: "新会话".to_string(),
                    phase: Phase::Idle,
                    pinned: false,
                });
                self.active_session = id;
                self.prompt = TextArea::new(vec![String::new()]);
                self.prompt
                    .set_cursor_line_style(ratatui::style::Style::default());
                self.route = Route::Home;
                self.reset_session_view();
            }
        }
    }

    /// 按序号（leader 1..9）切换会话。
    pub fn switch_session_by_index(&mut self, n: u8) -> bool {
        match self.sessions.get(n as usize - 1) {
            Some(s) => self.switch_session(s.id),
            None => false,
        }
    }

    /// 固定/取消固定当前会话（pinned 排侧栏最前）。
    pub fn toggle_pin_active(&mut self) {
        let id = self.active_session;
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.pinned = !s.pinned;
        }
    }

    /// 重命名当前会话；空串或纯空白时忽略。同步更新磁盘快照标题。
    pub fn rename_active_session(&mut self, new_title: &str) {
        let trimmed = new_title.trim();
        if trimmed.is_empty() {
            return;
        }
        let id = self.active_session;
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.title = trimmed.to_string();
        }
        // 同步磁盘快照标题（若已持久化）
        if let Some(store) = self.session_store.clone() {
            if let Some(mut record) = store.load(id) {
                record.title = trimmed.to_string();
                if let Err(e) = store.save_snapshot(&record) {
                    tracing::warn!("rename session snapshot failed: {e}");
                }
                self.history_sessions = store.list();
            }
        }
    }

    /// 打开重命名当前会话的输入对话框。
    pub fn open_rename_dialog(&mut self) {
        let current = self
            .sessions
            .iter()
            .find(|s| s.id == self.active_session)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        self.push_dialog(Dialog::Input(InputState::new(
            "rename session",
            InputPurpose::RenameSession,
            current,
        )));
    }

    /// 打开消息时间线对话框（P2-9）。
    pub fn open_timeline(&mut self) -> bool {
        if self.messages.is_empty() {
            return false;
        }
        let title = self
            .sessions
            .iter()
            .find(|s| s.id == self.active_session)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "会话".to_string());
        self.push_dialog(Dialog::Timeline(TimelineState::from_messages(
            format!("timeline — {title}"),
            &self.messages,
        )));
        true
    }

    /// 导出当前会话为 Markdown 文件，返回文件路径。
    pub fn export_transcript(&self) -> std::io::Result<PathBuf> {
        let title = self
            .sessions
            .iter()
            .find(|s| s.id == self.active_session)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "会话".to_string());
        let safe: String = title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(40)
            .collect();
        let filename = format!("mrorcode-{safe}.md");
        let path = self.working_dir.join(&filename);

        let mut md = String::new();
        md.push_str(&format!("# {title}\n\n"));
        md.push_str(&format!("> 模型：{}\n\n", self.model));

        for msg in &self.messages {
            match msg.role {
                Role::User => {
                    md.push_str(&format!("## 🧑 用户\n\n{}\n\n", msg.text));
                }
                Role::Assistant => {
                    if !msg.reasoning.is_empty() {
                        md.push_str("<details><summary>💭 思考过程</summary>\n\n");
                        md.push_str(&format!("{}\n\n</details>\n\n", msg.reasoning));
                    }
                    md.push_str(&format!("## 🤖 助手\n\n{}\n\n", msg.text));
                    for tool in &msg.tools {
                        let status = match tool.status {
                            ToolStatus::Ok => "✓",
                            ToolStatus::Failed => "✗",
                            ToolStatus::Running => "⏳",
                            _ => "",
                        };
                        md.push_str(&format!(
                            "<details><summary>🔧 {} {}</summary>\n\n```\n{}\n```\n\n</details>\n\n",
                            tool.name, status, tool.output
                        ));
                    }
                }
            }
        }

        std::fs::write(&path, &md)?;
        Ok(path)
    }

    /// 前后循环切换会话标签（M4-4）。
    pub fn cycle_session(&mut self, next: bool) -> bool {
        if self.sessions.len() < 2 {
            return false;
        }
        let cur = self
            .sessions
            .iter()
            .position(|s| s.id == self.active_session)
            .unwrap_or(0);
        let len = self.sessions.len();
        let idx = if next {
            (cur + 1) % len
        } else {
            (cur + len - 1) % len
        };
        self.switch_session(self.sessions[idx].id)
    }

    pub fn open_session_picker(&mut self) {
        let mut items: Vec<SelectItem> = self
            .sessions
            .iter()
            .map(|s| SelectItem {
                hint: if s.id == self.active_session {
                    "active".to_string()
                } else if s.phase.is_busy() {
                    "busy".to_string()
                } else {
                    String::new()
                },
                group: "会话".to_string(),
                label: s.title.clone(),
                value: s.id.to_string(),
            })
            .collect();
        // 磁盘历史（未装入本进程的）单列一组，value 前缀 disk: 区分。
        for h in &self.history_sessions {
            if self.sessions.iter().any(|s| s.id == h.id) {
                continue;
            }
            items.push(SelectItem {
                hint: format!("saved · {} msg", h.message_count),
                group: "On disk".to_string(),
                label: h.title.clone(),
                value: format!("disk:{}", h.id),
            });
        }
        self.push_dialog(Dialog::Select(SelectState::new(
            "sessions",
            SelectPurpose::Session,
            items,
        )));
    }

    /// 当前活动消息收桶（空消息不占桶）；同时暂存输入草稿。
    fn stash_active(&mut self) {
        if !self.messages.is_empty() {
            self.buckets
                .insert(self.active_session, std::mem::take(&mut self.messages));
        }
        // 暂存输入草稿（非空才存，空草稿不占空间）
        let text = self.prompt.lines().join("\n");
        if !text.is_empty() {
            self.drafts.insert(self.active_session, text);
        } else {
            self.drafts.remove(&self.active_session);
        }
    }

    /// 恢复指定会话的输入草稿。
    fn restore_prompt_draft(&mut self, id: u64) {
        let text = self.drafts.remove(&id).unwrap_or_default();
        let lines: Vec<String> = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|s| s.to_string()).collect()
        };
        self.prompt = TextArea::new(lines);
        self.prompt
            .set_cursor_line_style(ratatui::style::Style::default());
        // 光标移到末尾
        let last = self.prompt.lines().len().saturating_sub(1);
        let col = self.prompt.lines().get(last).map(|l| l.len()).unwrap_or(0);
        self.prompt
            .move_cursor(tui_textarea::CursorMove::Jump(last as u16, col as u16));
    }

    /// 打开 help：行数据由注册表 + 当前 keymap（含用户覆盖）实时生成。
    pub fn open_help(&mut self) {
        let rows = commands::registry()
            .iter()
            .map(|c| HelpRow {
                group: c.category.label().to_string(),
                name: c.name.to_string(),
                desc: c.desc.to_string(),
                keys: self.keymap.keys_for(commands::to_action(c.command)),
            })
            .collect();
        self.push_dialog(Dialog::Help(HelpState::new(rows)));
    }

    /// 打开单按钮 Alert。
    pub fn open_alert(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.push_dialog(Dialog::Alert(AlertState::new(title, message)));
    }

    /// 删除当前会话的确认框。
    pub fn open_delete_active_confirm(&mut self) {
        let id = self.active_session;
        let title = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "新会话".to_string());
        let message = format!("删除会话「{title}」？磁盘记录将一并删除。");
        self.push_dialog(Dialog::Confirm(ConfirmState::new(
            "delete session",
            message,
            ConfirmAction::DeleteSession(id),
        )));
    }

    /// 打开 rewind 选择器（M4-9）：列出历史轮次边界，选中即回退到该轮之前。
    /// busy 时不可回退，返回 false。
    pub fn open_rewind_picker(&mut self) -> bool {
        if self.phase.is_busy() {
            return false;
        }
        // 边界 = 每个 user turn 之后；value 以 turn 数计，与 kernel rewind 语义一致。
        let mut items = Vec::new();
        let mut turn = 0usize;
        for m in &self.messages {
            if matches!(m.role, Role::User) {
                turn += 1;
                let summary = crate::render::truncate_end(&m.text, 44);
                items.push(SelectItem {
                    label: format!("#{turn}  {summary}"),
                    hint: "回退到此轮之后".to_string(),
                    group: "Rewind".to_string(),
                    value: turn.to_string(),
                });
            }
        }
        if items.len() < 2 {
            return false;
        }
        self.push_dialog(crate::widgets::dialog::Dialog::Select(SelectState::new(
            "rewind session",
            SelectPurpose::Rewind,
            items,
        )));
        true
    }

    /// 执行 rewind：本地截断投影 + 通知 kernel + 覆盖 snapshot。
    /// `keep_turns`：保留的 user turn 数（含该轮之后收尾），其后截断。
    pub fn rewind_to(&mut self, keep_turns: usize, handle: &RuntimeHandle) {
        let session_id = self.active_session;
        if self.rewind_projection(keep_turns) {
            handle.rewind(session_id, keep_turns);
        }
    }

    /// 纯投影层 rewind：截断 messages / 回卷滚动 / 清工具焦点 / 覆盖 snapshot。
    /// 连带回滚被截断 turn 中的文件改动（edit 反向替换、write 删除文件）。
    /// 返回是否实际执行（turn 数不合法或 busy 时为 false）。
    pub fn rewind_projection(&mut self, keep_turns: usize) -> bool {
        if self.phase.is_busy() || keep_turns == 0 {
            return false;
        }
        // 定位第 keep_turns 个 user turn 的结尾（其后的第一条 user 消息之前；无则到末尾）。
        let mut seen = 0usize;
        let cutoff = self.messages.iter().position(|m| {
            if matches!(m.role, Role::User) {
                seen += 1;
                seen > keep_turns
            } else {
                false
            }
        });
        let cutoff = cutoff.unwrap_or(self.messages.len());
        if cutoff == 0 {
            return false;
        }
        // 连带回滚文件改动：收集被截断 turn 中的 edit/write 工具，反向应用
        let rolled_back = self.rollback_file_changes(cutoff);

        self.messages.truncate(cutoff);
        // 回卷滚动状态到最底
        self.stick_bottom = true;
        self.backscroll = 0;
        self.clear_tool_focus();
        self.persist_active_session();
        if rolled_back > 0 {
            self.toast(format!(
                "已回退并回滚 {rolled_back} 个文件改动；磁盘 transcript 保留完整历史"
            ));
        } else {
            self.toast("已回退；仅当前会话生效，磁盘 transcript 保留完整历史");
        }
        true
    }

    /// 回滚 cutoff 之后消息中的文件改动（edit 反向替换、write 删除文件）。
    /// 返回成功回滚的文件数。
    fn rollback_file_changes(&self, cutoff: usize) -> usize {
        let mut rolled = 0usize;
        for msg in &self.messages[cutoff..] {
            for tool in &msg.tools {
                match tool.name.as_str() {
                    "edit_file" | "edit" => {
                        if let Some((path, old, new)) = parse_edit_args(&tool.arguments) {
                            let full = self.working_dir.join(&path);
                            if let Ok(content) = std::fs::read_to_string(&full) {
                                if content.contains(&new) {
                                    let restored = content.replacen(&new, &old, 1);
                                    if std::fs::write(&full, restored).is_ok() {
                                        rolled += 1;
                                    }
                                }
                            }
                        }
                    }
                    "write_file" | "write" => {
                        if let Some(path) = parse_write_path(&tool.arguments) {
                            let full = self.working_dir.join(&path);
                            if full.exists() && std::fs::remove_file(&full).is_ok() {
                                rolled += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        rolled
    }

    /// 打开当前聚焦工具卡的 diff viewer；无可展开 diff 时返回 false。
    pub fn open_focused_diff(&mut self) -> bool {
        let Some(id) = self.focused_tool.clone() else {
            return false;
        };
        let found = self.messages.iter().find_map(|m| {
            m.tools
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.name.clone(), t.output.clone()))
        });
        let Some((name, output)) = found else {
            return false;
        };
        match crate::widgets::dialog::DiffState::from_output(name, &output) {
            Some(state) => {
                self.push_dialog(Dialog::Diff(state));
                true
            }
            None => false,
        }
    }

    /// 打开独立 diff viewer 的数据源选择器（git / branch / last-turn）。
    pub fn open_diff_source_picker(&mut self) {
        let items = vec![
            SelectItem {
                label: "git diff".to_string(),
                hint: "工作区 vs 暂存区".to_string(),
                group: "Diff Source".to_string(),
                value: "git".to_string(),
            },
            SelectItem {
                label: "branch diff".to_string(),
                hint: "当前分支 vs 主分支".to_string(),
                group: "Diff Source".to_string(),
                value: "branch".to_string(),
            },
            SelectItem {
                label: "last turn".to_string(),
                hint: "上一轮 AI 文件改动".to_string(),
                group: "Diff Source".to_string(),
                value: "last-turn".to_string(),
            },
        ];
        self.push_dialog(Dialog::Select(SelectState::new(
            "diff source",
            SelectPurpose::DiffSource,
            items,
        )));
    }

    /// 从当前会话收集最近一轮（最后一个 user turn 之后）的 edit/write 工具输出，
    /// 拼接为 unified diff 文本；无改动时返回 None。
    pub fn collect_last_turn_diff(&self) -> Option<String> {
        // 找到最后一个 user 消息的位置，之后的 assistant 消息中的工具输出即为"上一轮"
        let mut last_user_idx = None;
        for (i, m) in self.messages.iter().enumerate() {
            if matches!(m.role, Role::User) {
                last_user_idx = Some(i);
            }
        }
        let start = last_user_idx.map(|i| i + 1).unwrap_or(0);
        let mut out = String::new();
        for m in &self.messages[start..] {
            for t in &m.tools {
                if matches!(t.name.as_str(), "edit" | "write") && !t.output.is_empty() {
                    out.push_str(&t.output);
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    fn reset_session_view(&mut self) {
        self.clear_tool_focus();
        self.permission = None;
        self.dialogs.clear();
        self.stick_bottom = true;
        self.backscroll = 0;
        self.last_total_lines = 0;
        self.phase = self
            .sessions
            .iter()
            .find(|s| s.id == self.active_session)
            .map(|s| s.phase)
            .unwrap_or(Phase::Idle);
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        if let Some(meta) = self
            .sessions
            .iter_mut()
            .find(|s| s.id == self.active_session)
        {
            meta.phase = phase;
        }
    }

    pub fn on_runtime_event(&mut self, session_id: u64, ev: AgentEvent, handle: &RuntimeHandle) {
        let terminal = matches!(
            ev,
            AgentEvent::TurnComplete { .. } | AgentEvent::Cancelled | AgentEvent::Error { .. }
        );
        // M4-8 侧栏记账（与会话无关，全局累计）
        match &ev {
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => self.note_usage(*input_tokens, *output_tokens),
            AgentEvent::ToolStarted {
                name, arguments, ..
            } if is_file_tool(name) => {
                if let Some(p) = path_from_arguments(arguments) {
                    self.note_touched_file(&p);
                }
            }
            AgentEvent::ToolResult { name, .. }
                if session_id == self.active_session
                    && matches!(name.as_str(), "plan_enter" | "plan_exit") =>
            {
                // 兼容旧 custom 命令：plan_enter → 只读，plan_exit → 工作区读写
                let mode = if name == "plan_enter" {
                    PermissionMode::ReadOnly
                } else {
                    PermissionMode::Workspace
                };
                if self.mode != mode {
                    self.set_mode(mode, handle);
                }
            }
            _ => {}
        }
        if session_id == self.active_session {
            // 全局 UI 态仅随活动会话更新
            match &ev {
                AgentEvent::TurnStarted => self.set_phase(Phase::Working),
                AgentEvent::TextDelta(_) | AgentEvent::ToolStarted { .. } => {
                    self.set_phase(Phase::Working)
                }
                AgentEvent::Request { id, kind, payload } if kind == "approval" => {
                    self.set_phase(Phase::WaitingApproval);
                    let tool = payload
                        .get("tool")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    let arguments = payload
                        .get("arguments")
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    self.permission = Some(PermissionRequest {
                        id: *id,
                        tool,
                        arguments,
                    });
                    self.permission_cursor = 0;
                }
                AgentEvent::TurnComplete { .. } => {
                    self.set_phase(Phase::Idle);
                    // 失焦时播放终端 bell 音提醒用户
                    if !self.terminal_focused {
                        use std::io::Write;
                        let _ = std::io::stdout().write_all(b"\x07");
                        let _ = std::io::stdout().flush();
                    }
                }
                AgentEvent::Cancelled => {
                    self.set_phase(Phase::Idle);
                    self.permission = None;
                    self.toast("已中断");
                }
                AgentEvent::Error { message, .. } => {
                    self.set_phase(Phase::Idle);
                    self.toast_error(format!("错误：{message}"));
                }
                _ => {}
            }
            project_event(ev, &mut self.messages, &mut self.next_message_uid);
        } else {
            // 后台会话：只更新侧栏 phase 与消息桶（M2 单 turn，正常不可见）
            let bg_phase = match ev {
                AgentEvent::TurnStarted
                | AgentEvent::TextDelta(_)
                | AgentEvent::ToolStarted { .. }
                | AgentEvent::ToolResult { .. }
                | AgentEvent::Reasoning(_) => Some(Phase::Working),
                AgentEvent::TurnComplete { .. }
                | AgentEvent::Cancelled
                | AgentEvent::Error { .. } => Some(Phase::Idle),
                _ => None,
            };
            if let Some(phase) = bg_phase {
                if let Some(meta) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    meta.phase = phase;
                }
            }
            if let Some(bucket) = self.buckets.get_mut(&session_id) {
                project_event(ev, bucket, &mut self.next_message_uid);
            }
        }
        let _ = handle;
        // 每轮终态落盘（不参与流式热路径）。
        if terminal && session_id == self.active_session {
            self.persist_active_session();
        }
    }

    pub fn resolve_permission(&mut self, decision: &str, handle: &RuntimeHandle) {
        if let Some(req) = self.permission.take() {
            let value = serde_json::json!({ "decision": decision });
            handle.respond(req.id, value);
        }
        self.set_phase(Phase::Working);
    }

    pub fn scroll_up(&mut self, rows: u16) {
        self.stick_bottom = false;
        self.backscroll = self.backscroll.saturating_add(rows);
        let max = self.last_total_lines.saturating_sub(self.viewport_lines);
        self.backscroll = self.backscroll.min(max);
    }

    pub fn scroll_down(&mut self, rows: u16) {
        if self.backscroll <= rows {
            self.backscroll = 0;
            self.stick_bottom = true;
        } else {
            self.backscroll -= rows;
        }
    }
}

/// 分配一个进程内消息 uid。
fn alloc_uid(next: &mut u64) -> u64 {
    let uid = *next;
    *next = next.wrapping_add(1);
    uid
}

/// 该工具是否涉及文件路径（M4-8 Files 分区记账）。
fn is_file_tool(name: &str) -> bool {
    matches!(name, "read" | "write" | "edit" | "glob" | "grep" | "diff")
}

/// 从工具参数 JSON 中提取文件路径（`path` 字段优先，其次 `file_path`）。
fn path_from_arguments(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let v = v.get("path").or_else(|| v.get("file_path"))?;
    let p = v.as_str()?;
    if p.trim().is_empty() {
        None
    } else {
        Some(p.to_string())
    }
}

/// 从 edit 工具参数中提取 (path, old, new)，用于回滚。
fn parse_edit_args(arguments: &str) -> Option<(String, String, String)> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let path = v.get("path")?.as_str()?.to_string();
    let old = v.get("old")?.as_str()?.to_string();
    let new = v.get("new")?.as_str()?.to_string();
    Some((path, old, new))
}

/// 从 write 工具参数中提取文件路径，用于回滚（删除文件）。
fn parse_write_path(arguments: &str) -> Option<String> {
    path_from_arguments(arguments)
}

/// 纯消息投影：把 agent 事件追加进某个会话的消息视图（活动/后台桶通用）。
/// `next_uid` 为消息稳定 id 分配器。
fn project_event(ev: AgentEvent, messages: &mut Vec<MessageView>, next_uid: &mut u64) {
    match ev {
        AgentEvent::TurnStarted => messages.push(MessageView::assistant(alloc_uid(next_uid))),
        AgentEvent::TextDelta(t) => {
            if let Some(last) = messages.last_mut() {
                last.text.push_str(&t);
                last.doc.update(&last.text);
                last.streaming = true;
            }
        }
        AgentEvent::Reasoning(t) => {
            if let Some(last) = messages.last_mut() {
                last.reasoning.push_str(&t);
                last.reasoning_doc.update(&last.reasoning);
            }
        }
        AgentEvent::ToolStarted {
            id,
            name,
            arguments,
        } => {
            if let Some(last) = messages.last_mut() {
                if let Some(tool) = last.tools.iter_mut().find(|t| t.id == id) {
                    tool.status = ToolStatus::Running;
                    tool.arguments = arguments;
                } else {
                    last.tools.push(ToolView {
                        id,
                        name,
                        arguments,
                        status: ToolStatus::Running,
                        output: String::new(),
                        sub_agent_text: String::new(),
                        sub_agent_tools: Vec::new(),
                    });
                }
            }
        }
        AgentEvent::ToolResult {
            id,
            content,
            is_error,
            ..
        } => {
            if let Some(last) = messages.last_mut() {
                if let Some(tool) = last.tools.iter_mut().find(|t| t.id == id) {
                    tool.status = if is_error {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Ok
                    };
                    tool.output = content;
                }
            }
        }
        AgentEvent::TurnComplete { .. } | AgentEvent::Cancelled => {
            if let Some(last) = messages.last_mut() {
                last.streaming = false;
            }
        }
        AgentEvent::Error { message, .. } => {
            if let Some(last) = messages.last_mut() {
                last.is_error = true;
                last.streaming = false;
                last.text = format!("{}\n[error] {message}", last.text);
                last.doc.update(&last.text);
            }
        }
        AgentEvent::Usage { .. } | AgentEvent::Request { .. } => {}
        AgentEvent::SubAgent {
            parent_tool_id,
            event,
        } => {
            // 子 agent 事件投影：定位到主会话中对应的 task 工具卡，
            // 把子 agent 的文本增量和工具调用实时写入工具卡。
            if let Some(last) = messages.last_mut() {
                if let Some(tool) = last.tools.iter_mut().find(|t| t.id == parent_tool_id) {
                    match *event {
                        AgentEvent::TextDelta(t) => {
                            tool.sub_agent_text.push_str(&t);
                        }
                        AgentEvent::ToolStarted { name, .. } => {
                            tool.sub_agent_tools.push(SubAgentToolView {
                                name,
                                status: ToolStatus::Running,
                            });
                        }
                        AgentEvent::ToolResult { name, is_error, .. } => {
                            if let Some(sub) = tool
                                .sub_agent_tools
                                .iter_mut()
                                .rev()
                                .find(|s| s.name == name)
                            {
                                sub.status = if is_error {
                                    ToolStatus::Failed
                                } else {
                                    ToolStatus::Ok
                                };
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
impl MessageView {
    pub(crate) fn user(uid: u64, text: String) -> Self {
        let mut doc = MarkdownView::new();
        doc.update(&text);
        Self {
            uid,
            role: Role::User,
            text,
            doc,
            reasoning: String::new(),
            reasoning_doc: MarkdownView::new(),
            tools: Vec::new(),
            is_error: false,
            streaming: false,
        }
    }

    pub(crate) fn assistant(uid: u64) -> Self {
        Self {
            uid,
            role: Role::Assistant,
            text: String::new(),
            doc: MarkdownView::new(),
            reasoning: String::new(),
            reasoning_doc: MarkdownView::new(),
            tools: Vec::new(),
            is_error: false,
            streaming: true,
        }
    }
}

/// 用整行替换输入框的第 row 行（保留其他行）。
fn replace_current_line(prompt: &mut TextArea<'static>, row: usize, new_line: &str) {
    prompt.clear_atomic_ranges();
    prompt.move_cursor(tui_textarea::CursorMove::Jump(row as u16, 0));
    prompt.move_cursor(tui_textarea::CursorMove::End);
    for _ in 0..prompt.lines()[row].chars().count() {
        prompt.delete_char();
    }
    prompt.insert_str(new_line);
}

/// 把光标移动到 (row, col)。
fn set_cursor(prompt: &mut TextArea<'static>, row: usize, col: usize) {
    prompt.move_cursor(tui_textarea::CursorMove::Jump(row as u16, col as u16));
}

/// 消息投影 → kernel 会话体（持久化用）。
fn conversation_from_view(view: &[MessageView]) -> Conversation {
    let mut convo = Conversation::new();
    for m in view {
        match m.role {
            Role::User => convo.push(Message::user(m.text.clone())),
            Role::Assistant => {
                let mut msg = Message::assistant_text(m.text.clone());
                if !m.reasoning.is_empty() {
                    msg.reasoning = Some(m.reasoning.clone());
                }
                for t in &m.tools {
                    msg.tool_calls.push(ToolCall {
                        id: t.id.clone(),
                        name: t.name.clone(),
                        arguments: t.arguments.clone(),
                    });
                }
                convo.push(msg);
                for t in &m.tools {
                    convo.push(Message::tool_result(&mrorcode_kernel::tool::ToolResult {
                        call_id: t.id.clone(),
                        content: t.output.clone(),
                        is_error: t.status == ToolStatus::Failed,
                    }));
                }
            }
        }
    }
    convo
}

/// kernel 会话体 → 消息投影（resume 用）。把 assistant+tool_result 归并到同一 MessageView。
fn project_conversation(convo: &Conversation, next_uid: &mut u64) -> Vec<MessageView> {
    let mut out: Vec<MessageView> = Vec::new();
    for msg in &convo.messages {
        match msg.role {
            mrorcode_kernel::message::Role::System => {}
            mrorcode_kernel::message::Role::User => {
                out.push(MessageView::user(alloc_uid(next_uid), msg.text.clone()))
            }
            mrorcode_kernel::message::Role::Assistant => {
                let mut view = MessageView::assistant(alloc_uid(next_uid));
                view.streaming = false;
                view.text = msg.text.clone();
                view.reasoning = msg.reasoning.clone().unwrap_or_default();
                view.reasoning_doc.update(&view.reasoning);
                view.doc.update(&view.text);
                for call in &msg.tool_calls {
                    view.tools.push(ToolView {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        status: ToolStatus::Running,
                        output: String::new(),
                        sub_agent_text: String::new(),
                        sub_agent_tools: Vec::new(),
                    });
                }
                out.push(view);
            }
            mrorcode_kernel::message::Role::Tool => {
                if let (Some(last), Some(call_id)) = (out.last_mut(), msg.tool_call_id.as_deref()) {
                    if let Some(tool) = last.tools.iter_mut().find(|t| t.id == call_id) {
                        tool.output = msg.text.clone();
                        tool.status = if msg.is_error {
                            ToolStatus::Failed
                        } else {
                            ToolStatus::Ok
                        };
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Store as KvStore;
    use crate::widgets::dialog::DialogOutcome;
    use crate::widgets::whichkey::WkLayout;

    fn temp_state(tag: &str) -> AppState {
        let path = std::env::temp_dir().join(format!(
            "mrorcode-storetest-{}-{tag}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let (kv, _) = KvStore::load_from(&path);
        AppState::new("m".into(), Vec::new(), PathBuf::from("."), kv)
    }

    #[test]
    fn conversation_view_roundtrip_preserves_text_and_tools() {
        let mut view = Vec::new();
        view.push(MessageView::user(1, "hello".into()));
        let mut assistant = MessageView::assistant(2);
        assistant.text = "answer".into();
        assistant.tools.push(ToolView {
            id: "c1".into(),
            name: "bash".into(),
            arguments: "{\"command\":\"ls\"}".into(),
            status: ToolStatus::Ok,
            output: "listed".into(),
            sub_agent_text: String::new(),
            sub_agent_tools: Vec::new(),
        });
        view.push(assistant);

        let convo = conversation_from_view(&view);
        assert_eq!(convo.messages.len(), 3); // user + assistant + tool_result
        let back = project_conversation(&convo, &mut 1);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].text, "hello");
        assert_eq!(back[1].text, "answer");
        assert_eq!(back[1].tools.len(), 1);
        assert_eq!(back[1].tools[0].output, "listed");
        assert_eq!(back[1].tools[0].status, ToolStatus::Ok);
    }

    #[test]
    fn persist_and_reload_history_session() {
        let root = std::env::temp_dir().join(format!("mrorcode-storesess-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = SessionStore::with_root(root.clone());

        let mut state = temp_state("persist");
        state.attach_session_store(Some(store.clone()));
        state
            .messages
            .push(MessageView::user(1, "remember me".into()));
        state.persist_active_session();
        let id = state.active_session;

        // 新状态从磁盘恢复
        let mut fresh = temp_state("reload");
        fresh.attach_session_store(Some(store));
        assert!(fresh.history_sessions.iter().any(|h| h.id == id));
        let record = fresh.load_history_session(id).unwrap();
        assert_eq!(record.conversation.messages[0].text, "remember me");
        assert_eq!(fresh.messages.len(), 1);
        assert_eq!(fresh.messages[0].text, "remember me");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn completion_accepts_slash_command() {
        let mut state = temp_state("completion");
        state.prompt.insert_str("/he");
        state.refresh_completion();
        assert!(state.completion.is_some());
        // 选中 help（fuzzy 命中）
        assert!(state.accept_completion());
        let line = state.prompt.lines().join("");
        assert!(line.starts_with('/'), "got {line:?}");
        assert!(state.completion.is_none() || state.completion.as_ref().unwrap().visible());
    }

    #[test]
    fn reasoning_toggle_targets_last_block() {
        let mut state = temp_state("reasoning");
        let mut m = MessageView::assistant(7);
        m.reasoning = "think one\nthink two".into();
        state.messages.push(m);
        assert_eq!(state.last_reasoning_uid(), Some(7));
        assert!(state.toggle_last_reasoning());
        assert!(state.is_reasoning_expanded(7));
        assert!(state.toggle_last_reasoning());
        assert!(!state.is_reasoning_expanded(7));

        // 无思考块时返回 false（按键落给输入框）
        let mut none = temp_state("reasoning-none");
        none.messages.push(MessageView::assistant(1));
        assert!(!none.toggle_last_reasoning());
    }

    #[test]
    fn selection_extracts_from_screen_rows() {
        let mut state = temp_state("selection");
        state.screen_rows = vec!["hello world".into(), "second line".into()];
        state.begin_selection(0, 0);
        state.extend_selection(4, 1);
        assert_eq!(
            state.selection.unwrap().extract(&state.screen_rows),
            "hello\nsecon"
        );
        state.clear_selection();
        assert!(state.selection.is_none());
    }

    #[test]
    fn sidebar_accounting_accumulates_and_dedups() {
        let mut st = temp_state("sidebar");
        st.note_usage(120, 30);
        st.note_usage(80, 70);
        assert_eq!(st.usage_input_tokens, 200);
        assert_eq!(st.usage_output_tokens, 100);
        assert!(!st.compacted);
        st.note_compacted_marker("hello [compacted 12 earlier]");
        assert!(st.compacted);

        for i in 0..10 {
            st.note_touched_file(&format!("f{i}.rs"));
        }
        st.note_touched_file("f5.rs");
        assert_eq!(st.touched_files.len(), 8);
        assert_eq!(st.touched_files[0], "f5.rs");
        assert!(!st.touched_files.contains(&"f0.rs".to_string()));
    }

    #[test]
    fn sidebar_section_toggles() {
        let mut st = temp_state("section");
        assert!(!st.is_section_collapsed("文件"));
        st.toggle_section("文件");
        assert!(st.is_section_collapsed("文件"));
        st.toggle_section("文件");
        assert!(!st.is_section_collapsed("文件"));
    }

    #[test]
    fn path_extraction_from_tool_arguments() {
        assert_eq!(
            path_from_arguments(r#"{"path":"src/main.rs","line":"1"}"#).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            path_from_arguments(r#"{"file_path":"a/b.txt"}"#).as_deref(),
            Some("a/b.txt")
        );
        assert_eq!(path_from_arguments(r#"{"cmd":"ls"}"#), None);
        assert_eq!(path_from_arguments("not json"), None);
        assert_eq!(path_from_arguments(r#"{"path":""}"#), None);
        assert!(is_file_tool("read") && is_file_tool("edit"));
        assert!(!is_file_tool("bash"));
    }

    #[test]
    fn rewind_picker_steps_through_user_turns() {
        let mut st = temp_state("rewind-picker");
        st.messages.push(MessageView::user(1, "first round".into()));
        st.messages.push(MessageView::assistant(2));
        st.messages
            .push(MessageView::user(3, "second round".into()));
        st.messages.push(MessageView::assistant(4));
        assert!(st.open_rewind_picker());
        let Some(Dialog::Select(sel)) = st.dialogs.first() else {
            panic!("expected rewind select dialog");
        };
        assert_eq!(sel.purpose(), SelectPurpose::Rewind);
        assert_eq!(sel.len(), 2);
        // 接受时返回 turn 数（1..2），与 kernel rewind 语义一致
        let mut sel = match st.dialogs.remove(0) {
            Dialog::Select(sel) => sel,
            _ => unreachable!(),
        };
        for (i, expect_turn) in ["1", "2"].iter().enumerate() {
            if i > 0 {
                sel.on_key(&crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Down,
                    crossterm::event::KeyModifiers::NONE,
                ));
            }
            let out = sel.on_key(&crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            ));
            assert!(matches!(
                out,
                DialogOutcome::Accepted(SelectPurpose::Rewind, ref v) if v == expect_turn
            ));
        }
    }

    #[test]
    fn rewind_picker_needs_at_least_two_turns() {
        let mut st = temp_state("rewind-picker-1");
        st.messages.push(MessageView::user(1, "only one".into()));
        assert!(!st.open_rewind_picker());
    }

    #[test]
    fn rewind_projection_truncates_and_tracks() {
        let mut st = temp_state("rewind-trunc");
        st.messages.push(MessageView::user(1, "hello".into()));
        let mut a1 = MessageView::assistant(2);
        a1.text = "answer one".into();
        st.messages.push(a1);
        st.messages.push(MessageView::user(3, "world".into()));
        let mut a2 = MessageView::assistant(4);
        a2.text = "answer two".into();
        st.messages.push(a2);
        st.backscroll = 5;
        // keep 1 turn → 保留第 1 个 user 及其后到下一个 user 之前的消息
        assert!(st.rewind_projection(1));
        assert_eq!(st.messages.len(), 2);
        assert_eq!(st.messages[0].text, "hello");
        assert_eq!(st.messages[1].text, "answer one");
        assert_eq!(st.backscroll, 0);
        // keep 不合法（0）时为 no-op
        assert!(!st.rewind_projection(0));
    }

    #[test]
    fn toast_levels_replace_current() {
        let mut st = temp_state("toast-levels");
        st.toast("info a");
        st.toast_warn("warn b");
        // 新 toast 替换当前，级别随最新一条
        assert_eq!(st.active_toast(), Some((ToastLevel::Warn, "warn b")));
        st.toast_error("err c");
        assert_eq!(st.active_toast(), Some((ToastLevel::Error, "err c")));
    }

    #[test]
    fn recent_models_are_recorded_mru_first_and_capped() {
        let mut st = temp_state("recent-models");
        st.available_models = vec!["a".into(), "b".into(), "c".into()];
        assert!(st.set_model("b"));
        st.set_model("c");
        st.set_model("b");
        // 最近使用在前，重复项前移而非重复
        assert_eq!(st.recent_models(), vec!["b", "c", "a"]);
        // 持久化的 recent 列表按上限截断
        st.available_models = (0..RECENT_MODELS_MAX as u32 + 3)
            .map(|i| format!("m{i}"))
            .collect();
        for i in 0..RECENT_MODELS_MAX as u32 + 3 {
            st.set_model(&format!("m{i}"));
        }
        let stored = st.kv.data.recent_models.clone().unwrap();
        assert_eq!(stored.len(), RECENT_MODELS_MAX);
        assert_eq!(stored[0], format!("m{}", RECENT_MODELS_MAX + 2));
        // 循环范围 = recent 前缀 + 其余可用模型
        let cycle = st.recent_models();
        assert_eq!(cycle.len(), st.available_models.len());
        assert_eq!(cycle[0], stored[0]);
    }

    #[test]
    fn set_model_rejects_unknown_model() {
        let mut st = temp_state("set-model-unknown");
        st.available_models = vec!["a".into()];
        assert!(!st.set_model("zzz"));
        assert_eq!(st.model, "m");
    }

    #[test]
    fn which_key_toggle_pins_panel_and_persists() {
        let mut st = temp_state("which-key-toggle");
        assert!(!st.which_key_visible());
        st.which_key.offset = 3;
        st.toggle_which_key();
        // 打开后常驻显示，不随 leader 窗口消失
        assert!(st.which_key_visible());
        st.toggle_which_key();
        assert!(!st.which_key_visible());
        // 关闭时重置滚动位置
        assert_eq!(st.which_key.offset, 0);
        assert_eq!(st.kv.data.which_key, Some(false));
    }

    #[test]
    fn which_key_layout_toggles_and_persists() {
        let mut st = temp_state("which-key-layout");
        assert_eq!(st.which_key_layout, WkLayout::Dock);
        st.toggle_which_key_layout();
        assert_eq!(st.which_key_layout, WkLayout::Overlay);
        assert_eq!(st.kv.data.which_key_layout.as_deref(), Some("overlay"));
        st.toggle_which_key_layout();
        assert_eq!(st.which_key_layout, WkLayout::Dock);
    }

    #[test]
    fn which_key_group_delta_wraps_around() {
        use crate::widgets::whichkey as wk;
        let mut st = temp_state("which-key-group");
        let n = wk::group_labels(&wk::leader_items(&st.keymap)).len();
        assert!(n > 1);
        for _ in 0..n {
            st.which_key_group_delta(1);
        }
        assert_eq!(st.which_key.group, Some(0));
        st.which_key_group_delta(-1);
        assert_eq!(st.which_key.group, Some(n - 1));
    }

    #[test]
    fn which_key_scroll_is_clamped_to_max_offset() {
        use crate::widgets::whichkey as wk;
        let mut st = temp_state("which-key-scroll");
        st.toggle_which_key();
        let (w, h) = (30u16, 8u16);
        // 选条目最多的分组；窄面板单列 3 行，条目多于一页才可滚动
        let items = wk::leader_items(&st.keymap);
        let labels = wk::group_labels(&items);
        let mut best = (0usize, 0usize);
        for (i, g) in labels.iter().enumerate() {
            let n = wk::group_entries(&items, g).len();
            if n > best.0 {
                best = (n, i);
            }
        }
        st.which_key.group = Some(best.1);
        let max = st.which_key_max_offset(w, h);
        assert!(
            max > 0,
            "expected group with {} entries to overflow",
            best.0
        );
        st.which_key_scroll(w, h, 1);
        assert_eq!(st.which_key.offset, 1);
        st.which_key_jump(w, h, true);
        assert_eq!(st.which_key.offset, max);
        st.which_key_scroll(w, h, 99);
        assert_eq!(st.which_key.offset, max);
        st.which_key_jump(w, h, false);
        assert_eq!(st.which_key.offset, 0);
        st.which_key_scroll(w, h, -99);
        assert_eq!(st.which_key.offset, 0);
        // 切组后回到组首
        st.which_key_jump(w, h, true);
        st.which_key_group_delta(1);
        assert_eq!(st.which_key.offset, 0);
        assert!(wk::page_size(w, h) > 0);
    }

    #[test]
    fn rename_active_session_updates_title_and_persists() {
        let root = std::env::temp_dir().join(format!("mrorcode-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = SessionStore::with_root(root.clone());

        let mut st = temp_state("rename");
        st.attach_session_store(Some(store.clone()));
        st.messages.push(MessageView::user(1, "hello".into()));
        st.persist_active_session();
        let id = st.active_session;

        // 重命名
        st.rename_active_session("  我的会话  ");
        let title = st
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap();
        assert_eq!(title, "我的会话"); // trim 生效

        // 磁盘快照标题同步更新
        let record = store.load(id).unwrap();
        assert_eq!(record.title, "我的会话");

        // 空串不改名
        st.rename_active_session("   ");
        let title2 = st
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap();
        assert_eq!(title2, "我的会话");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn open_rename_dialog_pushes_input_with_current_title() {
        let mut st = temp_state("rename-dialog");
        st.sessions[0].title = "原标题".into();
        assert!(st.dialogs.is_empty());
        st.open_rename_dialog();
        assert_eq!(st.dialogs.len(), 1);
        match &st.dialogs[0] {
            Dialog::Input(input) => {
                assert_eq!(input.text(), "原标题");
            }
            _ => panic!("expected Input dialog"),
        }
    }

    #[test]
    fn open_diff_source_picker_lists_three_sources() {
        let mut st = temp_state("diff-picker");
        st.open_diff_source_picker();
        assert_eq!(st.dialogs.len(), 1);
        match &st.dialogs[0] {
            Dialog::Select(sel) => {
                assert_eq!(sel.len(), 3);
            }
            _ => panic!("expected Select dialog"),
        }
    }

    #[test]
    fn collect_last_turn_diff_gathers_edit_outputs() {
        let mut st = temp_state("last-turn");
        // user turn
        st.messages.push(MessageView::user(1, "请修改文件".into()));
        // assistant turn with edit tool
        let mut assistant = MessageView::assistant(2);
        assistant.tools.push(ToolView {
            id: "t1".into(),
            name: "edit".into(),
            arguments: "{}".into(),
            status: ToolStatus::Ok,
            output:
                "--- a.rs\n+++ a.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    old();\n+    new();\n }\n"
                    .into(),
            sub_agent_text: String::new(),
            sub_agent_tools: Vec::new(),
        });
        st.messages.push(assistant);

        let diff = st.collect_last_turn_diff().unwrap();
        assert!(diff.contains("old()"));
        assert!(diff.contains("new()"));
    }

    #[test]
    fn collect_last_turn_diff_returns_none_when_empty() {
        let st = temp_state("last-turn-empty");
        assert!(st.collect_last_turn_diff().is_none());
    }

    #[test]
    fn rewind_rolls_back_edit_file_changes() {
        let dir = std::env::temp_dir().join(format!("mrorcode-rewind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let mut st = temp_state("rewind-edit");
        st.working_dir = dir.clone();
        // 第一轮 user + assistant（无文件修改）
        st.messages.push(MessageView::user(1, "first".into()));
        st.messages.push(MessageView::assistant(2));
        // 第二轮 user + assistant（含 edit 工具，把 world 改成 rust）
        st.messages.push(MessageView::user(3, "second".into()));
        let mut assistant = MessageView::assistant(4);
        assistant.tools.push(ToolView {
            id: "e1".into(),
            name: "edit_file".into(),
            arguments: r#"{"path":"test.txt","old":"world","new":"rust"}"#.into(),
            status: ToolStatus::Ok,
            output: "Edited test.txt (1 replacement(s))".into(),
            sub_agent_text: String::new(),
            sub_agent_tools: Vec::new(),
        });
        st.messages.push(assistant);

        // 模拟 edit 已执行：文件内容已变为 hello rust
        std::fs::write(&file, "hello rust").unwrap();

        // 回退到第一轮（keep_turns=1），应回滚第二轮的 edit
        assert!(st.rewind_projection(1));
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hello world");
        assert_eq!(st.messages.len(), 2); // 第一轮 user + assistant

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewind_removes_write_file_changes() {
        let dir = std::env::temp_dir().join(format!("mrorcode-rewind-w-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("new.txt");

        let mut st = temp_state("rewind-write");
        st.working_dir = dir.clone();
        // 第一轮 user + assistant（无文件修改）
        st.messages.push(MessageView::user(1, "first".into()));
        st.messages.push(MessageView::assistant(2));
        // 第二轮 user + assistant（含 write 工具）
        st.messages.push(MessageView::user(3, "second".into()));
        let mut assistant = MessageView::assistant(4);
        assistant.tools.push(ToolView {
            id: "w1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"new.txt","content":"hello"}"#.into(),
            status: ToolStatus::Ok,
            output: "Wrote new.txt".into(),
            sub_agent_text: String::new(),
            sub_agent_tools: Vec::new(),
        });
        st.messages.push(assistant);

        // 模拟 write 已执行：文件存在
        std::fs::write(&file, "hello").unwrap();
        assert!(file.exists());

        // 回退到第一轮，应删除第二轮 write 创建的文件
        assert!(st.rewind_projection(1));
        assert!(!file.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
