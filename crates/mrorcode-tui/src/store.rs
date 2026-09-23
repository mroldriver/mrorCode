//! Elm 式状态层：权威 agent 状态只在 runtime，这里只存投影与纯 UI 态。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tui_textarea::TextArea;

use mrorcode_agent::RuntimeHandle;
use mrorcode_kernel::event::AgentEvent;

use crate::widgets::markdown::MarkdownView;

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
    pub working_dir: PathBuf,
    pub sidebar: bool,

    pub messages: Vec<MessageView>,
    pub prompt: TextArea<'static>,
    /// 已提交历史（M1 内存）。
    pub history: Vec<String>,
    pub hist_idx: Option<usize>,
    /// 提交前的草稿（翻历史时暂存）。
    pub draft: String,

    pub permission: Option<PermissionRequest>,
    /// 粘底滚动；上翻后脱离。
    pub stick_bottom: bool,
    /// 像素行偏移（从底部计的回看行数）。
    pub backscroll: u16,
    /// 上一帧布局总行数（用于滚动边界）。
    pub last_total_lines: u16,
    pub viewport_lines: u16,

    /// Esc 双击中断的第一次时刻。
    pub esc_first: Option<Instant>,
    /// 短暂提示（如“再按一次 Esc 中断”）。
    pub notice: Option<(String, Instant)>,

    /// spinner 帧计数。
    pub spinner_ticks: u32,
}

impl AppState {
    pub fn new(model: String, working_dir: PathBuf) -> Self {
        let mut prompt = TextArea::new(vec![String::new()]);
        prompt.set_cursor_line_style(ratatui::style::Style::default());
        Self {
            route: Route::Home,
            phase: Phase::Idle,
            should_quit: false,
            model,
            working_dir,
            sidebar: true,
            messages: Vec::new(),
            prompt,
            history: Vec::new(),
            hist_idx: None,
            draft: String::new(),
            permission: None,
            stick_bottom: true,
            backscroll: 0,
            last_total_lines: 0,
            viewport_lines: 0,
            esc_first: None,
            notice: None,
            spinner_ticks: 0,
        }
    }

    pub fn notice_text(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, t)| t.elapsed() < Duration::from_secs(3))
            .map(|(t, _)| t.as_str())
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
        self.history.push(text.clone());
        self.hist_idx = None;
        self.draft.clear();

        self.messages.push(MessageView::user(text.clone()));
        self.route = Route::Session;
        self.phase = Phase::Working;
        self.stick_bottom = true;
        self.backscroll = 0;
        self.prompt.move_cursor(tui_textarea::CursorMove::Top);
        self.prompt.select_all();
        self.prompt.delete_char();
        handle.submit(text);
        true
    }

    pub fn on_runtime_event(&mut self, ev: AgentEvent, handle: &RuntimeHandle) {
        match ev {
            AgentEvent::TurnStarted => {
                self.phase = Phase::Working;
                self.messages.push(MessageView::assistant());
            }
            AgentEvent::TextDelta(t) => {
                if let Some(last) = self.messages.last_mut() {
                    last.text.push_str(&t);
                    last.doc.update(&last.text);
                    last.streaming = true;
                }
                self.phase = Phase::Working;
            }
            AgentEvent::Reasoning(t) => {
                if let Some(last) = self.messages.last_mut() {
                    last.reasoning.push_str(&t);
                }
            }
            AgentEvent::ToolStarted {
                id,
                name,
                arguments,
            } => {
                if let Some(last) = self.messages.last_mut() {
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
                if let Some(last) = self.messages.last_mut() {
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
            AgentEvent::Request { id, kind, payload } if kind == "approval" => {
                self.phase = Phase::WaitingApproval;
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
                    id,
                    tool,
                    arguments,
                });
            }
            AgentEvent::TurnComplete { .. } => {
                self.phase = Phase::Idle;
                self.finish_streaming();
            }
            AgentEvent::Cancelled => {
                self.phase = Phase::Idle;
                self.permission = None;
                self.finish_streaming();
                self.set_notice("已中断");
            }
            AgentEvent::Error { message, .. } => {
                self.phase = Phase::Idle;
                self.finish_streaming();
                if let Some(last) = self.messages.last_mut() {
                    last.is_error = true;
                    last.text = format!("{}\n[error] {message}", last.text);
                    last.doc.update(&last.text);
                }
                self.set_notice(format!("错误：{message}"));
            }
            AgentEvent::Usage { .. } => {}
            AgentEvent::Request { .. } => {}
        }
        let _ = handle;
    }

    pub fn resolve_permission(&mut self, decision: &str, handle: &RuntimeHandle) {
        if let Some(req) = self.permission.take() {
            let value = serde_json::json!({ "decision": decision });
            handle.respond(req.id, value);
        }
        self.phase = Phase::Working;
    }

    pub fn set_notice(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    fn finish_streaming(&mut self) {
        if let Some(last) = self.messages.last_mut() {
            last.streaming = false;
        }
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

impl MessageView {
    fn user(text: String) -> Self {
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
