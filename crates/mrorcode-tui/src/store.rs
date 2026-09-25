//! Elm 式状态层：权威 agent 状态只在 runtime，这里只存投影与纯 UI 态。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tui_textarea::TextArea;

use mrorcode_agent::{RuntimeHandle, SessionStore};
use mrorcode_kernel::event::AgentEvent;
use mrorcode_kernel::message::{Conversation, Message, ToolCall};
use mrorcode_kernel::session::{SessionRecord, SessionSummary};

use crate::commands;
use crate::keymap::Keymap;
use crate::storage::Store;
use crate::theme::{Mode, Theme, ThemeSet};
use crate::widgets::dialog::{
    AlertState, ConfirmAction, ConfirmState, Dialog, HelpRow, HelpState, SelectItem, SelectPurpose,
    SelectState,
};
use crate::widgets::markdown::MarkdownView;
use crate::widgets::toast::ToastLevel;
use crate::widgets::tool_card;

/// 侧栏会话列表条目（进程内；磁盘持久化归 M3）。
pub struct SessionMeta {
    pub id: u64,
    pub title: String,
    pub phase: Phase,
}

/// 首个会话的固定 id；后续由 next_session_id 递增。
pub const DEFAULT_SESSION: u64 = 1;

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

#[derive(Clone, Copy)]
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
    pub working_dir: PathBuf,
    pub sidebar: bool,
    /// 多会话标签栏是否显示（M4-4）。
    pub show_tabs: bool,

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
    pub prompt: TextArea<'static>,
    /// 已提交历史（M1 内存）。
    pub history: Vec<String>,
    pub hist_idx: Option<usize>,
    /// 提交前的草稿（翻历史时暂存）。
    pub draft: String,

    pub permission: Option<PermissionRequest>,
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
    /// 短暂 toast 队列（新尾追加，3s 过期，最多 5 条）；带级别（M4-11）。
    pub toasts: Vec<(String, Instant, crate::widgets::toast::ToastLevel)>,

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

    /// 键位表（默认 + KV 用户覆盖）。
    pub keymap: Keymap,
    /// leader 等待窗口截止时刻。
    pub leader_until: Option<Instant>,

    /// plan 模式（只读 gate）；底栏徽标 + 提交时告知 kernel。
    pub plan_mode: bool,

    /// `@`/`/` 补全浮层状态（None 表示未激活）。
    pub completion: Option<crate::input::Completion>,

    /// 标签栏命中区域（每帧重建，M4-4/5）。
    pub tab_hits: Vec<crate::widgets::tabs::TabHit>,
    /// 鼠标命中区域（每帧重建，M4-5）。
    pub hit_regions: Vec<crate::hit::HitRegion>,
    /// 选区（M4-6）；键盘 `v` 或鼠标拖拽进入。
    pub selection: Option<crate::selection::Selection>,
    /// 键盘选择模式（modal，M4-6）：方向键扩展选区、y 复制、esc 取消。
    pub select_mode: bool,
    /// 上一帧的屏幕文本行（选区提取用，M4-6）。
    pub screen_rows: Vec<String>,

    /// 累计 token 用量（M4-8 Context 分区）。
    pub usage_input_tokens: u64,
    pub usage_output_tokens: u64,
    /// 是否发生过上下文压缩（投影里出现 `[compacted` 标记，M4-8）。
    pub compacted: bool,
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

    /// 当前生效主题。
    pub theme: Theme,
    pub theme_name: String,
    pub theme_mode: Mode,
    pub themes_dark: ThemeSet,
    pub themes_light: ThemeSet,
    /// 主题代际：每次切换主题/模式 +1，用于让 markdown 高亮缓存失效。
    pub theme_gen: u64,
}

