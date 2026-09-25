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

use mrorcode_agent::{Assembly, RuntimeEvent};

use crate::commands::{self, AppCommand};
use crate::store::{AppState, Route};
use crate::theme::Theme;
use crate::widgets::dialog::{self, DialogOutcome, SelectPurpose};
use crate::widgets::toast::ToastLevel;
use crate::widgets::{home, permission, prompt, session, sidebar, tabs, toast};

const RENDER_COALESCE: Duration = Duration::from_millis(8);
const SPINNER_TICK: Duration = Duration::from_millis(100);

/// 输入线程 → 主循环的统一输入事件（M4：键/鼠标/粘贴分流）。
#[derive(Clone, Debug)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// 终端粘贴（crossterm bracketed paste），文本原样交给输入框。
    Paste(String),
}

/// TUI 入口。`resume` 为要恢复的会话 id（来自 `--resume`）。
pub async fn run(assembly: Assembly, resume: Option<u64>) -> anyhow::Result<()> {
    let Assembly {
        handle,
        mut events,
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
    let mut dirty = true; // 首帧必画

    let result: io::Result<()> = loop {
        ensure_wake(&mut spinner, &state);
        tokio::select! {
            biased;

            maybe_input = input_rx.recv() => {
                match maybe_input {
                    Some(input) => {
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
            } else {
                state.should_quit = true;
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
        TogglePlanMode => {
            state.toggle_plan_mode(handle);
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
        AppCommand::Quit => state.should_quit = true,
        AppCommand::Interrupt => {
            if state.phase.is_busy() {
                handle.cancel();
            } else {
                state.open_alert("interrupt", "当前没有正在进行的任务");
            }
        }
        AppCommand::OpenHelp => state.open_help(),
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

fn ensure_wake(spinner: &mut Option<Interval>, state: &AppState) {
    // busy 时驱动 spinner；有 toast 时也需要节拍让其按时消失（空闲无 toast 时 0 唤醒）
    let needed = state.phase.is_busy() || state.has_active_toasts();
    match (needed, spinner.is_some()) {
        (true, false) => {
            *spinner = Some(tokio::time::interval(SPINNER_TICK));
        }
        (false, true) => *spinner = None,
        _ => {}
    }
}

// ---------------- 绘制 ----------------

fn draw(f: &mut Frame<'_>, state: &mut AppState) {
    // 每帧一次浅克隆（~50 个 Color，栈分配），避免与 state 的可变借用冲突
    let theme = state.theme.clone();
    state.hit_regions.clear();
    let area = f.area();
    // 整屏背景
    Block::default()
        .style(Style::default().bg(theme.background))
        .render(area, f.buffer_mut());

    match state.route {
        Route::Home => {
            home::render(state, area, f.buffer_mut(), &theme);
        }
        Route::Session => draw_session(f, state, &theme, area),
    }

    // 审批浮层
    if state.permission.is_some() {
        permission::render(state, area, f.buffer_mut(), &theme);
    }

    // 对话框栈顶（Select/Confirm/Help）
    if let Some(dialog) = state.dialogs.last() {
        dialog::render_top(dialog, area, f.buffer_mut(), &theme);
    }

    // which-key 浮层：leader 等待窗口内优先占底部（M4-7）
    let leader_now = std::time::Instant::now();
    let whichkey_h = if state.route == Route::Session && state.leader_active(leader_now) {
        crate::widgets::whichkey::height(&state.keymap, area.width)
    } else {
        0
    };
    if whichkey_h > 0 {
        let wk_area = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(whichkey_h),
            width: area.width,
            height: whichkey_h,
        };
        crate::widgets::whichkey::render(&state.keymap, wk_area, f.buffer_mut(), &theme);
    }

    // toast 浮层（最新在下，底部居中；which-key/补全浮层时上移避让，M4-7/11）
    let toasts: Vec<(ToastLevel, &str)> = state.active_toasts().collect();
    if !toasts.is_empty() {
        // 补全浮层占底部 prompt 行：同样纳入避让（M4-11）
        let completion_h = if state.completion.as_ref().is_some_and(|c| c.visible()) {
            1
        } else {
            0
        };
        let skip_bottom = whichkey_h.max(completion_h);
        toast::render(area, f.buffer_mut(), &theme, &toasts, skip_bottom);
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

    let prompt_h = prompt::max_height(area.height) + 2;
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
        ensure_wake(&mut spinner, &state);
        assert!(spinner.is_none(), "idle loop must arm no timer (0 wakeups)");

        state.phase = Phase::Working;
        ensure_wake(&mut spinner, &state);
        assert!(spinner.is_some(), "busy state arms 100ms spinner");

        state.phase = Phase::Idle;
        ensure_wake(&mut spinner, &state);
        assert!(spinner.is_none(), "spinner must stop when idle again");
    }

    #[tokio::test]
    async fn toasts_arm_wake_and_expire() {
        let mut state = test_state();
        state.toast("hi");
        let mut spinner: Option<tokio::time::Interval> = None;
        ensure_wake(&mut spinner, &state);
        assert!(spinner.is_some(), "active toast arms the wake interval");
        state.gc_toasts();
        assert_eq!(state.active_toasts().count(), 1);

        // 手动把时间戳调旧，gc 后节拍应撤掉
        if let Some((_, t, _)) = state.toasts.first_mut() {
            *t = std::time::Instant::now() - Duration::from_secs(4);
        }
        state.gc_toasts();
        ensure_wake(&mut spinner, &state);
        assert!(spinner.is_none(), "no timer after toasts expire");
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

        // Tab 聚焦、Enter 展开后输出可见
        assert!(on_key(&mut state, &k(KeyCode::Tab), &handle));
        assert_eq!(state.focused_tool.as_deref(), Some("t1"));
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
        assert!(buffer_contains(&term, "f2"));

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

        // Esc 在无对话框时回到 base 层行为（idle 直接退出）
        on_key(&mut state, &k(KeyCode::Esc), &handle);
        assert!(state.should_quit);
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

        state.toasts.clear();
        term.draw(|f| draw(f, &mut state)).unwrap();
        assert!(!buffer_contains(&term, "已"));
        handle.shutdown();
    }

    #[tokio::test]
    async fn ctrl_c_empty_quits_and_f2_toggles_sidebar() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = test_state();
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &handle,
        );
        assert!(state.should_quit);

        let mut state2 = test_state();
        assert!(state2.sidebar);
        on_key(&mut state2, &k(KeyCode::F(2)), &handle);
        assert!(!state2.sidebar);
        handle.shutdown();
    }
}
