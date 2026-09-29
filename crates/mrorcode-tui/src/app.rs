//! TUI 应用：终端生命周期 + 脏标记驱动事件循环。
//!
//! 渲染节拍：无固定帧率 tick。事件只置脏，首个脏信号启动 8ms 尾沿定时器，
//! 期间 delta 全部合并成一帧；空闲时 select 完全阻塞（0 唤醒）。
//! spinner 仅在 busy 时启用 100ms 慢节拍。

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEvent, KeyModifiers, MouseEvent,
};
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

use mrorcode_agent::{AssembleOptions, Assembly, RuntimeEvent};

use crate::commands::{self, AppCommand};
use crate::store::{AppState, Route};
use crate::theme::Theme;
use crate::widgets::dialog::{self, DialogOutcome, SelectPurpose};
use crate::widgets::{home, permission, prompt, session, sidebar, tabs, toast, whichkey};

const RENDER_COALESCE: Duration = Duration::from_millis(8);
const SPINNER_TICK: Duration = Duration::from_millis(100);
/// 输入光标亮/灭半周期（应用层驱动闪烁）。
const CURSOR_BLINK_TICK: Duration = Duration::from_millis(600);
/// 关停旧 runtime 时等待 Exited 的兜底超时。
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// 输入线程 → 主循环的统一输入事件（M4：键/鼠标/粘贴分流）。
#[derive(Clone, Debug)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// 终端粘贴（crossterm bracketed paste），文本原样交给输入框。
    Paste(String),
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
        ..
    } = assembly;
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

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

    // 5) 更新投影；plan 模式需重新下发给新 kernel。
    state.note_provider_switched(provider_name.clone(), new_model.clone(), new_available);
    if state.plan_mode {
        handle.set_plan_mode(true);
    }
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
            Ok(event::Event::Key(k)) => InputEvent::Key(k),
            Ok(event::Event::Mouse(m)) => InputEvent::Mouse(m),
            Ok(event::Event::Paste(s)) => InputEvent::Paste(s),
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
        InputEvent::Mouse(mouse) => on_mouse(state, mouse, handle),
        InputEvent::Paste(text) => on_paste(state, text),
    }
}

/// 粘贴：规范化 CR/CRLF 后插入输入框（M2 clipboard 风险表）。
fn on_paste(state: &mut AppState, text: &str) -> bool {
    if state.permission.is_some() || !state.dialogs.is_empty() {
        return false;
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            state.prompt.insert_newline();
        }
        state.prompt.insert_str(line);
    }
    state.refresh_completion();
    true
}

