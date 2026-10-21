//! TUI 应用：终端生命周期 + 脏标记驱动事件循环。
//!
//! 渲染节拍：无固定帧率 tick。事件只置脏，首个脏信号启动 8ms 尾沿定时器，
//! 期间 delta 全部合并成一帧；空闲时 select 完全阻塞（0 唤醒）。
//! spinner 仅在 busy 时启用 100ms 慢节拍。

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Widget};
use ratatui::{Frame, Terminal};
use std::pin::Pin;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Interval, Sleep};
use tui_textarea::TextArea;

use mrorcode_agent::{AssembleOptions, Assembly, RuntimeEvent};

use crate::commands::{self, AppCommand};
use crate::store::{AppState, Route};
use crate::theme::Theme;
use crate::widgets::dialog::{self, DialogOutcome, SelectPurpose};
use crate::widgets::{home, permission, prompt, session, sidebar, toast, whichkey};

const RENDER_COALESCE: Duration = Duration::from_millis(8);
const SPINNER_TICK: Duration = Duration::from_millis(100);
/// 输入光标亮/灭半周期（应用层驱动闪烁）。
const CURSOR_BLINK_TICK: Duration = Duration::from_millis(600);
/// 关停旧 runtime 时等待 Exited 的兜底超时。
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// 输入线程 → 主循环的统一输入事件（M4：键/粘贴分流）。
#[derive(Clone, Debug)]
pub enum InputEvent {
    Key(KeyEvent),
    /// 终端粘贴（crossterm bracketed paste），文本原样交给输入框。
    Paste(String),
    /// 终端窗口获得焦点。
    FocusGained,
    /// 终端窗口失去焦点。
    FocusLost,
}

/// TUI 入口。`resume` 为要恢复的会话 id（来自 `--resume`）。
pub async fn run(
    assembly: Assembly,
    resume: Option<u64>,
    config: mrorcode_config::Config,
    options: AssembleOptions,
) -> anyhow::Result<()> {
    let Assembly {
        mut handle,
        mut events,
        provider_name,
        model,
        available_models,
        mcp,
        ..
    } = assembly;
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // 启动期一次性探测：MCP server 列表 + 当前 git 分支（仅用于展示）。
    let mcp_server_names: Vec<String> = mcp
        .as_ref()
        .map(|b| b.servers.iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default();
    let git_branch_current = detect_git_branch(&working_dir);

    let mut terminal = setup_terminal()?;
    install_panic_restore();

    let (kv, kv_warnings) = crate::storage::Store::load();
    for w in kv_warnings {
        tracing::warn!("state: {w}");
    }

    // KV 中记住的模型选择：AppState 已恢复为初始模型；
    // 若与 config 默认模型不同，补发 SetModel（kernel 下一轮生效）
    let assembly_model = model.clone();
    let mut state = AppState::new(model, available_models, working_dir.clone(), kv);
    state.attach_config(config, options, provider_name);
    state.mcp_servers = mcp_server_names;
    state.git_branch = git_branch_current;
    if state.model != assembly_model {
        let m = state.model.clone();
        handle.set_model(m.clone());
        state.toast(format!("模型：{m}（下一轮生效）"));
    }
    // 磁盘历史会话：装入侧栏；--resume 指定的会话直接恢复投影 + 注入 kernel。
    state.attach_session_store(mrorcode_agent::SessionStore::standard());
    if let Some(id) = resume {
        if let Some(record) = state.load_history_session(id) {
            handle.resume(id, record.conversation);
            state.toast(format!("已恢复会话 {id}"));
        } else {
            state.toast(format!("未找到会话 {id}"));
        }
    }
    let initial_theme = state.theme.clone();
    prompt::style_textarea(&mut state, &initial_theme);

    let (_input_thread, mut input_rx) = spawn_input_thread();

    let mut render_due: Option<Pin<Box<Sleep>>> = None;
    let mut spinner: Option<Interval> = None;
    let mut blinker: Option<Interval> = None;
    let mut dirty = true; // 首帧必画

    let result: io::Result<()> = loop {
        // 待执行的 provider 切换：在事件循环顶部统一处理（M5-e）。
        if state.pending_provider_switch.is_some() {
            apply_provider_switch(&mut state, &mut handle, &mut events).await;
            dirty = true;
        }
        // 外部编辑器：恢复终端 → 编辑临时文件 → 回读 → 重建终端
        if state.pending_external_editor {
            state.pending_external_editor = false;
            run_external_editor(&mut state, &mut terminal);
            dirty = true;
        }
        ensure_wake(&mut spinner, &mut blinker, &mut state);
        tokio::select! {
            biased;

            maybe_input = input_rx.recv() => {
                match maybe_input {
                    Some(input) => {
                        if prompt_focused(&state) {
                            reset_cursor_blink(&mut blinker, &mut state);
                        }
                        if on_input(&mut state, &input, &handle) {
                            dirty = true;
                        }
                    }
                    None => break Ok(()),
                }
            }

            ev = events.recv() => {
                match ev {
                    Some(RuntimeEvent::Agent { session_id, event: agent_ev }) => {
                        state.on_runtime_event(session_id, agent_ev, &handle);
                        dirty = true;
                    }
                    Some(RuntimeEvent::Exited) => {
                        state.toast_error("runtime exited");
                        dirty = true;
                    }
                    // 通道关闭：runtime 已终止，退出主循环，避免空转
                    None => break Ok(()),
                }
            }

            _ = async {
                if let Some(i) = spinner.as_mut() {
                    i.tick().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if spinner.is_some() => {
                if state.phase.is_busy() {
                    state.spinner_ticks = state.spinner_ticks.wrapping_add(1);
                }
                state.gc_toasts();
                dirty = true;
            }

            _ = async {
                if let Some(b) = blinker.as_mut() {
                    b.tick().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if blinker.is_some() => {
                // 只翻转标志；光标样式在下一帧 draw 中统一同步
                state.cursor_blink_on = !state.cursor_blink_on;
                dirty = true;
            }

            _ = async {
                if let Some(s) = render_due.as_mut() {
                    s.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if render_due.is_some() && dirty => {
                if state.should_quit {
                    break Ok(());
                }
                if let Err(e) = terminal.draw(|f| draw(f, &mut state)) {
                    break Err(e);
                }
                render_due = None;
                dirty = false;
            }
        }

        if dirty && render_due.is_none() {
            render_due = Some(Box::pin(tokio::time::sleep(RENDER_COALESCE)));
        }
        if state.should_quit && render_due.is_none() {
            break Ok(());
        }
    };

    shutdown_terminal();
    handle.shutdown();
    result?;
    Ok(())
}

// ---------------- provider 切换（M5-e） ----------------

/// 执行待处理的 provider 切换。
///
/// 顺序：先构建新 Assembly（失败则旧 runtime 原样保留）→ 关停旧的 →
/// 替换句柄 → resume 当前会话。busy 时拒绝切换。
async fn apply_provider_switch(
    state: &mut AppState,
    handle: &mut mrorcode_agent::RuntimeHandle,
    events: &mut UnboundedReceiver<RuntimeEvent>,
) {
    let Some(target) = state.pending_provider_switch.take() else {
        return;
    };
    if state.phase.is_busy() {
        state.toast_warn("请等当前任务结束后再切换 provider");
        return;
    }
    if target == state.current_provider {
        return;
    }

    // 先把当前会话落盘（保证 snapshot 最新），并取出会话体。
    state.persist_active_session();
    let session_id = state.active_session;
    let conversation = state.active_conversation();

    // 1) 先构建新 runtime；失败时旧 runtime 原样保留。
    let new_assembly = mrorcode_agent::rebuild_assembly(
        &state.config,
        &target,
        state.working_dir.clone(),
        &state.assemble_options,
    )
    .await;
    let Assembly {
        handle: new_handle,
        events: new_events,
        provider_name,
        model: new_model,
        available_models: new_available,
        ..
    } = match new_assembly {
        Ok(a) => a,
        Err(e) => {
            state.toast_error(format!("切换失败（仍用原 provider）：{e}"));
            return;
        }
    };

    // 2) 关停旧 runtime 并排空事件直到 Exited（带超时兜底）。
    handle.shutdown();
    drain_until_exited(events).await;

    // 3) 替换句柄。
    *handle = new_handle;
    *events = new_events;

    // 4) 恢复当前会话体到新 runtime。
    handle.resume(session_id, conversation);

    // 5) 更新投影；权限模式需重新下发给新 kernel。
    state.note_provider_switched(provider_name.clone(), new_model.clone(), new_available);
    handle.set_mode(state.mode);
    prompt::style_textarea(state, &state.theme.clone());
    state.toast(format!(
        "已切换 provider：{provider_name} · 模型 {new_model}"
    ));
}

/// 排空旧 runtime 事件直到 Exited / 通道关闭；超时也返回（旧句柄将被替换）。
async fn drain_until_exited(events: &mut UnboundedReceiver<RuntimeEvent>) {
    let deadline = tokio::time::sleep(SHUTDOWN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            ev = events.recv() => {
                match ev {
                    Some(RuntimeEvent::Exited) | None => return,
                    Some(_) => continue,
                }
            }
            _ = &mut deadline => return,
        }
    }
}

// ---------------- 输入 ----------------

fn spawn_input_thread() -> (std::thread::JoinHandle<()>, UnboundedReceiver<InputEvent>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    // 阻塞式 crossterm 读取线程（poll/read 不占 CPU）
    let handle = std::thread::spawn(move || loop {
        let input = match event::read() {
            // Windows 控制台对同一物理键会同时下发 Press（按下）与 Release（松开），
            // 且长按自动重复也全部以 Press 上报（无 Repeat 类型）。
            // 若不过滤，一次方向键敲击会被 on_key 执行两次（按下一次 + 松开一次）。
            Ok(event::Event::Key(k)) if k.kind != KeyEventKind::Press => continue,
            Ok(event::Event::Key(k)) => InputEvent::Key(k),
            Ok(event::Event::Paste(s)) => InputEvent::Paste(s),
            Ok(event::Event::FocusGained) => InputEvent::FocusGained,
            Ok(event::Event::FocusLost) => InputEvent::FocusLost,
            Ok(_) => continue,
            Err(_) => break,
        };
        if tx.send(input).is_err() {
            break;
        }
    });
    (handle, rx)
}

/// 统一输入分派；返回是否需要重绘。
fn on_input(
    state: &mut AppState,
    input: &InputEvent,
    handle: &mrorcode_agent::RuntimeHandle,
) -> bool {
    match input {
        InputEvent::Key(key) => on_key(state, key, handle),
        InputEvent::Paste(text) => on_paste(state, text),
        InputEvent::FocusGained => {
            state.terminal_focused = true;
            false
        }
        InputEvent::FocusLost => {
            state.terminal_focused = false;
            false
        }
    }
}

/// 粘贴：规范化 CR/CRLF 后插入输入框（M2 clipboard 风险表）。
fn on_paste(state: &mut AppState, text: &str) -> bool {
    if state.permission.is_some() || !state.dialogs.is_empty() {
        return false;
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    // 检测单文件路径粘贴（图片/PDF）：自动转为 @path 附件引用
    let trimmed = normalized.trim();
    if is_attachment_path(trimmed) {
        let reference = format!("@{trimmed} ");
        state.prompt.insert_str(&reference);
        state.refresh_completion();
        state.toast(format!("已附加文件：{trimmed}"));
        return true;
    }
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            state.prompt.insert_newline();
        }
        state.prompt.insert_str(line);
    }
    state.refresh_completion();
    true
}

/// 判断粘贴的文本是否是单个图片/PDF 文件路径（用于自动附加）。
fn is_attachment_path(text: &str) -> bool {
    if text.is_empty() || text.contains('\n') {
        return false;
    }
    let lower = text.to_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
        || lower.ends_with(".bmp")
        || lower.ends_with(".svg")
        || lower.ends_with(".pdf")
}

/// 键盘选择模式：方向键移动光标、y 复制、esc/q 退出（M4-6）。
fn handle_select_key(state: &mut AppState, key: &KeyEvent) -> bool {
    // 初始进入时，若尚无选区则以屏幕中心为锚点
    if state.selection.is_none() {
        let (cx, cy) = (state.viewport_lines.max(1) / 2, 0);
        state.begin_selection(cx, cy);
    }
    let pos = state.selection.map(|s| s.cursor);
    let move_by = |state: &mut AppState, dx: i32, dy: i32| {
        if let Some(p) = pos {
            let nx = (p.x as i32 + dx).max(0) as u16;
            let ny = (p.y as i32 + dy).max(0) as u16;
            state.extend_selection(nx, ny);
        }
    };
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.clear_selection();
            state.select_mode = false;
        }
        KeyCode::Char('y') | KeyCode::Char('c') => {
            if state.copy_selection() {
                state.toast("已复制选中内容");
            } else {
                state.toast("没有可复制的内容");
            }
            state.clear_selection();
            state.select_mode = false;
        }
        KeyCode::Up | KeyCode::Char('k') => move_by(state, 0, -1),
        KeyCode::Down | KeyCode::Char('j') => move_by(state, 0, 1),
        KeyCode::Left | KeyCode::Char('h') => move_by(state, -1, 0),
        KeyCode::Right | KeyCode::Char('l') => move_by(state, 1, 0),
        _ => {}
    }
    true
}

/// 处理一个按键；返回是否需要重绘。
fn on_key(state: &mut AppState, key: &KeyEvent, handle: &mrorcode_agent::RuntimeHandle) -> bool {
    // modal 层：审批弹窗优先吃键（横向导航 + 回车确认，reject 阶段可输入原因）
    if state.permission.is_some() {
        let stage = state.permission.as_ref().map(|r| r.stage);

        // reject 阶段：字符直接输入到 reject_message
        if stage == Some(crate::store::PermissionStage::Reject) {
            match key.code {
                KeyCode::Enter => state.permission_confirm(handle),
                KeyCode::Esc => state.permission_escape(handle),
                KeyCode::Backspace => {
                    if let Some(req) = state.permission.as_mut() {
                        req.reject_message.pop();
                    }
                    return true;
                }
                KeyCode::Char(c) => {
                    if let Some(req) = state.permission.as_mut() {
                        req.reject_message.push(c);
                    }
                    return true;
                }
                _ => return false,
            }
            return true;
        }

        // permission / always_confirm 阶段：横向导航
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                state.permission_cycle(-1);
                return true;
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                state.permission_cycle(1);
                return true;
            }
            KeyCode::Enter => state.permission_confirm(handle),
            KeyCode::Esc => state.permission_escape(handle),
            // 保留旧快捷键兼容：y=本次允许, a=始终允许, n=拒绝
            KeyCode::Char('y') if stage == Some(crate::store::PermissionStage::Permission) => {
                state.permission_cursor = 0;
                state.permission_confirm(handle);
            }
            KeyCode::Char('a') if stage == Some(crate::store::PermissionStage::Permission) => {
                state.permission_cursor = 1;
                state.permission_confirm(handle);
            }
            KeyCode::Char('n') if stage == Some(crate::store::PermissionStage::Permission) => {
                state.permission_cursor = 2;
                state.permission_confirm(handle);
            }
            _ => return false,
        }
        return true;
    }

    // modal 层：对话框栈顶吃键，base 层不参与
    if !state.dialogs.is_empty() {
        let outcome = dialog::key_top(state.dialogs.last_mut().unwrap(), key);
        return match outcome {
            DialogOutcome::None => true,
            DialogOutcome::Closed => {
                state.pop_dialog();
                true
            }
            DialogOutcome::Accepted(purpose, value) => {
                state.pop_dialog();
                apply_select(state, purpose, value, handle);
                true
            }
            DialogOutcome::Confirmed(action) => {
                state.pop_dialog();
                apply_confirm(state, action);
                true
            }
            DialogOutcome::InputSubmitted(purpose, text) => {
                state.pop_dialog();
                apply_input(state, purpose, text);
                true
            }
            DialogOutcome::TimelineRewind(msg_index) => {
                state.pop_dialog();
                // timeline 回退：保留选中消息及之前的内容
                // 计算需要保留的 user turn 数（与 kernel rewind 语义一致）
                let keep_turns = state.messages[..=msg_index]
                    .iter()
                    .filter(|m| matches!(m.role, crate::store::Role::User))
                    .count();
                if keep_turns > 0 {
                    state.rewind_to(keep_turns, handle);
                    state.toast(format!("已回退到消息 #{msg_index} 之后"));
                }
                true
            }
        };
    }

    // modal 层：键盘选择模式（M4-6）
    if state.select_mode {
        return handle_select_key(state, key);
    }

    // Esc 先解除工具卡焦点，再走中断/退出（在 keymap 之前拦截）
    if state.focused_tool.is_some() && key.code == KeyCode::Esc {
        state.clear_tool_focus();
        return true;
    }

    // 补全浮层（modal 语义）：只在 base 层（无对话框/审批）时活跃
    if state.completion.as_ref().is_some_and(|c| c.visible()) {
        match key.code {
            KeyCode::Up => {
                if let Some(c) = state.completion.as_mut() {
                    c.move_selection(-1);
                }
                return true;
            }
            KeyCode::Down => {
                if let Some(c) = state.completion.as_mut() {
                    c.move_selection(1);
                }
                return true;
            }
            KeyCode::Tab | KeyCode::Enter => {
                // 若当前行已是完整可解析的斜杠命令，不再当作“再接受一次补全”，
                // 而是关闭弹层并落到 Submit（如 /quit 后回车直接执行，避免被补全弹层吞掉）。
                if key.code == KeyCode::Enter {
                    let raw = state.prompt.lines().join("\n");
                    let trimmed = raw.trim();
                    if trimmed.starts_with('/') && commands::parse_slash_command(trimmed).is_some()
                    {
                        state.clear_completion();
                        return false;
                    }
                }
                if state.accept_completion() {
                    return true;
                }
            }
            KeyCode::Esc => {
                state.clear_completion();
                return true;
            }
            _ => {}
        }
    }

    let now = std::time::Instant::now();
    let leader_active = state.leader_active(now);

    // ctrl+x 触发 leader；窗口内的 ctrl+x 当作未识别第二键处理
    let is_leader_trigger =
        key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL);
    if is_leader_trigger && !leader_active {
        state.arm_leader(now);
        state.toast("leader… 2s 内按组合键");
        return true;
    }

    // base 层统一走键位表（用户覆盖在此生效）
    if let Some(action) = state
        .keymap
        .resolve(crate::keymap::Layer::Base, key, leader_active)
    {
        if leader_active {
            state.clear_leader();
        }
        if apply_action(state, action, handle) {
            return true;
        }
        // 动作声明未消费（如历史浏览光标不在边界）：落给输入框
    } else if leader_active {
        // leader 第二键无绑定：清窗口并吞键，避免误输入/误退出
        state.clear_leader();
        return true;
    }

    // 普通编辑键交给 textarea
    state.esc_first = None;
    let consumed = state.prompt.input(*key);
    state.refresh_completion();
    consumed
}

