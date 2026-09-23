//! TUI 应用：终端生命周期 + 脏标记驱动事件循环。
//!
//! 渲染节拍：无固定帧率 tick。事件只置脏，首个脏信号启动 8ms 尾沿定时器，
//! 期间 delta 全部合并成一帧；空闲时 select 完全阻塞（0 唤醒）。
//! spinner 仅在 busy 时启用 100ms 慢节拍。

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Widget};
use ratatui::{Frame, Terminal};
use std::pin::Pin;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Interval, Sleep};

use mrorcode_agent::{Assembly, RuntimeEvent};

use crate::store::{AppState, Route};
use crate::theme::Theme;
use crate::widgets::{home, permission, prompt, session, sidebar};

const RENDER_COALESCE: Duration = Duration::from_millis(8);
const SPINNER_TICK: Duration = Duration::from_millis(100);

/// TUI 入口。
pub async fn run(assembly: Assembly) -> anyhow::Result<()> {
    let Assembly {
        handle,
        mut events,
        model,
    } = assembly;
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let mut terminal = setup_terminal()?;
    install_panic_restore();

    let mut state = AppState::new(model, working_dir);
    let theme = Theme::tokyonight();
    prompt::style_textarea(&mut state, &theme);

    let (_input_thread, mut key_rx) = spawn_input_thread();

    let mut render_due: Option<Pin<Box<Sleep>>> = None;
    let mut spinner: Option<Interval> = None;
    let mut dirty = true; // 首帧必画

    let result: io::Result<()> = loop {
        ensure_spinner(&mut spinner, &state);
        tokio::select! {
            biased;

            maybe_key = key_rx.recv() => {
                match maybe_key {
                    Some(key) => {
                        if on_key(&mut state, &key, &handle) {
                            dirty = true;
                        }
                    }
                    None => break Ok(()),
                }
            }

            ev = events.recv() => {
                match ev {
                    Some(RuntimeEvent::Agent(agent_ev)) => {
                        state.on_runtime_event(agent_ev, &handle);
                        dirty = true;
                    }
                    Some(RuntimeEvent::Exited) | None => {
                        state.set_notice("runtime exited");
                        dirty = true;
                    }
                }
            }

            _ = async {
                if let Some(i) = spinner.as_mut() {
                    i.tick().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if spinner.is_some() => {
                state.spinner_ticks = state.spinner_ticks.wrapping_add(1);
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
                if let Err(e) = terminal.draw(|f| draw(f, &mut state, &theme)) {
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

fn spawn_input_thread() -> (std::thread::JoinHandle<()>, UnboundedReceiver<KeyEvent>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<KeyEvent>();
    // 阻塞式 crossterm 读取线程（poll/read 不占 CPU）
    let handle = std::thread::spawn(move || loop {
        match event::read() {
            Ok(event::Event::Key(k)) => {
                if tx.send(k).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });
    (handle, rx)
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

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if input_is_empty(state) {
                state.should_quit = true;
            } else {
                state.prompt.select_all();
                state.prompt.delete_char();
            }
            return true;
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.prompt.insert_newline();
            return true;
        }
        KeyCode::F(2) => {
            state.sidebar = !state.sidebar;
            return true;
        }
        KeyCode::PageUp => {
            if state.route == Route::Session {
                state.scroll_up(state.viewport_lines / 2 + 1);
                return true;
            }
        }
        KeyCode::PageDown => {
            if state.route == Route::Session {
                state.scroll_down(state.viewport_lines / 2 + 1);
                return true;
            }
        }
        KeyCode::Esc => {
            if state.phase.is_busy() {
                let now = std::time::Instant::now();
                let double = state
                    .esc_first
                    .is_some_and(|t| now.duration_since(t) < Duration::from_secs(5));
                if double {
                    handle.cancel();
                    state.esc_first = None;
                    state.set_notice("正在中断…");
                } else {
                    state.esc_first = Some(now);
                    state.set_notice("再按一次 Esc 中断");
                }
                return true;
            } else {
                state.should_quit = true;
                return true;
            }
        }
        KeyCode::Enter => {
            // Alt/Shift/Ctrl+Enter 换行
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::CONTROL)
            {
                state.prompt.insert_newline();
                return true;
            }
            if state.submit_prompt(handle) {
                return true;
            }
            return false;
        }
        KeyCode::Up | KeyCode::Down => {
            if history_key(state, key.code) {
                return true;
            }
        }
        _ => {
            state.esc_first = None;
        }
    }

    // 普通编辑键交给 textarea
    state.prompt.input(*key)
}

fn input_is_empty(state: &AppState) -> bool {
    state.prompt.lines().iter().all(|l| l.is_empty())
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

fn ensure_spinner(spinner: &mut Option<Interval>, state: &AppState) {
    match (state.phase.is_busy(), spinner.is_some()) {
        (true, false) => {
            *spinner = Some(tokio::time::interval(SPINNER_TICK));
        }
        (false, true) => *spinner = None,
        _ => {}
    }
}

// ---------------- 绘制 ----------------

fn draw(f: &mut Frame<'_>, state: &mut AppState, theme: &Theme) {
    let area = f.area();
    // 整屏背景
    Block::default()
        .style(Style::default().bg(theme.background))
        .render(area, f.buffer_mut());

    match state.route {
        Route::Home => {
            home::render(state, area, f.buffer_mut(), theme);
        }
        Route::Session => draw_session(f, state, theme, area),
    }

    // 审批浮层
    if state.permission.is_some() {
        permission::render(state, area, f.buffer_mut(), theme);
    }

    // 顶部 notice
    if let Some(text) = state.notice_text() {
        let line = ratatui::text::Line::from(ratatui::text::Span::styled(
            text.to_string(),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
        f.buffer_mut()
            .set_line(area.x + 1, area.y, &line, area.width - 2);
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
    let meta = prompt::meta_line(state, theme);
    f.buffer_mut()
        .set_line(chunks[2].x, chunks[2].y, &meta, chunks[2].width);

    if show_sidebar {
        sidebar::render(state, side, f.buffer_mut(), theme);
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
        });
        (h, rx)
    }

    #[tokio::test]
    async fn home_renders_logo_and_hint() {
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = AppState::new("m".into(), PathBuf::from("."));
        term.draw(|f| draw(f, &mut state, &Theme::tokyonight()))
            .unwrap();
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
        let mut state = AppState::new("m".into(), PathBuf::from("."));

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
            let terminal = matches!(ev, RuntimeEvent::Agent(AgentEvent::TurnComplete { .. }));
            if let RuntimeEvent::Agent(agent_ev) = ev {
                state.on_runtime_event(agent_ev, &handle);
            }
            if terminal {
                break;
            }
        }

        term.draw(|f| draw(f, &mut state, &Theme::tokyonight()))
            .unwrap();
        // 宽字符占两个 Cell，逐字断言而非整串（占格单元会打断连续性）
        for ch in "你好世界".chars() {
            assert!(buffer_contains(&term, &ch.to_string()), "missing {ch}");
        }
        handle.shutdown();
    }

    #[tokio::test]
    async fn idle_has_no_spinner_timer() {
        let mut state = AppState::new("m".into(), PathBuf::from("."));
        let mut spinner: Option<tokio::time::Interval> = None;
        ensure_spinner(&mut spinner, &state);
        assert!(spinner.is_none(), "idle loop must arm no timer (0 wakeups)");

        state.phase = Phase::Working;
        ensure_spinner(&mut spinner, &state);
        assert!(spinner.is_some(), "busy state arms 100ms spinner");

        state.phase = Phase::Idle;
        ensure_spinner(&mut spinner, &state);
        assert!(spinner.is_none(), "spinner must stop when idle again");
    }

    #[tokio::test]
    async fn ctrl_c_empty_quits_and_f2_toggles_sidebar() {
        let (handle, _rx) = start_runtime(ScriptedProvider::new(vec![text_script("x")]));
        let mut state = AppState::new("m".into(), PathBuf::from("."));
        on_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &handle,
        );
        assert!(state.should_quit);

        let mut state2 = AppState::new("m".into(), PathBuf::from("."));
        assert!(state2.sidebar);
        on_key(&mut state2, &k(KeyCode::F(2)), &handle);
        assert!(!state2.sidebar);
        handle.shutdown();
    }
}