/// 鼠标事件：滚轮滚动消息区、点击命中区域（M4-5）。
fn on_mouse(
    state: &mut AppState,
    mouse: &MouseEvent,
    _handle: &mrorcode_agent::RuntimeHandle,
) -> bool {
    use crossterm::event::{MouseButton, MouseEventKind};
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if state.route == Route::Session {
                state.scroll_up(3);
                true
            } else {
                false
            }
        }
        MouseEventKind::ScrollDown => {
            if state.route == Route::Session {
                state.scroll_down(3);
                true
            } else {
                false
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // 优先命中原生控件（标签/侧栏/工具卡/思考块/diff）
            if let Some(target) = crate::hit::hit_test(&state.hit_regions, mouse.column, mouse.row)
            {
                return match target {
                    crate::hit::HitTarget::Session(id) => state.switch_session(id),
                    crate::hit::HitTarget::CloseSession(id) => {
                        state.delete_session(id);
                        true
                    }
                    crate::hit::HitTarget::Tool(id) => {
                        state.focused_tool = Some(id.clone());
                        state.toggle_focused_tool();
                        true
                    }
                    crate::hit::HitTarget::Reasoning(uid) => state.toggle_reasoning(uid),
                    crate::hit::HitTarget::Diff(id) => {
                        state.focused_tool = Some(id);
                        state.open_focused_diff()
                    }
                    crate::hit::HitTarget::SidebarSection(title) => {
                        state.toggle_section(&title);
                        true
                    }
                };
            }
            // 否则视为文本拖选
            state.begin_selection(mouse.column, mouse.row);
            state.select_mode = true;
            true
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if state.select_mode {
                state.extend_selection(mouse.column, mouse.row);
                true
            } else {
                false
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // 拖拽结束：保留选区供 y 复制；无实际范围的单击清掉
            if let Some(sel) = state.selection {
                let (a, b) = sel.bounds();
                if a == b {
                    state.clear_selection();
                    state.select_mode = false;
                }
            }
            true
        }
        _ => false,
    }
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
    // modal 层：审批弹窗优先吃键
    if state.permission.is_some() {
        match key.code {
            KeyCode::Char('y') => state.resolve_permission("allow", handle),
            KeyCode::Char('a') => state.resolve_permission("allow_always", handle),
            KeyCode::Char('n') | KeyCode::Esc => state.resolve_permission("deny", handle),
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
        ScrollUp => {
            if state.route == Route::Session {
                state.scroll_up(state.viewport_lines / 2 + 1);
                true
            } else {
                false
            }
        }
        ScrollDown => {
            if state.route == Route::Session {
                state.scroll_down(state.viewport_lines / 2 + 1);
                true
            } else {
                false
            }
        }
        HistoryPrev => history_key(state, KeyCode::Up),
        HistoryNext => history_key(state, KeyCode::Down),
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
        OpenDiffViewer => state.open_focused_diff(),
        ToggleTabs => {
            state.toggle_tabs();
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
        OpenThemePicker => {
            state.open_theme_picker();
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
        SelectPurpose::Theme => {
            if state.set_theme(&value) {
                state.toast(format!("主题：{value}"));
            }
        }
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
    }
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

/// 执行命令面板分派的纯数据动作。
fn execute_command(state: &mut AppState, cmd: AppCommand, handle: &mrorcode_agent::RuntimeHandle) {
    match cmd {
        AppCommand::OpenCommandPalette => state.open_command_palette(),
        AppCommand::NewSession => {
            state.new_session();
        }
        AppCommand::ListSessions => state.open_session_picker(),
        AppCommand::DeleteSession => state.open_delete_active_confirm(),
        AppCommand::OpenThemePicker => state.open_theme_picker(),
        AppCommand::OpenModelPicker => state.open_model_picker(),
        AppCommand::ToggleSidebar => state.toggle_sidebar(),
        AppCommand::TogglePlanMode => state.toggle_plan_mode(handle),
        AppCommand::CycleModel => state.cycle_model(false, handle),
        AppCommand::ToggleWhichKey => state.toggle_which_key(),
        AppCommand::ToggleWhichKeyLayout => state.toggle_which_key_layout(),
        AppCommand::ToggleReasoning => {
            state.toggle_last_reasoning();
        }
        AppCommand::OpenDiffViewer => {
            if !state.open_focused_diff() {
                state.open_alert(
                    "diff",
                    "当前没有聚焦的 diff 工具卡（先 Tab 聚焦含 diff 的工具）",
                );
            }
        }
        AppCommand::ToggleTabs => state.toggle_tabs(),
        AppCommand::SelectText => {
            state.select_mode = true;
        }
        AppCommand::RewindSession => {
            state.open_rewind_picker();
        }
        AppCommand::PinSession => {
            state.toggle_pin_active();
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

/// 历史浏览：光标在首行首列（Up）或末行末列（Down）时触发，返回是否拦截。
fn history_key(state: &mut AppState, code: KeyCode) -> bool {
    let (row, col) = state.prompt.cursor();
    let line_count = state.prompt.lines().len();
    match code {
        KeyCode::Up if row == 0 && col == 0 && !state.history.is_empty() => {
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
        KeyCode::Down if state.hist_idx.is_some() && row == line_count - 1 => {
            let i = state.hist_idx.unwrap();
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
        _ => false,
    }
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
    state.hit_regions.clear();
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
    let show_sidebar = state.sidebar && area.width > 100;
    let main_width = if show_sidebar {
        area.width - sidebar::WIDTH
    } else {
        area.width
    };
    let main = ratatui::layout::Rect {
        width: main_width,
        ..area
    };
    let side = ratatui::layout::Rect {
        x: area.x + main_width,
        y: area.y,
        width: if show_sidebar { sidebar::WIDTH } else { 0 },
        height: area.height,
    };

    let prompt_h = prompt::content_height(state, area.height) + 2;
    let show_tabs = state.show_tabs && area.height > 6;
    let tab_h = if show_tabs { tabs::HEIGHT } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(tab_h),
            Constraint::Min(3),
            Constraint::Length(prompt_h),
            Constraint::Length(1),
        ])
        .split(main);

    if show_tabs {
        state.tab_hits = tabs::render(state, chunks[0], f.buffer_mut(), theme);
        for hit in &state.tab_hits {
            state.hit_regions.push(crate::hit::HitRegion {
                rect: hit.rect,
                target: crate::hit::HitTarget::Session(hit.session_id),
            });
        }
    } else {
        state.tab_hits.clear();
    }

    session::render(state, chunks[1], f.buffer_mut(), theme);
    prompt::render_prompt(state, chunks[2], f.buffer_mut(), theme);
    if let Some(comp) = state.completion.as_ref().filter(|c| c.visible()) {
        crate::input::render(comp, chunks[2], f.buffer_mut(), theme);
    }
    let meta = prompt::meta_line(state, theme);
    f.buffer_mut()
        .set_line(chunks[3].x, chunks[3].y, &meta, chunks[3].width);

    if show_sidebar {
        let side_hits = sidebar::render(state, side, f.buffer_mut(), theme);
        state.hit_regions.extend(side_hits);
    }
}

// ---------------- 终端生命周期 ----------------

type Term = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> anyhow::Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn shutdown_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
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
                return true;
            }
        }
        false
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
        assert!(buffer_contains(&term, "mrorcode"));
        assert!(buffer_contains(&term, "enter"));
        for ch in "发送".chars() {
            assert!(buffer_contains(&term, &ch.to_string()));
        }
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
    async fn leader_sequences_toggle_sidebar_and_open_themes() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        let before = state.sidebar;
        let ctrl_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);

        // ctrl+x 然后 b：切换侧栏
        on_key(&mut state, &ctrl_x, &handle);
        assert!(state.leader_until.is_some());
        assert!(on_key(&mut state, &k(KeyCode::Char('b')), &handle));
        assert_ne!(state.sidebar, before);
        assert!(state.leader_until.is_none());

        // ctrl+x 然后 t：主题选择器
        on_key(&mut state, &ctrl_x, &handle);
        on_key(&mut state, &k(KeyCode::Char('t')), &handle);
        assert_eq!(state.dialogs.len(), 1);
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
        assert!(buffer_contains(&term, "leader t"));
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
    async fn mouse_click_tab_switch_updates_rendered_content() {
        use crate::store::MessageView;
        use crossterm::event::{MouseEvent, MouseEventKind};
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

        // 找到会话 1 标签的命中坐标，模拟鼠标点击
        let hit = state
            .tab_hits
            .iter()
            .find(|h| h.session_id == 1)
            .expect("tab hit for session 1");
        let click = MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: hit.rect.x,
            row: hit.rect.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(on_mouse(&mut state, &click, &handle));
        assert_eq!(state.active_session, 1);
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(
            buffer_contains(&term, "ALPHA"),
            "clicked tab must render ALPHA"
        );
        assert!(!buffer_contains(&term, "BETA"));
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
        for ch in "side".chars() {
            on_key(&mut state, &k(KeyCode::Char(ch)), &handle);
        }
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert!(state.dialogs.is_empty());
        assert_ne!(state.sidebar, before);
        handle.shutdown();
    }

    #[tokio::test]
    async fn command_palette_chains_into_theme_picker() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();

        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &handle,
        );
        for ch in "theme".chars() {
            on_key(&mut state, &k(KeyCode::Char(ch)), &handle);
        }
        on_key(&mut state, &k(KeyCode::Enter), &handle);

        // 面板出栈后立即压入主题选择器
        assert_eq!(state.dialogs.len(), 1);
        use crate::widgets::dialog::Dialog;
        if let Dialog::Select(s) = &state.dialogs[0] {
            use crate::widgets::dialog::select::SelectPurpose as P;
            assert_eq!(s.purpose(), P::Theme);
            assert!(s.len() >= 30, "33 个 vendored 主题应可选");
        }
        handle.shutdown();
    }

    #[tokio::test]
    async fn select_dialog_filters_and_applies_theme() {
        use crate::widgets::dialog::select::SelectItem;
        use crate::widgets::dialog::{select::SelectState, Dialog, SelectPurpose};

        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        let items = vec!["nord", "dracula", "tokyonight"]
            .into_iter()
            .map(|n| SelectItem {
                label: n.into(),
                hint: String::new(),
                group: "Dark".into(),
                value: n.into(),
            })
            .collect();
        state.push_dialog(Dialog::Select(SelectState::new(
            "switch theme",
            SelectPurpose::Theme,
            items,
        )));

        for ch in "nor".chars() {
            on_key(&mut state, &k(KeyCode::Char(ch)), &handle);
        }
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "nord"));
        assert!(!buffer_contains(&term, "dracula"));

        // Enter 应用、对话框出栈、KV 记住
        on_key(&mut state, &k(KeyCode::Enter), &handle);
        assert!(state.dialogs.is_empty());
        assert_eq!(state.theme_name, "nord");
        assert_eq!(state.kv.data.theme.as_deref(), Some("nord"));

        // Esc 在无对话框空闲时不再退出（避免误触），仅被吞掉
        on_key(&mut state, &k(KeyCode::Esc), &handle);
        assert!(!state.should_quit);
        handle.shutdown();
    }

    #[tokio::test]
    async fn mouse_click_switches_session_and_scrolls() {
        use crate::store::MessageView;
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        state.route = Route::Session;
        state.messages.push(MessageView::user(1, "alpha".into()));
        state.sessions[0].title = "alpha".into();
        state.new_session();
        state.route = Route::Session;
        state.messages.push(MessageView::user(2, "beta".into()));
        state.sessions[1].title = "beta".into();

        term.draw(|f| draw(f, &mut state)).unwrap();
        // 点击第一个标签 -> 切到会话 1
        let target = state.tab_hits[0].rect;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: target.x + 1,
            row: target.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(on_mouse(&mut state, &click, &handle));
        assert_eq!(state.active_session, 1);

        // 滚轮上滚 -> 脱离粘底
        let scroll = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        on_mouse(&mut state, &scroll, &handle);
        assert!(!state.stick_bottom || state.backscroll > 0);
        handle.shutdown();
    }

    #[tokio::test]
    async fn tab_bar_renders_sessions_and_cycles() {
        use crate::store::MessageView;
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = test_state();
        state.route = Route::Session;
        state.messages.push(MessageView::user(1, "alpha".into()));
        state.sessions[0].title = "alpha".into();
        state.new_session();
        state.route = Route::Session;
        state.messages.push(MessageView::user(2, "beta".into()));
        state.sessions[1].title = "beta".into();

        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(buffer_contains(&term, "alpha"));
        assert!(buffer_contains(&term, "beta"));
        assert_eq!(state.tab_hits.len(), 2);

        // ctrl+tab 切回第一个（从第二个循环回到第一个）
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL),
            &handle,
        );
        assert_eq!(state.active_session, 1);

        // 关闭标签栏：不再渲染 tab 行
        state.toggle_tabs();
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(state.tab_hits.is_empty());
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
    async fn tab_cycles_agent_plan_mode() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        assert!(!state.plan_mode);
        on_key(&mut state, &k(KeyCode::Tab), &handle);
        assert!(state.plan_mode);
        on_key(&mut state, &k(KeyCode::BackTab), &handle);
        assert!(!state.plan_mode);
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
}