/// 执行 base 层键位动作；返回 false 表示未消费（按键继续交给 textarea）。
fn apply_action(
    state: &mut AppState,
    action: crate::keymap::Action,
    handle: &mrorcode_agent::RuntimeHandle,
) -> bool {
    use crate::keymap::Action::*;
    match action {
        Submit => {
            // 焦点在工具卡上时 Enter 展开/收起，否则发送
            if state.toggle_focused_tool() {
                return true;
            }
            // 斜杠命令：提交时解析执行，不作为用户消息发送
            let raw = state.prompt.lines().join("\n");
            let trimmed = raw.trim();
            if trimmed.starts_with('/') {
                if state.phase.is_busy() {
                    state.toast_warn("忙碌中无法执行命令");
                    return true;
                }
                match commands::parse_slash_command(trimmed) {
                    Some(cmd) => {
                        execute_command(state, cmd, handle);
                    }
                    None => {
                        state.toast_error(format!("未知命令：{trimmed}"));
                    }
                }
                state.prompt.select_all();
                state.prompt.delete_char();
                state.draft.clear();
                return true;
            }
            state.submit_prompt(handle)
        }
        Newline => {
            state.prompt.insert_newline();
            true
        }
        CancelOrClear => {
            if state.phase.is_busy() {
                let now = std::time::Instant::now();
                let double = state
                    .esc_first
                    .is_some_and(|t| now.duration_since(t) < Duration::from_secs(5));
                if double {
                    handle.cancel();
                    state.esc_first = None;
                    state.toast("正在中断…");
                } else {
                    state.esc_first = Some(now);
                    state.toast("再按一次 Esc 中断");
                }
            }
            true
        }
        Quit => {
            // 输入非空时先清空（M1 行为），再次 ctrl+c 才退出
            if input_is_empty(state) {
                state.should_quit = true;
            } else {
                state.prompt.select_all();
                state.prompt.delete_char();
            }
            true
        }
        Rewind => {
            state.open_rewind_picker();
            true
        }
        PinSession => {
            state.toggle_pin_active();
            let pinned = state
                .sessions
                .iter()
                .find(|s| s.id == state.active_session)
                .map(|s| s.pinned)
                .unwrap_or(false);
            state.toast(if pinned {
                "已固定会话"
            } else {
                "已取消固定"
            });
            true
        }
        RenameSession => {
            state.open_rename_dialog();
            true
        }
        ExportTranscript => match state.export_transcript() {
            Ok(path) => {
                state.toast(format!("已导出：{}", path.display()));
                true
            }
            Err(e) => {
                state.open_alert("导出失败", e.to_string());
                true
            }
        },
        WidenSidebar => {
            state.adjust_sidebar_width(4);
            true
        }
        NarrowSidebar => {
            state.adjust_sidebar_width(-4);
            true
        }
        OpenExternalEditor => {
            state.pending_external_editor = true;
            true
        }
        ScrollUp => {
            // 输入框为空才滚聊天；有内容时落给 textarea 移光标（多行编辑）
            if state.route == Route::Session && input_is_empty(state) {
                state.scroll_up(state.viewport_lines / 2 + 1);
                true
            } else {
                false
            }
        }
        ScrollDown => {
            if state.route == Route::Session && input_is_empty(state) {
                state.scroll_down(state.viewport_lines / 2 + 1);
                true
            } else {
                false
            }
        }
        HistoryPrev => history_prev(state),
        HistoryNext => history_next(state),
        FocusNextTool => {
            state.cycle_tool_focus(true);
            true
        }
        FocusPrevTool => {
            state.cycle_tool_focus(false);
            true
        }
        CycleAgent => {
            state.cycle_agent(false, handle);
            true
        }
        CycleAgentReverse => {
            state.cycle_agent(true, handle);
            true
        }
        CycleModel => {
            state.cycle_model(false, handle);
            true
        }
        CycleModelReverse => {
            state.cycle_model(true, handle);
            true
        }
        ToggleWhichKey => {
            state.toggle_which_key();
            true
        }
        ToggleWhichKeyLayout => {
            state.toggle_which_key_layout();
            true
        }
        // 面板导航键仅在面板可见时生效（opencode：绑定随 visible() 启停）
        WhichKeyGroupPrev | WhichKeyGroupNext | WhichKeyScrollUp | WhichKeyScrollDown
        | WhichKeyPageUp | WhichKeyPageDown | WhichKeyHome | WhichKeyEnd => {
            if !state.which_key_visible() {
                return true;
            }
            let (w, h) = whichkey_geometry(state);
            match action {
                WhichKeyGroupPrev => state.which_key_group_delta(-1),
                WhichKeyGroupNext => state.which_key_group_delta(1),
                WhichKeyScrollUp => state.which_key_scroll(w, h, -(whichkey::columns(w) as i32)),
                WhichKeyScrollDown => state.which_key_scroll(w, h, whichkey::columns(w) as i32),
                WhichKeyPageUp => state.which_key_scroll(w, h, -(whichkey::page_size(w, h) as i32)),
                WhichKeyPageDown => state.which_key_scroll(w, h, whichkey::page_size(w, h) as i32),
                WhichKeyHome => state.which_key_jump(w, h, false),
                WhichKeyEnd => state.which_key_jump(w, h, true),
                _ => {}
            }
            true
        }
        ToggleReasoning => state.toggle_last_reasoning(),
        OpenDiffViewer => {
            if !state.open_focused_diff() {
                state.open_diff_source_picker();
            }
            true
        }
        NextSession => state.cycle_session(true),
        PrevSession => state.cycle_session(false),
        ToggleSelect => {
            state.select_mode = true;
            true
        }
        CopySelection => {
            if state.copy_selection() {
                state.toast("已复制选中内容");
            } else {
                state.toast("没有可复制的内容");
            }
            true
        }
        ToggleFocusedTool => state.toggle_focused_tool(),
        OpenCommandPalette => {
            state.open_command_palette();
            true
        }
        ToggleSidebar => {
            state.toggle_sidebar();
            true
        }
        OpenModelPicker => {
            state.open_model_picker();
            true
        }
        NewSession => {
            state.new_session();
            true
        }
        BackgroundTask => {
            // agent 运行时：新建会话并切换，当前任务在后台继续（不取消）
            if state.phase.is_busy() {
                let old_id = state.active_session;
                state.new_session();
                state.toast(format!(
                    "任务已在后台运行（会话 #{old_id}），已切换到新会话"
                ));
            } else {
                state.toast("当前没有运行中的任务");
            }
            true
        }
        OpenTimeline => {
            if !state.open_timeline() {
                state.toast("当前会话没有消息");
            }
            true
        }
        ListSessions => {
            state.open_session_picker();
            true
        }
        DeleteSession => {
            state.open_delete_active_confirm();
            true
        }
        SwitchSession(n) => state.switch_session_by_index(n),
        OpenHelp => {
            state.open_help();
            true
        }
        Interrupt => {
            handle.cancel();
            true
        }
        SwitchProvider => {
            if !state.open_provider_picker() {
                if state.phase.is_busy() {
                    state.open_alert("switch provider", "请等当前任务结束后再切换");
                } else {
                    state.open_alert("switch provider", "配置中只有一个 provider，无需切换");
                }
            }
            true
        }
    }
}

