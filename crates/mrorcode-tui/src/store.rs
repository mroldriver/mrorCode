//! Elm 式状态层：权威 agent 状态只在 runtime，这里只存投影与纯 UI 态。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tui_textarea::TextArea;

use mrorcode_agent::RuntimeHandle;
use mrorcode_kernel::event::AgentEvent;

use crate::commands;
use crate::keymap::Keymap;
use crate::storage::Store;
use crate::theme::{Mode, Theme, ThemeSet};
use crate::widgets::dialog::{
    AlertState, ConfirmAction, ConfirmState, Dialog, HelpRow, HelpState, SelectItem, SelectPurpose,
    SelectState,
};
use crate::widgets::markdown::MarkdownView;
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

#[derive(Clone, Copy, PartialEq, Eq)]
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
    pub role: Role,
    pub text: String,
    pub doc: MarkdownView,
    pub reasoning: String,
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

    pub messages: Vec<MessageView>,
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
    /// 短暂 toast 队列（新尾追加，3s 过期，最多 5 条）。
    pub toasts: Vec<(String, Instant)>,

    /// 已展开工具卡（按 tool id）；长输出默认折叠。
    pub expanded_tools: HashSet<String>,
    /// 当前聚焦工具卡 id（Tab 循环，Enter 展开/收起）。
    pub focused_tool: Option<String>,

    /// spinner 帧计数。
    pub spinner_ticks: u32,

    /// 键位表（默认 + KV 用户覆盖）。
    pub keymap: Keymap,
    /// leader 等待窗口截止时刻。
    pub leader_until: Option<Instant>,

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
            messages: Vec::new(),
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
            focused_tool: None,
            spinner_ticks: 0,
            keymap,
            leader_until: None,
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

    /// 推入一条 toast；与上一条相同则不重复，上限 5 条。
    pub fn toast(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.toasts.last().map(|(t, _)| t.as_str()) != Some(text.as_str()) {
            self.toasts.push((text, Instant::now()));
            if self.toasts.len() > 5 {
                let drop_n = self.toasts.len() - 5;
                self.toasts.drain(0..drop_n);
            }
        }
    }

    /// 清理过期 toast（由慢节拍定时调用）。
    pub fn gc_toasts(&mut self) {
        self.toasts
            .retain(|(_, t)| t.elapsed() < Duration::from_secs(3));
    }

    pub fn has_active_toasts(&self) -> bool {
        self.toasts
            .iter()
            .any(|(_, t)| t.elapsed() < Duration::from_secs(3))
    }

    /// 当前帧应显示的 toast（新→旧）。
    pub fn active_toasts(&self) -> impl Iterator<Item = &str> {
        self.toasts
            .iter()
            .rev()
            .filter(|(_, t)| t.elapsed() < Duration::from_secs(3))
            .map(|(s, _)| s.as_str())
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

        self.messages.push(MessageView::user(text.clone()));
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

    pub fn open_session_picker(&mut self) {
        let items = self
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
        let message = format!("删除会话「{title}」？仅进程内，退出即丢失。");
        self.push_dialog(Dialog::Confirm(ConfirmState::new(
            "delete session",
            message,
            ConfirmAction::DeleteSession(id),
        )));
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
                    self.toast(format!("错误：{message}"));
                }
                _ => {}
            }
            project_event(ev, &mut self.messages);
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
                project_event(ev, bucket);
            }
        }
        let _ = handle;
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

/// 纯消息投影：把 agent 事件追加进某个会话的消息视图（活动/后台桶通用）。
fn project_event(ev: AgentEvent, messages: &mut Vec<MessageView>) {
    match ev {
        AgentEvent::TurnStarted => messages.push(MessageView::assistant()),
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
    pub(crate) fn user(text: String) -> Self {
        let mut doc = MarkdownView::new();
        doc.update(&text);
        Self {
            role: Role::User,
            text,
            doc,
            reasoning: String::new(),
            tools: Vec::new(),
            is_error: false,
            streaming: false,
        }
    }

    pub(crate) fn assistant() -> Self {
        Self {
            role: Role::Assistant,
            text: String::new(),
            doc: MarkdownView::new(),
            reasoning: String::new(),
            tools: Vec::new(),
            is_error: false,
            streaming: true,
        }
    }
}