impl AppState {
    pub fn new(
        model: String,
        available_models: Vec<String>,
        working_dir: PathBuf,
        kv: Store,
    ) -> Self {
        let sidebar = kv.data.sidebar.unwrap_or(true);
        let show_tabs = kv.data.tabs.unwrap_or(true);
        let history = kv.data.prompt_history.clone();
        let theme_mode = Mode::parse(&kv.data.theme_mode);
        // KV 记住的模型在可选列表内时恢复（kernel 侧由调用方补发 SetModel）
        let model = match &kv.data.model {
            Some(remembered) if available_models.iter().any(|m| m == remembered) => {
                remembered.clone()
            }
            _ => model,
        };
        let themes_dark = ThemeSet::bundled(Mode::Dark);
        let themes_light = ThemeSet::bundled(Mode::Light);
        let theme_name = kv
            .data
            .theme
            .clone()
            .filter(|n| match theme_mode {
                Mode::Dark => themes_dark.get(n).is_some(),
                Mode::Light => themes_light.get(n).is_some(),
            })
            .unwrap_or_else(|| "tokyonight".to_string());
        let theme = match theme_mode {
            Mode::Dark => themes_dark.get(&theme_name),
            Mode::Light => themes_light.get(&theme_name),
        }
        .cloned()
        .unwrap_or_else(Theme::tokyonight);

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
            working_dir,
            sidebar,
            show_tabs,
            messages: Vec::new(),
            next_message_uid: 1,
            sessions: vec![SessionMeta {
                id: DEFAULT_SESSION,
                title: "新会话".to_string(),
                phase: Phase::Idle,
            }],
            active_session: DEFAULT_SESSION,
            next_session_id: DEFAULT_SESSION + 1,
            buckets: HashMap::new(),
            prompt,
            history,
            hist_idx: None,
            draft: String::new(),
            permission: None,
            dialogs: Vec::new(),
            stick_bottom: true,
            backscroll: 0,
            last_total_lines: 0,
            viewport_lines: 0,
            esc_first: None,
            toasts: Vec::new(),
            expanded_tools: HashSet::new(),
            expanded_reasoning: HashSet::new(),
            focused_reasoning: None,
            focused_tool: None,
            spinner_ticks: 0,
            keymap,
            leader_until: None,
            plan_mode: false,
            completion: None,
            tab_hits: Vec::new(),
            hit_regions: Vec::new(),
            selection: None,
            select_mode: false,
            screen_rows: Vec::new(),
            usage_input_tokens: 0,
            usage_output_tokens: 0,
            compacted: false,
            touched_files: Vec::new(),
            collapsed_sections: HashSet::new(),
            session_store: None,
            history_sessions: Vec::new(),
            kv,
            theme,
            theme_name,
            theme_mode,
            themes_dark,
            themes_light,
            theme_gen: 0,
        }
    }

    /// 切换主题（名称为去 .json 的文件名）；成功返回 true。
    pub fn set_theme(&mut self, name: &str) -> bool {
        let set = match self.theme_mode {
            Mode::Dark => &self.themes_dark,
            Mode::Light => &self.themes_light,
        };
        match set.get(name) {
            Some(t) => {
                self.theme = t.clone();
                self.theme_name = name.to_string();
                self.theme_gen = self.theme_gen.wrapping_add(1);
                self.kv.data.theme = Some(name.to_string());
                let _ = self.kv.save();
                true
            }
            None => false,
        }
    }

    /// 切换 dark/light；当前主题在另一模式不存在时回退 tokyonight。
    pub fn set_theme_mode(&mut self, mode: Mode) {
        if mode == self.theme_mode {
            return;
        }
        self.theme_mode = mode;
        if !self.set_theme(&self.theme_name.clone()) {
            self.set_theme("tokyonight");
        }
        // set_theme 已 bump gen 并保存；这里只需补齐 mode 字段
        self.kv.data.theme_mode = mode.as_str().to_string();
        let _ = self.kv.save();
    }
    pub fn toggle_sidebar(&mut self) {
        self.sidebar = !self.sidebar;
        self.kv.data.sidebar = Some(self.sidebar);
        let _ = self.kv.save();
    }

    /// 切换标签栏显示（M4-4）。
    pub fn toggle_tabs(&mut self) {
        self.show_tabs = !self.show_tabs;
        self.kv.data.tabs = Some(self.show_tabs);
        let _ = self.kv.save();
    }

    /// 切换 plan 模式并通知 kernel（只读 gate）。
    pub fn toggle_plan_mode(&mut self, handle: &RuntimeHandle) {
        self.plan_mode = !self.plan_mode;
        handle.set_plan_mode(self.plan_mode);
        if self.plan_mode {
            self.toast("plan 模式：仅只读工具，写操作将被拒绝");
        } else {
            self.toast("已退出 plan 模式");
        }
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

    /// 打开主题选择器（列出当前模式下全部成功解析的主题）。
    pub fn open_theme_picker(&mut self) {
        let names: Vec<String> = match self.theme_mode {
            Mode::Dark => self.themes_dark.names().map(str::to_string).collect(),
            Mode::Light => self.themes_light.names().map(str::to_string).collect(),
        };
        let items = names
            .into_iter()
            .map(|n| SelectItem {
                hint: if n == self.theme_name {
                    "active".to_string()
                } else {
                    String::new()
                },
                group: "Themes".to_string(),
                label: n.clone(),
                value: n,
            })
            .collect();
        self.push_dialog(Dialog::Select(SelectState::new(
            "switch theme",
            SelectPurpose::Theme,
            items,
        )));
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
        let _ = self.kv.save();
        true
    }

    /// 推入一条信息 toast；与上一条相同则不重复，上限 5 条。
    pub fn toast(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), ToastLevel::Info);
    }

    /// 推入一条警告 toast（左边框 warning 色）。
    pub fn toast_warn(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), ToastLevel::Warn);
    }

    /// 推入一条错误 toast（左边框 error 色）。
    pub fn toast_error(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), ToastLevel::Error);
    }

    fn push_toast(&mut self, text: String, level: ToastLevel) {
        if self.toasts.last().map(|(t, _, _)| t.as_str()) != Some(text.as_str()) {
            self.toasts.push((text, Instant::now(), level));
            if self.toasts.len() > 5 {
                let drop_n = self.toasts.len() - 5;
                self.toasts.drain(0..drop_n);
            }
        }
    }

    /// 清理过期 toast（由慢节拍定时调用）。
    pub fn gc_toasts(&mut self) {
        self.toasts
            .retain(|(_, t, _)| t.elapsed() < Duration::from_secs(3));
    }

    pub fn has_active_toasts(&self) -> bool {
        self.toasts
            .iter()
            .any(|(_, t, _)| t.elapsed() < Duration::from_secs(3))
    }

    /// 当前帧应显示的 toast（新→旧），携带级别（M4-11）。
    pub fn active_toasts(&self) -> impl Iterator<Item = (ToastLevel, &str)> {
        self.toasts
            .iter()
            .rev()
            .filter(|(_, t, _)| t.elapsed() < Duration::from_secs(3))
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
        });
        self.active_session = id;
        self.messages.clear();
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
        self.reset_session_view();
        true
    }

    /// 删除会话（仅进程内，M3 接磁盘时会有真正的 unlink 确认）。
    pub fn delete_session(&mut self, id: u64) {
        self.buckets.remove(&id);
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
                self.reset_session_view();
            }
            None => {
                let id = self.next_session_id;
                self.next_session_id += 1;
                self.sessions.push(SessionMeta {
                    id,
                    title: "新会话".to_string(),
                    phase: Phase::Idle,
                });
                self.active_session = id;
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
                group: "Sessions".to_string(),
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

    /// 当前活动消息收桶（空消息不占桶）。
    fn stash_active(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        self.buckets
            .insert(self.active_session, std::mem::take(&mut self.messages));
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
        self.messages.truncate(cutoff);
        // 回卷滚动状态到最底
        self.stick_bottom = true;
        self.backscroll = 0;
        self.clear_tool_focus();
        self.persist_active_session();
        self.toast("已回退；仅当前会话生效，磁盘 transcript 保留完整历史");
        true
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
                }
                AgentEvent::TurnComplete { .. } => self.set_phase(Phase::Idle),
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
        assert!(!st.is_section_collapsed("Files"));
        st.toggle_section("Files");
        assert!(st.is_section_collapsed("Files"));
        st.toggle_section("Files");
        assert!(!st.is_section_collapsed("Files"));
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
    fn toast_levels_are_kept_and_deduped() {
        let mut st = temp_state("toast-levels");
        st.toast("info a");
        st.toast_warn("warn b");
        st.toast_error("err c");
        // 与当前最后一条相同则不重复
        st.toast_error("err c");
        let active: Vec<_> = st.active_toasts().collect();
        assert_eq!(active.len(), 3);
        assert_eq!(active[0], (ToastLevel::Error, "err c"));
        assert_eq!(active[1], (ToastLevel::Warn, "warn b"));
        assert_eq!(active[2], (ToastLevel::Info, "info a"));
        // 级别为 Error 的错误类 toast 不应回退为 Info
        let mut st2 = temp_state("toast-levels-2");
        st2.toast_error("boom");
        assert_eq!(
            st2.active_toasts().next(),
            Some((ToastLevel::Error, "boom"))
        );
    }
}