fn input_is_empty(state: &AppState) -> bool {
    state.prompt.lines().iter().all(|l| l.is_empty())
}

/// 对话框接受结果的动作分派（纯值动作，闭包不进对话框）。
fn apply_select(
    state: &mut AppState,
    purpose: SelectPurpose,
    value: String,
    handle: &mrorcode_agent::RuntimeHandle,
) {
    match purpose {
        SelectPurpose::Command => {
            if let Some(cmd) = commands::command_by_id(&value) {
                execute_command(state, cmd, handle);
            }
        }
        SelectPurpose::Model => {
            if state.set_model(&value) {
                handle.set_model(value.clone());
                state.toast(format!("模型：{value}（下一轮生效）"));
            }
        }
        SelectPurpose::Session => {
            if let Some(rest) = value.strip_prefix("disk:") {
                if let Ok(id) = rest.parse::<u64>() {
                    if let Some(record) = state.load_history_session(id) {
                        handle.resume(id, record.conversation);
                        state.toast(format!("已恢复会话 {id}"));
                    }
                }
            } else if let Ok(id) = value.parse::<u64>() {
                state.switch_session(id);
            }
        }
        SelectPurpose::Rewind => {
            if let Ok(keep) = value.parse::<usize>() {
                state.rewind_to(keep, handle);
            }
        }
        SelectPurpose::Provider => {
            if value != state.current_provider {
                state.pending_provider_switch = Some(value);
            }
        }
        SelectPurpose::DiffSource => {
            open_diff_from_source(state, &value);
        }
    }
}

/// 根据选择的数据源打开独立 diff viewer。
fn open_diff_from_source(state: &mut AppState, source: &str) {
    let cwd = state.working_dir.clone();
    match source {
        "git" => {
            let output = run_git(&cwd, &["diff", "--no-color"]);
            match crate::widgets::dialog::DiffState::from_output("git diff", &output) {
                Some(diff) => state.push_dialog(dialog::Dialog::Diff(diff)),
                None => state.open_alert("git diff", "没有可显示的改动（工作区干净）"),
            }
        }
        "branch" => {
            let main_branch = detect_main_branch(&cwd);
            let args = format!("{}...HEAD", main_branch);
            let output = run_git(&cwd, &["diff", "--no-color", &args]);
            let title = format!("branch diff ({main_branch}...HEAD)");
            match crate::widgets::dialog::DiffState::from_output(&title, &output) {
                Some(diff) => state.push_dialog(dialog::Dialog::Diff(diff)),
                None => state.open_alert("branch diff", "当前分支与主分支无差异"),
            }
        }
        "last-turn" => match state.collect_last_turn_diff() {
            Some(output) => {
                match crate::widgets::dialog::DiffState::from_output("last turn", &output) {
                    Some(diff) => state.push_dialog(dialog::Dialog::Diff(diff)),
                    None => state.open_alert("last turn", "上一轮没有文件改动"),
                }
            }
            None => state.open_alert("last turn", "上一轮没有文件改动"),
        },
        _ => {}
    }
}

/// 执行 git 命令并返回 stdout（失败时返回空串）。
fn run_git(cwd: &std::path::Path, args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// 探测当前分支名（`git rev-parse --abbrev-ref HEAD`）；非仓库 / 失败返回 None。
fn detect_git_branch(cwd: &std::path::Path) -> Option<String> {
    let out = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let b = out.trim().to_string();
    if b.is_empty() || b == "HEAD" {
        None
    } else {
        Some(b)
    }
}

/// 检测仓库主分支名（main 或 master），默认 main。
fn detect_main_branch(cwd: &std::path::Path) -> String {
    for candidate in &["main", "master"] {
        let ok = std::process::Command::new("git")
            .args(["rev-parse", "--verify", candidate])
            .current_dir(cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return candidate.to_string();
        }
    }
    "main".to_string()
}

/// Confirm 对话框确认后的动作分派。
fn apply_confirm(state: &mut AppState, action: dialog::ConfirmAction) {
    match action {
        dialog::ConfirmAction::DeleteSession(id) => {
            state.delete_history_session(id);
            state.toast("会话已删除");
        }
    }
}

/// 输入对话框提交后的动作分派。
fn apply_input(state: &mut AppState, purpose: dialog::InputPurpose, text: String) {
    match purpose {
        dialog::InputPurpose::RenameSession => {
            state.rename_active_session(&text);
            state.toast("会话已重命名");
        }
    }
}

/// 执行命令面板分派的纯数据动作。
fn execute_command(state: &mut AppState, cmd: AppCommand, handle: &mrorcode_agent::RuntimeHandle) {
    match cmd {
        AppCommand::OpenCommandPalette => state.open_command_palette(),
        AppCommand::NewSession => {
            state.new_session();
        }
        AppCommand::ListSessions => state.open_session_picker(),
        AppCommand::DeleteSession => state.open_delete_active_confirm(),
        AppCommand::OpenModelPicker => state.open_model_picker(),
        AppCommand::ToggleSidebar => state.toggle_sidebar(),
        AppCommand::TogglePlanMode => state.cycle_mode(false, handle),
        AppCommand::CycleModel => state.cycle_model(false, handle),
        AppCommand::ToggleWhichKey => state.toggle_which_key(),
        AppCommand::ToggleWhichKeyLayout => state.toggle_which_key_layout(),
        AppCommand::ToggleReasoning => {
            state.toggle_last_reasoning();
        }
        AppCommand::OpenDiffViewer => {
            if !state.open_focused_diff() {
                // 无聚焦 diff 工具卡时，打开独立 diff viewer 的数据源选择器
                state.open_diff_source_picker();
            }
        }
        AppCommand::SelectText => {
            state.select_mode = true;
        }
        AppCommand::RewindSession => {
            state.open_rewind_picker();
        }
        AppCommand::PinSession => {
            state.toggle_pin_active();
        }
        AppCommand::RenameSession => {
            state.open_rename_dialog();
        }
        AppCommand::BackgroundTask => {
            if state.phase.is_busy() {
                let old_id = state.active_session;
                state.new_session();
                state.toast(format!(
                    "任务已在后台运行（会话 #{old_id}），已切换到新会话"
                ));
            } else {
                state.toast("当前没有运行中的任务");
            }
        }
        AppCommand::OpenTimeline => {
            if !state.open_timeline() {
                state.toast("当前会话没有消息");
            }
        }
        AppCommand::ExportTranscript => match state.export_transcript() {
            Ok(path) => {
                state.toast(format!("已导出：{}", path.display()));
            }
            Err(e) => {
                state.open_alert("导出失败", e.to_string());
            }
        },
        AppCommand::WidenSidebar => state.adjust_sidebar_width(4),
        AppCommand::NarrowSidebar => state.adjust_sidebar_width(-4),
        AppCommand::OpenExternalEditor => {
            state.pending_external_editor = true;
        }
        AppCommand::Quit => state.should_quit = true,
        AppCommand::Interrupt => {
            if state.phase.is_busy() {
                handle.cancel();
            } else {
                state.open_alert("interrupt", "当前没有正在进行的任务");
            }
        }
        AppCommand::OpenHelp => state.open_help(),
        AppCommand::SwitchProvider => {
            if !state.open_provider_picker() {
                if state.phase.is_busy() {
                    state.open_alert("switch provider", "请等当前任务结束后再切换");
                } else {
                    state.open_alert("switch provider", "配置中只有一个 provider，无需切换");
                }
            }
        }
    }
}

/// 输入历史浏览：Ctrl+P 上一条 / Ctrl+N 下一条（不依赖光标位置）。
/// Ctrl+N 在翻到最新一条后退出浏览并恢复草稿。
fn history_prev(state: &mut AppState) -> bool {
    if state.history.is_empty() {
        return false;
    }
    let next = match state.hist_idx {
        None => state.history.len() - 1,
        Some(0) => 0,
        Some(i) => i - 1,
    };
    if state.hist_idx.is_none() {
        state.draft = state.prompt.lines().join("\n");
    }
    state.hist_idx = Some(next);
    restore_into_prompt(state, state.history[next].clone());
    true
}

fn history_next(state: &mut AppState) -> bool {
    let Some(i) = state.hist_idx else {
        return false;
    };
    if i + 1 >= state.history.len() {
        state.hist_idx = None;
        let draft = std::mem::take(&mut state.draft);
        restore_into_prompt(state, draft);
    } else {
        state.hist_idx = Some(i + 1);
        restore_into_prompt(state, state.history[i + 1].clone());
    }
    true
}

fn restore_into_prompt(state: &mut AppState, text: String) {
    let lines: Vec<std::string::String> = text.lines().map(str::to_string).collect();
    state.prompt.move_cursor(tui_textarea::CursorMove::Top);
    state.prompt.select_all();
    state.prompt.delete_char();
    if lines.is_empty() {
        state.prompt.insert_str("");
    } else {
        for (i, l) in lines.into_iter().enumerate() {
            if i > 0 {
                state.prompt.insert_newline();
            }
            state.prompt.insert_str(&l);
        }
    }
}

// ---------------- 节拍 ----------------

fn ensure_wake(
    spinner: &mut Option<Interval>,
    blinker: &mut Option<Interval>,
    state: &mut AppState,
) {
    // busy 时驱动 spinner；有 toast 时也需要节拍让其按时消失（空闲无 toast 时 0 唤醒）
    let spinner_needed = state.phase.is_busy() || state.has_active_toasts();
    match (spinner_needed, spinner.is_some()) {
        (true, false) => {
            *spinner = Some(tokio::time::interval(SPINNER_TICK));
        }
        (false, true) => *spinner = None,
        _ => {}
    }

    // 输入光标常闪：仅在输入框可编辑（无审批弹窗/对话框/选择模式）时驱动
    let blink_needed = prompt_focused(state);
    match (blink_needed, blinker.is_some()) {
        (true, false) => {
            // 亮帧起步，首个 tick 延后一个半周期（避免 interval 立即 tick 把光标立刻灭掉）
            state.cursor_blink_on = true;
            let first = tokio::time::Instant::now() + CURSOR_BLINK_TICK;
            *blinker = Some(tokio::time::interval_at(first, CURSOR_BLINK_TICK));
        }
        (false, true) => *blinker = None,
        _ => {}
    }
}

/// 输入框当前是否处于可编辑态（光标只在此时闪烁）。
fn prompt_focused(state: &AppState) -> bool {
    state.permission.is_none() && state.dialogs.is_empty() && !state.select_mode
}

/// 任意输入都让光标回到亮帧并重新计时（连续输入时不熄灭）。
fn reset_cursor_blink(blinker: &mut Option<Interval>, state: &mut AppState) {
    state.cursor_blink_on = true;
    if let Some(interval) = blinker.as_mut() {
        // reset()：下次到期 = 现在 + 一个半周期
        interval.reset();
    }
}

// ---------------- 绘制 ----------------

/// which-key 面板几何（宽度、高度），取自最近一帧的屏幕尺寸。
/// 键盘导航键需要按列数/页大小滚动，必须知道面板尺寸，故复用 `state.screen`。
fn whichkey_geometry(state: &AppState) -> (u16, u16) {
    (state.screen.width, whichkey::height(state.screen.height))
}

fn draw(f: &mut Frame<'_>, state: &mut AppState) {
    // 每帧一次浅克隆（~50 个 Color，栈分配），避免与 state 的可变借用冲突
    let theme = state.theme.clone();
    // 每帧按当前闪烁态/主题同步光标样式（覆盖主题切换等外部改动）
    prompt::sync_cursor_style(state, &theme);
    let area = f.area();
    state.screen = area;
    // 整屏背景
    Block::default()
        .style(Style::default().bg(theme.background))
        .render(area, f.buffer_mut());

    // which-key 面板：dock 时内容让位（会话区收缩），overlay 时浮于内容之上
    // （对齐 opencode 的 `app_bottom` / `app` 两个插槽）
    let whichkey_h = if state.route == Route::Session && state.which_key_visible() {
        whichkey::height(area.height)
    } else {
        0
    };
    let reserve = match state.which_key_layout {
        whichkey::WkLayout::Dock => whichkey_h,
        whichkey::WkLayout::Overlay => 0,
    };
    if reserve > 0 {
        let content = Rect {
            height: area.height.saturating_sub(reserve),
            ..area
        };
        match state.route {
            Route::Home => home::render(state, content, f.buffer_mut(), &theme),
            Route::Session => draw_session(f, state, &theme, content),
        }
    } else {
        match state.route {
            Route::Home => {
                home::render(state, area, f.buffer_mut(), &theme);
            }
            Route::Session => draw_session(f, state, &theme, area),
        }
    }

    // which-key 面板（dock 与 overlay 都在屏幕最底部；dock 已在上方让出内容区）
    if whichkey_h > 0 {
        let wk_area = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(whichkey_h),
            width: area.width,
            height: whichkey_h,
        };
        whichkey::render(
            &state.keymap,
            wk_area,
            f.buffer_mut(),
            &theme,
            state.which_key_layout,
            &state.which_key,
        );
    }

    // 审批浮层
    if state.permission.is_some() {
        permission::render(state, area, f.buffer_mut(), &theme);
    }

    // 对话框栈顶（Select/Confirm/Help）
    if let Some(dialog) = state.dialogs.last() {
        dialog::render_top(dialog, area, f.buffer_mut(), &theme);
    }

    // toast：右上角单条（对齐 opencode，5s 过期）
    if let Some((level, text)) = state.active_toast() {
        toast::render(area, f.buffer_mut(), &theme, level, text);
    }

    // 记录本帧屏幕文本供选区提取；最后叠加选区高亮（M4-6）
    let rows = {
        let buf = f.buffer_mut();
        crate::selection::rows_from_buffer(buf)
    };
    state.screen_rows = rows;
    if let Some(sel) = state.selection {
        crate::selection::paint(&sel, f.buffer_mut(), &theme);
    }
}

fn draw_session(
    f: &mut Frame<'_>,
    state: &mut AppState,
    theme: &Theme,
    area: ratatui::layout::Rect,
) {
    // 对齐 opencode：宽 >120 内联侧栏；≤120 时侧栏作浮层覆盖主内容右侧（不挤压主区）
    const SIDEBAR_INLINE_THRESHOLD: u16 = 120;
    let sidebar_wanted = state.sidebar;
    let inline_sidebar = sidebar_wanted && area.width > SIDEBAR_INLINE_THRESHOLD;
    let overlay_sidebar = sidebar_wanted && area.width <= SIDEBAR_INLINE_THRESHOLD;
    let sw = state.sidebar_width;
    // 主内容区与侧栏之间留 1 列背景色间隔（对齐 opencode 视觉）
    const SIDEBAR_GAP: u16 = 1;
    // 顶部留白行（对齐 opencode：主区与侧栏上方各留一行空白 padding）
    const HEADER_H: u16 = 1;
    // 主内容区左右各 1 列内边距（内容不贴屏幕边缘）
    const CONTENT_PAD: u16 = 1;
    let main_width = if inline_sidebar {
        area.width.saturating_sub(sw + SIDEBAR_GAP)
    } else {
        area.width
    };
    // 主区整体内缩：顶部 HEADER_H 行 + 左右 CONTENT_PAD 列
    let main = ratatui::layout::Rect {
        x: area.x + CONTENT_PAD,
        y: area.y + HEADER_H,
        width: main_width.saturating_sub(CONTENT_PAD * 2),
        height: area.height.saturating_sub(HEADER_H),
    };
    let side = ratatui::layout::Rect {
        x: if inline_sidebar {
            area.x + main_width + SIDEBAR_GAP
        } else {
            // 浮层：贴右边缘，覆盖在主内容之上
            area.x + area.width.saturating_sub(sw)
        },
        y: area.y + HEADER_H,
        width: sw,
        height: area.height.saturating_sub(HEADER_H),
    };

    let prompt_h = prompt::content_height(state, area.height) + 2;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(prompt_h),
            Constraint::Length(1),
        ])
        .split(main);

    session::render(state, chunks[0], f.buffer_mut(), theme);
    prompt::render_prompt(state, chunks[1], f.buffer_mut(), theme);
    if let Some(comp) = state.completion.as_ref().filter(|c| c.visible()) {
        crate::input::render(comp, chunks[1], f.buffer_mut(), theme);
    }
    let meta = prompt::meta_line(state, theme);
    f.buffer_mut()
        .set_line(chunks[2].x, chunks[2].y, &meta, chunks[2].width);

    if inline_sidebar || overlay_sidebar {
        if overlay_sidebar {
            // 浮层遮罩：用主题背景色盖住侧栏底下的内容（跟随主题，模拟 opencode 的 alpha 70 底）
            let overlay_color = theme.background;
            for y in side.y..side.y + side.height {
                for x in side.x..side.x + side.width {
                    if let Some(cell) = f.buffer_mut().cell_mut((x, y)) {
                        cell.set_style(ratatui::style::Style::default().bg(overlay_color));
                    }
                }
            }
        }
        sidebar::render(state, side, f.buffer_mut(), theme);
    }
}

// ---------------- 终端生命周期 ----------------

type Term = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> anyhow::Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn shutdown_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

/// 恢复终端 → 在 $EDITOR 中编辑临时文件 → 回读 → 重建终端。
fn run_external_editor(state: &mut AppState, terminal: &mut Term) {
    use std::io::Write;

    // 1. 写入当前输入到临时文件
    let text = state.prompt.lines().join("\n");
    let tmp = std::env::temp_dir().join(format!("mrorcode-edit-{}.md", std::process::id()));
    if let Ok(mut f) = std::fs::File::create(&tmp) {
        let _ = f.write_all(text.as_bytes());
    }

    // 2. 恢复终端
    let _ = terminal.clear();
    shutdown_terminal();

    // 3. 调用编辑器
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });
    let _ = std::process::Command::new(&editor).arg(&tmp).status();

    // 4. 回读文件
    if let Ok(edited) = std::fs::read_to_string(&tmp) {
        let trimmed = edited.trim_end_matches('\n').to_string();
        let lines: Vec<String> = if trimmed.is_empty() {
            vec![String::new()]
        } else {
            trimmed.split('\n').map(|s| s.to_string()).collect()
        };
        state.prompt = TextArea::new(lines);
        state
            .prompt
            .set_cursor_line_style(ratatui::style::Style::default());
        let last = state.prompt.lines().len().saturating_sub(1);
        let col = state.prompt.lines().get(last).map(|l| l.len()).unwrap_or(0);
        state
            .prompt
            .move_cursor(tui_textarea::CursorMove::Jump(last as u16, col as u16));
    }
    let _ = std::fs::remove_file(&tmp);

    // 5. 重建终端
    if let Ok(new_term) = setup_terminal() {
        *terminal = new_term;
    }
    state.toast("已从编辑器载入");
}

fn install_panic_restore() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        shutdown_terminal();
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::store::{Phase, Route};
    use mrorcode_agent::{start, RuntimeHandle, RuntimeStart};
    use mrorcode_kernel::event::AgentEvent;
    use mrorcode_kernel::testkit::{text_script, ScriptedProvider};
    use mrorcode_kernel::tool::ToolRegistry;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn test_state() -> AppState {
        use std::sync::atomic::{AtomicU64, Ordering};
        static TEST_N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mrorcode-tui-test-{}-{}.json",
            std::process::id(),
            TEST_N.fetch_add(1, Ordering::Relaxed)
        ));
        let (store, _) = crate::storage::Store::load_from(&path);
        AppState::new("m".into(), Vec::new(), PathBuf::from("."), store)
    }

    fn buffer_contains(term: &Terminal<TestBackend>, needle: &str) -> bool {
        row_of(term, needle).is_some()
    }

    /// 返回包含 needle 的行号（自上往下第一处）。
    fn row_of(term: &Terminal<TestBackend>, needle: &str) -> Option<u16> {
        let buf = term.backend().buffer();
        let area = buf.area();
        for y in area.top()..area.bottom() {
            let mut row = String::new();
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell((x, y)) {
                    row.push_str(cell.symbol());
                }
            }
            if row.contains(needle) {
                return Some(y);
            }
        }
        None
    }

    fn start_runtime(
        provider: ScriptedProvider,
    ) -> (
        RuntimeHandle,
        tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    ) {
        let (h, rx) = start(RuntimeStart {
            provider: std::sync::Arc::new(provider),
            tools: ToolRegistry::new(),
            middleware: Vec::new(),
            working_dir: PathBuf::from("."),
            system_prompt: String::new(),
            model: None,
            compaction: Default::default(),
        });
        (h, rx)
    }

    #[tokio::test]
    async fn home_renders_logo_and_hint() {
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        term.draw(|f| draw(f, &mut state)).unwrap();
        // Logo（中文标题）
        assert!(buffer_contains(&term, "欧"));
        // 模型行：权限模式徽章 + 当前模型（CJK 字符单元格间有填充空格，用模糊匹配）
        let meta_row = row_of(&term, "工 作 区 读 写").expect("元信息行存在");
        // 键位提示行：ctrl+p commands
        let hint_row = row_of(&term, "ctrl+p").expect("键位提示行存在");

        // 布局守卫（垂直 chunks 索引错位会打破这个间距）：
        //   meta_row      元信息行（灰底面板最后一条内容行）
        //   +1            面板内 bottom padding（灰底空行）
        //   +2            面板外 bottom margin（空白行）
        //   +3            cwd / 键位提示行
        assert_eq!(
            hint_row - meta_row,
            3,
            "元信息行与提示行之间应隔出 面板底padding行 + 外边距空白行"
        );
        let buf = term.backend().buffer();
        // 120 宽终端 pw=75 面板居中：x≈22 起蓝竖线、x=23 起灰底，取 x=30 在面板内
        let pad_bg = buf.cell((30, meta_row + 1)).unwrap().style().bg;
        assert_eq!(
            pad_bg,
            Some(ratatui::style::Color::Rgb(0x14, 0x14, 0x14)),
            "元信息行下方一行应仍是灰底（面板 bottom padding）"
        );
        let margin_blank = (0..buf.area().width).all(|x| {
            buf.cell((x, meta_row + 2))
                .unwrap()
                .symbol()
                .trim()
                .is_empty()
        });
        assert!(margin_blank, "面板与提示行之间应有一行完全空白的外边距");
        handle.shutdown();
    }

    #[tokio::test]
    async fn submit_opens_session_and_streams_text() {
        let (handle, mut events) =
            start_runtime(ScriptedProvider::new(vec![text_script("你好世界")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();

        on_key(&mut state, &k(KeyCode::Char('h')), &handle);
        on_key(&mut state, &k(KeyCode::Char('i')), &handle);
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));
        assert_eq!(state.route, Route::Session);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let ev = tokio::time::timeout_at(deadline, events.recv())
                .await
                .expect("event")
                .unwrap();
            let terminal = matches!(
                ev,
                RuntimeEvent::Agent {
                    event: AgentEvent::TurnComplete { .. },
                    ..
                }
            );
            if let RuntimeEvent::Agent {
                session_id,
                event: agent_ev,
            } = ev
            {
                state.on_runtime_event(session_id, agent_ev, &handle);
            }
            if terminal {
                break;
            }
        }

        term.draw(|f| draw(f, &mut state)).unwrap();
        // 宽字符占两个 Cell，逐字断言而非整串（占格单元会打断连续性）
        for ch in "你好世界".chars() {
            assert!(buffer_contains(&term, &ch.to_string()), "missing {ch}");
        }
        handle.shutdown();
    }

    #[tokio::test]
    async fn idle_has_no_spinner_timer() {
        let mut state = test_state();
        let mut spinner: Option<tokio::time::Interval> = None;
        let mut blinker: Option<tokio::time::Interval> = None;
        ensure_wake(&mut spinner, &mut blinker, &mut state);
        assert!(
            spinner.is_none(),
            "idle loop must arm no spinner (0 spinner wakeups)"
        );
        assert!(blinker.is_some(), "idle prompt still blinks its cursor");

        state.phase = Phase::Working;
        ensure_wake(&mut spinner, &mut blinker, &mut state);
        assert!(spinner.is_some(), "busy state arms 100ms spinner");

        state.phase = Phase::Idle;
        ensure_wake(&mut spinner, &mut blinker, &mut state);
        assert!(spinner.is_none(), "spinner must stop when idle again");
    }

    #[tokio::test]
    async fn toast_arms_wake_and_expires_after_5s() {
        let mut state = test_state();
        state.toast("hi");
        let mut spinner: Option<tokio::time::Interval> = None;
        let mut blinker: Option<tokio::time::Interval> = None;
        ensure_wake(&mut spinner, &mut blinker, &mut state);
        assert!(spinner.is_some(), "active toast arms the wake interval");
        state.gc_toasts();
        assert!(state.active_toast().is_some());

        // 手动把时间戳调旧（>5s），gc 后节拍应撤掉
        if let Some((_, t, _)) = state.current_toast.as_mut() {
            *t = std::time::Instant::now() - Duration::from_secs(6);
        }
        state.gc_toasts();
        ensure_wake(&mut spinner, &mut blinker, &mut state);
        assert!(spinner.is_none(), "no timer after toasts expire");
    }

    #[tokio::test]
    async fn new_toast_replaces_current() {
        let mut state = test_state();
        state.toast("first");
        state.toast("second");
        assert_eq!(state.active_toast().map(|(_, t)| t), Some("second"));
    }

    #[tokio::test]
    async fn tool_card_collapse_focus_and_expand() {
        use crate::store::MessageView;

        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        state.route = Route::Session;
        state.messages.push(MessageView::assistant(1));

        // 3 行 bash 输出：默认折叠，只显示提示
        state.on_runtime_event(
            crate::store::DEFAULT_SESSION,
            AgentEvent::ToolStarted {
                id: "t1".into(),
                name: "bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
            &handle,
        );
        state.on_runtime_event(
            crate::store::DEFAULT_SESSION,
            AgentEvent::ToolResult {
                id: "t1".into(),
                name: "bash".into(),
                content: "line-a\nline-b\nline-c".into(),
                is_error: false,
            },
            &handle,
        );
        term.draw(|f| draw(f, &mut state)).unwrap();
        // CJK 双宽字符的占位 Cell 会打断连续匹配，逐字断言
        for ch in "输出行".chars() {
            assert!(
                buffer_contains(&term, &ch.to_string()),
                "missing {ch} in collapsed hint"
            );
        }
        assert!(!buffer_contains(&term, "line-a"));

        // 直接聚焦工具卡、Enter 展开后输出可见
        state.focused_tool = Some("t1".into());
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));
        assert!(state.is_tool_expanded("t1"));
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "line-a"));
        assert!(buffer_contains(&term, "line-c"));

        // 再按 Enter 收起；Esc 清焦点
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));
        assert!(!state.is_tool_expanded("t1"));
        assert!(on_key(&mut state, &k(KeyCode::Esc), &handle));
        assert!(state.focused_tool.is_none());
        handle.shutdown();
    }

    #[tokio::test]
    async fn expired_leader_falls_through_to_editing() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &handle,
        );
        // 手动把窗口放到过去
        state.leader_until =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(3000));
        let sidebar_before = state.sidebar;
        on_key(&mut state, &k(KeyCode::Char('b')), &handle);
        // 不触发 leader b：侧栏不变，字符进入输入框
        assert_eq!(state.sidebar, sidebar_before);
        assert!(state.prompt.lines().join("").contains('b'));
        handle.shutdown();
    }

    #[tokio::test]
    async fn narrow_screen_renders_sidebar_as_overlay() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        state.sidebar = true;
        // 进入会话页
        on_key(&mut state, &k(KeyCode::Char('h')), &handle);
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));
        assert_eq!(state.route, Route::Session);

        // 窄屏（宽 100 ≤ 120 阈值）：侧栏作为浮层渲染，主内容不被挤压
        let mut narrow = Terminal::new(TestBackend::new(100, 40)).unwrap();
        narrow.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&narrow, "欧"));
        // 浮层侧栏贴右边缘：侧栏标题 "欧瑞文..." 应在 x≈59（100-42+1）、y=2（顶部留白+侧栏内 padding）
        let buf_narrow = narrow.backend().buffer();
        let mut found_overlay = false;
        for x in 55..65 {
            if let Some(cell) = buf_narrow.cell((x, 2)) {
                if cell.symbol() == "欧" {
                    found_overlay = true;
                    break;
                }
            }
        }
        assert!(
            found_overlay,
            "sidebar title should appear in overlay region at right edge"
        );

        // 宽屏（宽 140 > 120）：侧栏内联渲染
        let mut wide = Terminal::new(TestBackend::new(140, 40)).unwrap();
        wide.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&wide, "欧"));
        // 内联侧栏标题应在 x≈98（140-42+1-1）、y=2（顶部留白+侧栏内 padding）
        let buf_wide = wide.backend().buffer();
        let mut found_inline = false;
        for x in 94..104 {
            if let Some(cell) = buf_wide.cell((x, 2)) {
                if cell.symbol() == "欧" {
                    found_inline = true;
                    break;
                }
            }
        }
        assert!(
            found_inline,
            "sidebar title should appear in inline region at right edge"
        );
        handle.shutdown();
    }

    #[tokio::test]
    async fn unknown_leader_second_key_is_swallowed() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &handle,
        );
        on_key(&mut state, &k(KeyCode::Char('q')), &handle);
        assert!(state.leader_until.is_none());
        assert!(state.prompt.lines().join("").is_empty());
        handle.shutdown();
    }

    #[tokio::test]
    async fn user_keybind_override_takes_effect() {
        use crate::storage::KeybindEntry;
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let path =
            std::env::temp_dir().join(format!("mrorcode-tui-keybind-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (mut store, _) = crate::storage::Store::load_from(&path);
        store.data.keybinds = vec![KeybindEntry {
            keys: "ctrl+k".into(),
            action: "command_palette".into(),
        }];
        let mut state = AppState::new("m".into(), Vec::new(), PathBuf::from("."), store);

        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            &handle,
        );
        assert_eq!(state.dialogs.len(), 1);
        let _ = std::fs::remove_file(&path);
        handle.shutdown();
    }

    #[tokio::test]
    async fn model_picker_sets_model_and_persists() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let path =
            std::env::temp_dir().join(format!("mrorcode-tui-model-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (store, _) = crate::storage::Store::load_from(&path);
        let mut state = AppState::new(
            "m1".into(),
            vec!["m1".to_string(), "m2-fast".to_string()],
            PathBuf::from("."),
            store,
        );
        state.open_model_picker();
        assert_eq!(state.dialogs.len(), 1);

        // 下移选第二项并确认
        on_key(&mut state, &k(KeyCode::Down), &handle);
        on_key(&mut state, &k(KeyCode::Enter), &handle);

        assert!(state.dialogs.is_empty());
        assert_eq!(state.model, "m2-fast");
        assert_eq!(state.kv.data.model.as_deref(), Some("m2-fast"));

        // 重载状态：记住的模型成为初始模型
        let (store2, _) = crate::storage::Store::load_from(&path);
        let state2 = AppState::new(
            "m1".into(),
            vec!["m1".to_string(), "m2-fast".to_string()],
            PathBuf::from("."),
            store2,
        );
        assert_eq!(state2.model, "m2-fast");
        let _ = std::fs::remove_file(&path);
        handle.shutdown();
    }

    #[tokio::test]
    async fn help_dialog_lists_real_keybinds_and_closes() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();

        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &handle,
        );
        for ch in "help".chars() {
            on_key(&mut state, &k(KeyCode::Char(ch)), &handle);
        }
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert_eq!(state.dialogs.len(), 1);

        term.draw(|f| draw(f, &mut state)).unwrap();
        // 真实绑定（含 leader）出现在 help 中
        assert!(buffer_contains(&term, "ctrl+p"));
        assert!(buffer_contains(&term, "ctrl+shift+t"));
        assert!(buffer_contains(&term, "leader b"));

        // q 关闭
        on_key(&mut state, &k(KeyCode::Char('q')), &handle);
        assert!(state.dialogs.is_empty());
        handle.shutdown();
    }

    #[tokio::test]
    async fn help_reflects_user_keybind_override() {
        use crate::storage::KeybindEntry;
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let path =
            std::env::temp_dir().join(format!("mrorcode-tui-help-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (mut store, _) = crate::storage::Store::load_from(&path);
        store.data.keybinds = vec![KeybindEntry {
            keys: "ctrl+k".into(),
            action: "command_palette".into(),
        }];
        let mut state = AppState::new("m".into(), Vec::new(), PathBuf::from("."), store);
        state.open_help();
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "ctrl+k"));
        let _ = std::fs::remove_file(&path);
        handle.shutdown();
    }

    #[tokio::test]
    async fn interrupt_when_idle_opens_alert() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        assert!(!state.phase.is_busy());

        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &handle,
        );
        for ch in "interrupt".chars() {
            on_key(&mut state, &k(KeyCode::Char(ch)), &handle);
        }
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert_eq!(state.dialogs.len(), 1);
        assert!(matches!(
            state.dialogs[0],
            crate::widgets::dialog::Dialog::Alert(_)
        ));
        // enter 关闭 alert
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert!(state.dialogs.is_empty());
        handle.shutdown();
    }

    #[tokio::test]
    async fn multiple_sessions_bucket_messages_and_restore() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![
            text_script("r1"),
            text_script("r2"),
        ]));
        let mut state = test_state();

        // 会话 1 提交（标题取自首条消息）
        for ch in "first topic".chars() {
            on_key(&mut state, &k(KeyCode::Char(ch)), &handle);
        }
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert_eq!(state.active_session, 1);
        assert_eq!(state.sessions[0].title, "first topic");
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.route, Route::Session);

        // ctrl+n 新建会话：消息收桶、回到 Home
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            &handle,
        );
        assert_eq!(state.active_session, 2);
        assert!(state.messages.is_empty());
        assert_eq!(state.route, Route::Home);
        for ch in "second".chars() {
            on_key(&mut state, &k(KeyCode::Char(ch)), &handle);
        }
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert_eq!(state.messages.len(), 1);

        // leader 1 切回会话 1：投影原样恢复
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &handle,
        );
        on_key(&mut state, &k(KeyCode::Char('1')), &handle);
        assert_eq!(state.active_session, 1);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.route, Route::Session);

        // leader 2 再切回去
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &handle,
        );
        on_key(&mut state, &k(KeyCode::Char('2')), &handle);
        assert_eq!(state.active_session, 2);
        handle.shutdown();
    }

    #[tokio::test]
    async fn ctrl_tab_switch_updates_rendered_message_content() {
        use crate::store::MessageView;
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();

        state.route = Route::Session;
        state
            .messages
            .push(MessageView::user(1, "ALPHA内容".into()));
        state.sessions[0].title = "alpha".into();
        state.new_session();
        state.route = Route::Session;
        state.messages.push(MessageView::user(2, "BETA内容".into()));
        state.sessions[1].title = "beta".into();
        // 当前在会话 2：屏幕应有 BETA
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "BETA"));

        // Ctrl+Tab 切到会话 1：屏幕必须变为 ALPHA
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
            &handle,
        );
        assert_eq!(state.active_session, 1);
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(
            buffer_contains(&term, "ALPHA"),
            "switched view must render ALPHA"
        );
        assert!(
            !buffer_contains(&term, "BETA"),
            "old BETA content must be cleared"
        );
        handle.shutdown();
    }

    #[tokio::test]
    async fn leader_number_switch_updates_rendered_content() {
        use crate::store::MessageView;
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();

        state.route = Route::Session;
        state
            .messages
            .push(MessageView::user(1, "ALPHA内容".into()));
        state.sessions[0].title = "alpha".into();
        state.new_session();
        state.route = Route::Session;
        state.messages.push(MessageView::user(2, "BETA内容".into()));
        state.sessions[1].title = "beta".into();
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "BETA"));

        // Ctrl+X 然后 1：切到会话 1
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &handle,
        );
        on_key(&mut state, &k(KeyCode::Char('1')), &handle);
        assert_eq!(state.active_session, 1);
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(
            buffer_contains(&term, "ALPHA"),
            "leader switch must render ALPHA"
        );
        assert!(!buffer_contains(&term, "BETA"));
        handle.shutdown();
    }

    #[tokio::test]
    async fn session_picker_switches_and_delete_confirm_removes() {
        use crate::store::MessageView;
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("r")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();

        // 造两个会话（消息直接入投影，标题模拟 submit 时设置）
        state.route = Route::Session;
        state.messages.push(MessageView::user(1, "alpha".into()));
        state.sessions[0].title = "alpha".into();
        state.new_session();
        state.route = Route::Session;
        state.messages.push(MessageView::user(2, "beta".into()));
        state.sessions[1].title = "beta".into();

        // ctrl+l 打开会话列表，下移选 alpha 并切换
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &handle,
        );
        assert_eq!(state.dialogs.len(), 1);
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "alpha"));
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert_eq!(state.active_session, 1);

        // 命令面板 → delete session → Confirm → y 确认
        state.open_delete_active_confirm();
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "alpha"));
        on_key(&mut state, &k(KeyCode::Char('y')), &handle);
        assert!(state.dialogs.is_empty());
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session, 2);
        assert_eq!(state.messages[0].text, "beta");
        handle.shutdown();
    }

    #[tokio::test]
    async fn command_palette_opens_filters_and_runs() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        let before = state.sidebar;

        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &handle,
        );
        assert_eq!(state.dialogs.len(), 1);
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "toggle sidebar"));

        // 过滤到 toggle sidebar 并执行
        for ch in "toggle sidebar".chars() {
            on_key(&mut state, &k(KeyCode::Char(ch)), &handle);
        }
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert!(state.dialogs.is_empty());
        assert_ne!(state.sidebar, before);
        handle.shutdown();
    }

    #[tokio::test]
    async fn reasoning_collapses_by_default_and_expands() {
        use crate::store::MessageView;
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        state.route = Route::Session;

        let mut m = MessageView::assistant(7);
        m.text = "final answer".into();
        m.doc.update(&m.text.clone());
        m.reasoning = "hidden chain of thought".into();
        m.reasoning_doc.update(&m.reasoning.clone());
        state.messages.push(m);

        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(!buffer_contains(&term, "hidden chain"));
        assert!(buffer_contains(&term, "final answer"));

        // leader r 展开
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &handle,
        );
        on_key(&mut state, &k(KeyCode::Char('r')), &handle);
        assert!(state.is_reasoning_expanded(7));
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "hidden chain"));
        handle.shutdown();
    }

    #[tokio::test]
    async fn focused_diff_tool_opens_viewer() {
        use crate::store::{MessageView, ToolStatus, ToolView};
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        state.route = Route::Session;

        let mut m = MessageView::assistant(1);
        m.tools.push(ToolView {
            id: "d1".into(),
            name: "edit_file".into(),
            arguments: "{}".into(),
            status: ToolStatus::Ok,
            output:
                "Edited a.rs (1 replacement(s))\n--- a.rs\n+++ a.rs\n@@ -1 +1 @@\n-old();\n+new();"
                    .into(),
            sub_agent_text: String::new(),
            sub_agent_tools: Vec::new(),
        });
        state.messages.push(m);
        state.focused_tool = Some("d1".into());

        // leader d 打开 viewer
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &handle,
        );
        on_key(&mut state, &k(KeyCode::Char('d')), &handle);
        assert_eq!(state.dialogs.len(), 1);
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "new();"));
        // q 关闭
        on_key(&mut state, &k(KeyCode::Char('q')), &handle);
        assert!(state.dialogs.is_empty());
        handle.shutdown();
    }

    #[tokio::test]
    async fn toast_renders_and_is_cleared_by_gc() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        state.toast("已保存");
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "已"));
        assert!(buffer_contains(&term, "保"));

        state.current_toast = None;
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(!buffer_contains(&term, "已"));
        handle.shutdown();
    }

    #[tokio::test]
    async fn ctrl_c_empty_quits_and_f2_cycles_model() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &handle,
        );
        assert!(state.should_quit);

        // F2 循环模型：单模型时不变，只提示
        let mut state2 = test_state();
        let model_before = state2.model.clone();
        on_key(&mut state2, &k(KeyCode::F(2)), &handle);
        assert_eq!(state2.model, model_before);
        handle.shutdown();
    }

    #[tokio::test]
    async fn f2_cycles_between_available_models() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let path =
            std::env::temp_dir().join(format!("mrorcode-tui-f2-{}.json", std::process::id()));
        let (store, _) = crate::storage::Store::load_from(&path);
        let mut state = AppState::new(
            "m1".into(),
            vec!["m1".into(), "m2".into()],
            PathBuf::from("."),
            store,
        );
        assert_eq!(state.model, "m1");
        on_key(&mut state, &k(KeyCode::F(2)), &handle);
        assert_eq!(state.model, "m2");
        on_key(&mut state, &k(KeyCode::F(2)), &handle);
        assert_eq!(state.model, "m1");
        handle.shutdown();
    }

    #[tokio::test]
    async fn shift_tab_cycles_permission_mode() {
        use mrorcode_agent::PermissionMode;
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        // 对齐 atomcode 默认：普通 Tab 不切模式（留给输入/补全）
        assert_eq!(state.mode, PermissionMode::Workspace);
        on_key(&mut state, &k(KeyCode::Tab), &handle);
        assert_eq!(state.mode, PermissionMode::Workspace, "普通 Tab 不应切模式");
        // Shift+Tab 正向循环：工作区读写 → 完全访问 → 只读 → 工作区读写
        on_key(&mut state, &k(KeyCode::BackTab), &handle);
        assert_eq!(state.mode, PermissionMode::Full);
        on_key(&mut state, &k(KeyCode::BackTab), &handle);
        assert_eq!(state.mode, PermissionMode::ReadOnly);
        on_key(&mut state, &k(KeyCode::BackTab), &handle);
        assert_eq!(state.mode, PermissionMode::Workspace);
        handle.shutdown();
    }

    #[tokio::test]
    async fn ctrl_alt_k_pins_which_key_panel() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        state.route = Route::Session;
        // 默认关闭
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(!buffer_contains(&term, "toggle ctrl+alt+k"));

        // ctrl+alt+k 打开后常驻（不依赖 leader 窗口）
        on_key(
            &mut state,
            &KeyEvent::new(
                KeyCode::Char('k'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
            &handle,
        );
        assert!(state.which_key_visible());
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "toggle ctrl+alt+k"));
        assert!(buffer_contains(&term, "overlay ctrl+alt+shift+k"));

        // 再按关闭
        on_key(
            &mut state,
            &KeyEvent::new(
                KeyCode::Char('k'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
            &handle,
        );
        assert!(!state.which_key_visible());
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(!buffer_contains(&term, "toggle ctrl+alt+k"));
        handle.shutdown();
    }

    #[tokio::test]
    async fn which_key_navigation_keys_are_ignored_when_hidden() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::ALT);
        // 面板关闭时导航键无效
        on_key(&mut state, &key, &handle);
        assert_eq!(state.which_key.group, None);

        on_key(
            &mut state,
            &KeyEvent::new(
                KeyCode::Char('k'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
            &handle,
        );
        on_key(&mut state, &key, &handle);
        assert_eq!(state.which_key.group, Some(1));
        on_key(&mut state, &key, &handle);
        assert_eq!(state.which_key.group, Some(2));
        handle.shutdown();
    }

    #[tokio::test]
    async fn which_key_dock_reserves_space_overlay_covers() {
        /// 返回包含 needle 的行号（0 起）。
        fn row_with(term: &Terminal<TestBackend>, w: u16, needle: &str) -> Option<usize> {
            term.backend()
                .buffer()
                .content()
                .chunks(w as usize)
                .position(|row| {
                    row.iter()
                        .map(|c| c.symbol())
                        .collect::<String>()
                        .contains(needle)
                })
        }

        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        state.route = Route::Session;
        state.toggle_which_key();
        let panel_h = crate::widgets::whichkey::height(40);
        assert_eq!(panel_h, 12);
        assert!(state.which_key_visible(), "panel should be on after toggle");
        assert_eq!(
            state.which_key_layout,
            crate::widgets::whichkey::WkLayout::Dock
        );

        // dock：面板独占底部 12 行，会话区收缩，输入框提示上移
        let mut dock = Terminal::new(TestBackend::new(120, 40)).unwrap();
        dock.draw(|f| draw(f, &mut state)).unwrap();
        assert_eq!(row_with(&dock, 120, "toggle ctrl+alt+k"), Some(39));
        assert_eq!(row_with(&dock, 120, "overlay ctrl+alt+shift+k"), Some(39));
        assert_eq!(row_with(&dock, 120, "ctrl+j"), Some(27));

        // overlay：会话区仍占满 40 行，不透明面板盖住底部输入框
        state.toggle_which_key_layout();
        let mut over = Terminal::new(TestBackend::new(120, 40)).unwrap();
        over.draw(|f| draw(f, &mut state)).unwrap();
        assert_eq!(row_with(&over, 120, "toggle ctrl+alt+k"), Some(39));
        assert_eq!(row_with(&over, 120, "dock ctrl+alt+shift+k"), Some(39));
        assert_eq!(row_with(&over, 120, "ctrl+j"), None);
        handle.shutdown();
    }

    // ---- 补全浮层键盘导航（M3 §9.5 遗留自动化） ----

    fn open_slash_completion(state: &mut AppState) {
        state.prompt.insert_str("/he");
        state.refresh_completion();
        assert!(
            state.completion.as_ref().is_some_and(|c| c.visible()),
            "expected /he to open completion"
        );
        assert!(
            state.completion.as_ref().unwrap().items.len() >= 2,
            "navigation test needs >=2 candidates, got {:?}",
            state.completion.as_ref().unwrap().items
        );
    }

    #[tokio::test]
    async fn completion_up_down_intercepted_and_wraps() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        open_slash_completion(&mut state);
        let n = state.completion.as_ref().unwrap().items.len();

        // Down 被浮层拦截、selected 前进、文本不动
        assert!(on_key(&mut state, &k(KeyCode::Down), &handle));
        assert_eq!(state.completion.as_ref().unwrap().selected, 1);
        assert_eq!(state.prompt.lines().join(""), "/he");

        // 越过末尾回绕到 0；再按 Up 越过开头回绕到末尾
        on_key(&mut state, &k(KeyCode::Down), &handle);
        on_key(&mut state, &k(KeyCode::Down), &handle);
        assert_eq!(state.completion.as_ref().unwrap().selected, 3 % n);
        on_key(&mut state, &k(KeyCode::Up), &handle);
        assert_eq!(state.completion.as_ref().unwrap().selected, 2 % n);
        handle.shutdown();
    }

    #[tokio::test]
    async fn completion_enter_accepts_navigated_selection() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        open_slash_completion(&mut state);
        let second_insert = state.completion.as_ref().unwrap().items[1].insert.clone();

        // 先 Down 选中第二项，再 Enter 接受当前选中项（而非固定第一项）
        on_key(&mut state, &k(KeyCode::Down), &handle);
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));
        let line = state.prompt.lines().join("");
        assert!(
            line.starts_with(&second_insert),
            "expected accepted second item {second_insert:?}, got {line:?}"
        );
        handle.shutdown();
    }

    #[tokio::test]
    async fn completion_esc_clears_without_accepting() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        open_slash_completion(&mut state);

        assert!(on_key(&mut state, &k(KeyCode::Esc), &handle));
        assert!(state.completion.is_none());
        // 文本原样保留，等待继续编辑
        assert_eq!(state.prompt.lines().join(""), "/he");
        handle.shutdown();
    }

    // ---- 跨 provider 切换（M5-e） ----

    fn two_provider_config() -> mrorcode_config::Config {
        use mrorcode_config::{Config, ProviderConfig};
        let mut cfg = Config::default();
        cfg.providers.insert(
            "a".into(),
            ProviderConfig {
                base_url: Some("https://a.example.com/v1".into()),
                model: Some("ma".into()),
                ..Default::default()
            },
        );
        cfg.providers.insert(
            "b".into(),
            ProviderConfig {
                provider_type: Some("anthropic".into()),
                base_url: Some("https://api.anthropic.com".into()),
                api_key: Some("k".into()),
                model: Some("mb".into()),
                ..Default::default()
            },
        );
        cfg
    }

    #[tokio::test]
    async fn provider_picker_arms_pending_switch() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        state.attach_config(
            two_provider_config(),
            AssembleOptions::default(),
            "a".into(),
        );

        // leader s 打开 provider 选择器
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &handle,
        );
        assert!(on_key(&mut state, &k(KeyCode::Char('s')), &handle));
        assert_eq!(state.dialogs.len(), 1);

        // 名称排序后 a 在前、b 在后；下移选 b 并确认 → pending 置位
        on_key(&mut state, &k(KeyCode::Down), &handle);
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert!(state.dialogs.is_empty());
        assert_eq!(state.pending_provider_switch.as_deref(), Some("b"));
        handle.shutdown();
    }

    #[tokio::test]
    async fn provider_picker_blocked_while_busy() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        state.attach_config(
            two_provider_config(),
            AssembleOptions::default(),
            "a".into(),
        );
        state.phase = Phase::Working;
        assert!(!state.open_provider_picker());
        assert!(state.dialogs.is_empty());
        handle.shutdown();
    }

    #[tokio::test]
    async fn provider_switch_replaces_runtime_and_resumes_session() {
        let (mut handle, mut events) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        state.attach_config(
            two_provider_config(),
            AssembleOptions::default(),
            "a".into(),
        );

        state.pending_provider_switch = Some("b".to_string());
        apply_provider_switch(&mut state, &mut handle, &mut events).await;

        // 切换后投影更新为 b / mb
        assert_eq!(state.current_provider, "b");
        assert_eq!(state.model, "mb");
        assert_eq!(state.available_models, vec!["mb"]);
        assert!(state.pending_provider_switch.is_none());
        // 新 runtime 替换成功：新事件通道存活且无残留事件
        match tokio::time::timeout(Duration::from_millis(50), events.recv()).await {
            Err(_) => {}
            Ok(ev) => panic!("unexpected event on new runtime: {ev:?}"),
        }
    }

    #[tokio::test]
    async fn provider_switch_failure_keeps_old_runtime() {
        use mrorcode_config::{Config, ProviderConfig};
        let mut cfg = Config::default();
        cfg.providers.insert(
            "a".into(),
            ProviderConfig {
                base_url: Some("https://a.example.com/v1".into()),
                model: Some("ma".into()),
                ..Default::default()
            },
        );
        // c 缺 base_url：resolve_named 失败
        cfg.providers.insert(
            "c".into(),
            ProviderConfig {
                model: Some("mc".into()),
                ..Default::default()
            },
        );

        let (handle, mut events) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        state.attach_config(cfg, AssembleOptions::default(), "a".into());

        apply_provider_switch_inner(&mut state, &handle, &mut events, "c").await;

        // 切换失败：provider 仍是 a，旧 runtime 事件通道仍存活
        assert_eq!(state.current_provider, "a");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), events.recv())
                .await
                .is_err()
        );
        handle.shutdown();
    }

    /// 直接置位 pending 并执行切换（复用正式流程）。
    async fn apply_provider_switch_inner(
        state: &mut AppState,
        handle: &RuntimeHandle,
        events: &mut UnboundedReceiver<RuntimeEvent>,
        target: &str,
    ) {
        state.pending_provider_switch = Some(target.to_string());
        let mut h = handle.clone();
        apply_provider_switch(state, &mut h, events).await;
    }

    // ---------- 斜杠命令提交解析 ----------

    #[tokio::test]
    async fn slash_command_new_session_creates_session_not_message() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        let sessions_before = state.sessions.len();
        let messages_before = state.messages.len();

        // 输入 /new-session 并提交
        state.prompt.insert_str("/new-session");
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));

        // 新建了会话，但没有往消息流里插入用户消息
        assert_eq!(state.sessions.len(), sessions_before + 1);
        assert_eq!(state.messages.len(), messages_before);
        // 输入框已清空
        assert!(state.prompt.lines().join("\n").trim().is_empty());
        handle.shutdown();
    }

    #[tokio::test]
    async fn slash_command_unknown_shows_toast_and_clears_prompt() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        let messages_before = state.messages.len();

        state.prompt.insert_str("/not-a-real-command");
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));

        // 没有发送消息
        assert_eq!(state.messages.len(), messages_before);
        // toast 显示了错误
        assert!(state
            .current_toast
            .as_ref()
            .is_some_and(|t| t.0.contains("未知命令")));
        // 输入框已清空
        assert!(state.prompt.lines().join("\n").trim().is_empty());
        handle.shutdown();
    }

    #[tokio::test]
    async fn slash_command_blocked_while_busy() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        state.phase = Phase::Working;
        let sessions_before = state.sessions.len();

        state.prompt.insert_str("/new-session");
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));

        // 忙碌中不执行命令
        assert_eq!(state.sessions.len(), sessions_before);
        assert!(state
            .current_toast
            .as_ref()
            .is_some_and(|t| t.0.contains("忙碌中")));
        handle.shutdown();
    }

    #[tokio::test]
    async fn plain_text_starting_with_slash_in_multiline_is_sent_as_message() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        let messages_before = state.messages.len();

        // 多行文本首行不是斜杠，整体作为普通消息发送
        state.prompt.insert_str("hello\n/new-session");
        assert!(on_key(&mut state, &k(KeyCode::Enter), &handle));

        // 消息已发送（submit_prompt 会 push 一条用户消息）
        assert_eq!(state.messages.len(), messages_before + 1);
        handle.shutdown();
    }

    // ---------- Emacs 行内键位（tui-textarea-2 原生支持，验证不被 keymap 拦截） ----------

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn alt_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[tokio::test]
    async fn emacs_ctrl_a_moves_to_line_start() {
        let handle = noop_handle();
        let mut state = test_state();
        state.prompt.insert_str("hello world");
        assert_eq!(state.prompt.cursor(), (0, 11));
        // 纯光标移动 input() 返回 false（未修改文本），但光标确实移动了
        on_key(&mut state, &ctrl_key(KeyCode::Char('a')), &handle);
        assert_eq!(state.prompt.cursor(), (0, 0));
        handle.shutdown();
    }

    #[tokio::test]
    async fn emacs_ctrl_e_moves_to_line_end() {
        let handle = noop_handle();
        let mut state = test_state();
        state.prompt.insert_str("hello world");
        on_key(&mut state, &ctrl_key(KeyCode::Char('a')), &handle);
        assert_eq!(state.prompt.cursor(), (0, 0));
        on_key(&mut state, &ctrl_key(KeyCode::Char('e')), &handle);
        assert_eq!(state.prompt.cursor(), (0, 11));
        handle.shutdown();
    }

    #[tokio::test]
    async fn emacs_ctrl_k_kills_to_end_of_line() {
        let handle = noop_handle();
        let mut state = test_state();
        state.prompt.insert_str("hello world");
        on_key(&mut state, &ctrl_key(KeyCode::Char('a')), &handle);
        for _ in 0..5 {
            on_key(&mut state, &k(KeyCode::Right), &handle);
        }
        assert_eq!(state.prompt.cursor(), (0, 5));
        assert!(on_key(&mut state, &ctrl_key(KeyCode::Char('k')), &handle));
        assert_eq!(state.prompt.lines().join(""), "hello");
        handle.shutdown();
    }

    #[tokio::test]
    async fn emacs_ctrl_w_kills_previous_word() {
        let handle = noop_handle();
        let mut state = test_state();
        state.prompt.insert_str("hello world");
        assert!(on_key(&mut state, &ctrl_key(KeyCode::Char('w')), &handle));
        assert_eq!(state.prompt.lines().join(""), "hello ");
        handle.shutdown();
    }

    #[tokio::test]
    async fn emacs_alt_f_moves_forward_word() {
        let handle = noop_handle();
        let mut state = test_state();
        state.prompt.insert_str("hello world");
        on_key(&mut state, &ctrl_key(KeyCode::Char('a')), &handle);
        assert_eq!(state.prompt.cursor(), (0, 0));
        on_key(&mut state, &alt_key(KeyCode::Char('f')), &handle);
        assert_eq!(state.prompt.cursor(), (0, 6));
        handle.shutdown();
    }

    #[tokio::test]
    async fn emacs_alt_b_moves_backward_word() {
        let handle = noop_handle();
        let mut state = test_state();
        state.prompt.insert_str("hello world");
        on_key(&mut state, &alt_key(KeyCode::Char('b')), &handle);
        assert_eq!(state.prompt.cursor(), (0, 6));
        handle.shutdown();
    }

    #[tokio::test]
    async fn emacs_alt_d_kills_next_word() {
        let handle = noop_handle();
        let mut state = test_state();
        state.prompt.insert_str("hello world");
        on_key(&mut state, &ctrl_key(KeyCode::Char('a')), &handle);
        assert!(on_key(&mut state, &alt_key(KeyCode::Char('d')), &handle));
        assert_eq!(state.prompt.lines().join(""), " world");
        handle.shutdown();
    }

    #[tokio::test]
    async fn emacs_ctrl_u_undo_and_ctrl_r_redo() {
        let handle = noop_handle();
        let mut state = test_state();
        state.prompt.insert_str("hello");
        assert_eq!(state.prompt.lines().join(""), "hello");
        assert!(on_key(&mut state, &ctrl_key(KeyCode::Char('u')), &handle));
        assert_eq!(state.prompt.lines().join(""), "");
        assert!(on_key(&mut state, &ctrl_key(KeyCode::Char('r')), &handle));
        assert_eq!(state.prompt.lines().join(""), "hello");
        handle.shutdown();
    }

    /// 不需要真实 provider 的测试用占位 handle（ScriptedProvider 空脚本）。
    fn noop_handle() -> RuntimeHandle {
        let (h, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        h
    }
}
