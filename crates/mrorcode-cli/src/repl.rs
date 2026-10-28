//! 裸 ANSI 交互 REPL（替代原 TUI）。
//!
//! 设计：不进 alternate screen、不做全屏重绘；所有输出走普通 stdout 流式打印，
//! 输入用 rustyline（纯 Rust 行编辑，Emacs 键位 + 历史）。
//!
//! 时序（两阶段循环，无 async select）：
//! - idle：阻塞读一行 prompt（行编辑线程在 std 线程里，不占 async worker）。
//! - working：`assembly.events.recv().await` 异步 drain 事件，直到 TurnComplete / Cancelled。
//! - 审批请求（`kind == "approval"`）在 working 循环里同步阻塞 stdin 问 y/n，再
//!   `respond` 回 runtime——此时 agent 已在等响应，阻塞是安全的。

use std::borrow::Cow;
use std::io::IsTerminal;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::sync::OnceLock;
use std::thread;

use rustyline::completion::Completer;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Editor, Helper};
use serde_json::{json, Value};

use mrorcode_agent::{Assembly, PermissionMode, RuntimeEvent};
use mrorcode_kernel::event::AgentEvent;

/// REPL 复用同一个会话 id，使多轮 prompt 共享上下文（对应原 TUI 单会话）。
const REPL_SESSION: u64 = 1;

/// 是否启用 ANSI 颜色：stdout 是终端且未设置 `NO_COLOR`（https://no-color.org）。
///
/// 管道/重定向输出自动退化为纯文本，避免把转义码写进日志或 CI 产物。
fn color_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
    })
}

/// 用 SGR 码包裹文本；未启用颜色时原样返回。
fn paint(code: &str, text: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

// 语义化配色（标准 16 色，兼容性最好）。
fn cyan(s: &str) -> String {
    paint("36", s)
}
fn green(s: &str) -> String {
    paint("32", s)
}
fn red(s: &str) -> String {
    paint("31", s)
}
fn yellow(s: &str) -> String {
    paint("33", s)
}
fn dim(s: &str) -> String {
    paint("2", s)
}
fn bold(s: &str) -> String {
    paint("1", s)
}

/// 行编辑线程的请求枚举：要么读用户 prompt，要么读审批 y/n。
enum ReplRequest {
    Prompt,
    Approval { tool: String, args_preview: String },
}

/// 行编辑线程的回传结果。
enum ReplOutcome {
    /// 用户输入了一行（prompt 或 y/n）。
    Line(String),
    /// Ctrl-C：丢弃当前行，继续提示。
    Interrupted,
    /// Ctrl-D / EOF：退出。
    Eof,
}

/// 行编辑 helper：只负责把提示符染成彩色。
///
/// rustyline 的 `readline(prompt)` 参数按**可见字符**计算宽度，若直接塞入 SGR
/// 转义序列（如 `\x1b[36m› \x1b[0m`），那 9 个不可见字符会被算成 9 列，导致
/// 光标定位偏移、输入行前冒出空格（issue #1）。颜色必须走 helper 的
/// `colored_prompt`：该路径算宽度时会忽略 ANSI 序列。
#[derive(Default)]
struct ReplHelper {
    /// 带颜色的提示符；为空时回退到 `readline` 传入的纯文本。
    colored_prompt: String,
}

impl Completer for ReplHelper {
    type Candidate = String;
}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Highlighter for ReplHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        if self.colored_prompt.is_empty() {
            Cow::Borrowed(prompt)
        } else {
            Cow::Borrowed(&self.colored_prompt)
        }
    }
}

impl Validator for ReplHelper {}

impl Helper for ReplHelper {}

/// 启动裸 ANSI REPL，直到用户退出。
pub async fn run_repl(assembly: &mut Assembly) -> anyhow::Result<()> {
    let history_path = history_path();
    let (req_tx, req_rx) = std_mpsc::channel::<ReplRequest>();
    let (out_tx, out_rx) = std_mpsc::channel::<ReplOutcome>();

    // 行编辑线程：持有 rustyline Editor，串行处理请求。
    thread::spawn(move || {
        let mut rl: Editor<ReplHelper, rustyline::history::DefaultHistory> =
            match Editor::new() {
                Ok(e) => e,
                Err(_) => return,
            };
        rl.set_helper(Some(ReplHelper::default()));
        if let Some(path) = &history_path {
            let _ = rl.load_history(path);
        }
        for req in req_rx {
            let outcome = match req {
                ReplRequest::Prompt => {
                    // 颜色只放 colored_prompt；readline 仍收纯文本（见 ReplHelper 注释）。
                    if let Some(h) = rl.helper_mut() {
                        h.colored_prompt = cyan("› ");
                    }
                    match rl.readline("› ") {
                        Ok(line) => {
                            let _ = rl.add_history_entry(&line);
                            if let Some(path) = &history_path {
                                let _ = rl.save_history(path);
                            }
                            ReplOutcome::Line(line)
                        }
                        // Ctrl-C 丢行不退出；Ctrl-D 及其它输入错误退出（`ReadlineError` 非穷尽）。
                        Err(rustyline::error::ReadlineError::Interrupted) => {
                            ReplOutcome::Interrupted
                        }
                        Err(_) => ReplOutcome::Eof,
                    }
                }
                ReplRequest::Approval { tool, args_preview } => {
                    let plain = format!("⚠ allow {tool} {args_preview}? [y/N] ");
                    // colored_prompt 必须与 plain 的可见文本完全一致，否则宽度会算错。
                    if let Some(h) = rl.helper_mut() {
                        h.colored_prompt = format!(
                            "{} allow {} {}? {} ",
                            yellow("⚠"),
                            bold(&tool),
                            dim(&args_preview),
                            bold("[y/N]")
                        );
                    }
                    match rl.readline(&plain) {
                        Ok(line) => ReplOutcome::Line(line),
                        // 审批读取失败一律按拒绝处理（fail-closed）。
                        Err(_) => ReplOutcome::Line("n".into()),
                    }
                }
            };
            if out_tx.send(outcome).is_err() {
                break;
            }
        }
    });

    let mut out = std::io::stdout();
    // 默认完全访问：REPL 是本地交互工具，工具审批默认放行（`/mode` 可随时收回）。
    let mut mode = PermissionMode::Full;
    assembly.handle.set_mode(mode);
    let _ = writeln!(
        out,
        "{} {}",
        bold(&cyan("mrorcode")),
        dim("(bare ANSI REPL)")
    );
    let _ = writeln!(
        out,
        "{}  {}\n{}",
        dim("permission:"),
        green(mode_name(mode)),
        dim("· /help for commands · /exit or Ctrl-D to quit")
    );

    loop {
        // ---- idle：阻塞读 prompt ----
        req_tx.send(ReplRequest::Prompt).ok();
        let line = match recv_outcome(&out_rx) {
            ReplOutcome::Eof => {
                let _ = writeln!(out, "\nbye.");
                break;
            }
            // Ctrl-C：丢弃当前行，重新提示。
            ReplOutcome::Interrupted => continue,
            ReplOutcome::Line(l) => l,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // 斜杠命令：命中即处理，不发给模型。
        if let Some(body) = trimmed.strip_prefix('/') {
            if run_slash(body, assembly, &mut out, &mut mode)? {
                let _ = writeln!(out, "bye.");
                break;
            }
            continue;
        }
        let _ = writeln!(out);
        assembly.handle.submit(REPL_SESSION, trimmed.to_string());

        // ---- working：drain 事件直到终态 ----
        while let Some(ev) = assembly.events.recv().await {
            match ev {
                RuntimeEvent::Agent { event, .. } => {
                    if handle_event(assembly, event, &req_tx, &out_rx, &mut out)? {
                        break;
                    }
                }
                RuntimeEvent::Exited => break,
            }
        }
    }

    assembly.handle.shutdown();
    Ok(())
}

/// 处理单个事件；返回 `true` 表示这一轮已结束（回到 idle 读下一个 prompt）。
fn handle_event(
    assembly: &Assembly,
    ev: AgentEvent,
    req_tx: &std_mpsc::Sender<ReplRequest>,
    out_rx: &std_mpsc::Receiver<ReplOutcome>,
    out: &mut std::io::Stdout,
) -> anyhow::Result<bool> {
    match ev {
        AgentEvent::TextDelta(t) => {
            let _ = write!(out, "{t}");
            let _ = out.flush();
        }
        AgentEvent::Reasoning(_) => {}
        AgentEvent::ToolStarted {
            name, arguments, ..
        } => {
            let preview = one_line_preview(&arguments);
            let _ = writeln!(out, "{} {} {}", cyan("$"), bold(&name), dim(&preview));
        }
        AgentEvent::ToolResult {
            name,
            ref content,
            is_error,
            ..
        } => {
            let mark = if is_error { red("✗") } else { green("✓") };
            let _ = writeln!(out, "{mark} {}", bold(&name));
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                for line in trimmed.lines().take(20) {
                    let _ = writeln!(out, "  {line}");
                }
                if trimmed.lines().count() > 20 {
                    let _ = writeln!(out, "  {}", dim("...[output truncated]"));
                }
            }
        }
        AgentEvent::Request { id, kind, payload } if kind == "approval" => {
            let tool = payload
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            // payload.arguments 是 JSON 对象；取 command/path 作单行预览（与 headless 一致）。
            let args_preview = payload
                .get("arguments")
                .map(|v| v.to_string())
                .map(|s| one_line_preview(&s))
                .unwrap_or_default();
            req_tx
                .send(ReplRequest::Approval {
                    tool,
                    args_preview,
                })
                .ok();
            let decision = match recv_outcome(out_rx) {
                ReplOutcome::Line(line) => {
                    let c = line.trim().to_ascii_lowercase();
                    if c == "y" || c == "yes" {
                        "allow"
                    } else {
                        "deny"
                    }
                }
                ReplOutcome::Eof => "deny",
                // 审批时 Ctrl-C 也走拒绝（fail-closed）。
                ReplOutcome::Interrupted => "deny",
            };
            let value = if decision == "allow" {
                json!({"decision": "allow"})
            } else {
                json!({"decision": "deny", "reason": "user rejected in REPL"})
            };
            assembly.handle.respond(id, value);
        }
        AgentEvent::Request { id, .. } => {
            // 非审批类请求：fail-closed（与 headless 一致）。
            assembly.handle.respond(
                id,
                json!({"decision": "deny", "reason": "REPL: non-approval requests auto-denied"}),
            );
        }
        AgentEvent::TurnComplete { .. } => {
            let _ = writeln!(out);
            return Ok(true);
        }
        AgentEvent::Cancelled => {
            let _ = writeln!(out, "{}", dim("(cancelled)"));
            return Ok(true);
        }
        AgentEvent::Error { message, .. } => {
            eprintln!("{} {message}", red("error:"));
        }
        AgentEvent::SubAgent { event, .. } => {
            // 子 agent 事件递归渲染（REPL 不嵌套审批，子请求不会出现）。
            return handle_event(assembly, *event, req_tx, out_rx, out);
        }
        AgentEvent::TurnStarted | AgentEvent::Usage { .. } => {}
    }
    Ok(false)
}

/// 处理斜杠命令；返回 `true` 表示退出 REPL。
fn run_slash(
    body: &str,
    assembly: &mut Assembly,
    out: &mut std::io::Stdout,
    mode: &mut PermissionMode,
) -> anyhow::Result<bool> {
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let arg = parts.next().unwrap_or("").trim();

    match name.as_str() {
        "exit" | "quit" | "q" => return Ok(true),
        "help" | "h" | "?" => {
            let _ = writeln!(
                out,
                "{}   {}\n{}\n{}\n{}\n{}",
                bold(&cyan("/help")),
                dim("show this help"),
                format!("{} {}", bold(&cyan("/model [name]")), dim("show or switch model")),
                format!(
                    "{} {}",
                    bold(&cyan("/mode [ro|rw|full]")),
                    dim("show or set permission mode (no arg = cycle)")
                ),
                format!(
                    "{} {}",
                    bold(&cyan("/interrupt")),
                    dim("cancel the running turn")
                ),
                format!("{} {}", bold(&cyan("/exit, /quit")), dim("quit")),
            );
        }
        "model" | "m" => {
            if arg.is_empty() {
                let _ = writeln!(out, "{} {}", dim("model:"), bold(&assembly.model));
            } else {
                assembly.handle.set_model(arg);
                assembly.model = arg.to_string();
                let _ = writeln!(out, "{} {}", dim("model →"), bold(arg));
            }
        }
        "mode" => {
            if arg.is_empty() {
                // 无参数：循环切换 Workspace → ReadOnly → Full → Workspace。
                *mode = match *mode {
                    PermissionMode::Workspace => PermissionMode::ReadOnly,
                    PermissionMode::ReadOnly => PermissionMode::Full,
                    PermissionMode::Full => PermissionMode::Workspace,
                };
            } else {
                match arg.to_ascii_lowercase().as_str() {
                    "ro" | "readonly" | "read-only" | "plan" => *mode = PermissionMode::ReadOnly,
                    "rw" | "workspace" | "write" => *mode = PermissionMode::Workspace,
                    "full" | "all" | "yolo" => *mode = PermissionMode::Full,
                    _ => {
                        let _ = writeln!(
                            out,
                            "{} {}",
                            red("unknown mode:"),
                            format!("`{arg}` (use ro | rw | full)")
                        );
                        return Ok(false);
                    }
                }
            }
            assembly.handle.set_mode(*mode);
            let _ = writeln!(out, "{} {}", dim("mode →"), mode_color(*mode, mode_name(*mode)));
        }
        "interrupt" | "cancel" | "esc" => {
            assembly.handle.cancel();
            let _ = writeln!(out, "{}", dim("(interrupt sent)"));
        }
        other => {
            let _ = writeln!(
                out,
                "{} {} {}",
                red("unknown command:"),
                format!("`/{other}`"),
                dim("(/help for list)")
            );
        }
    }
    Ok(false)
}

/// 权限模式的显示名。
fn mode_name(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "read-only",
        PermissionMode::Workspace => "workspace",
        PermissionMode::Full => "full",
    }
}

/// 权限模式着色：只读=黄（受限）、工作区=青、完全=绿（放行）。
fn mode_color(mode: PermissionMode, name: &str) -> String {
    match mode {
        PermissionMode::ReadOnly => yellow(name),
        PermissionMode::Workspace => cyan(name),
        PermissionMode::Full => green(name),
    }
}

/// 从阻塞 std channel 取结果；行编辑线程退出时返回 EOF。
fn recv_outcome(rx: &std_mpsc::Receiver<ReplOutcome>) -> ReplOutcome {
    rx.recv().unwrap_or(ReplOutcome::Eof)
}

/// 与 headless 一致的单行参数预览。
fn one_line_preview(arguments: &str) -> String {
    let parsed: Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let cmd = parsed
        .get("command")
        .and_then(|v| v.as_str())
        .or_else(|| parsed.get("path").and_then(|v| v.as_str()))
        .unwrap_or("");
    cmd.chars().take(120).collect()
}

/// 历史文件路径：`~/.mrorcode/history`。
fn history_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let dir = home.join(".mrorcode");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("history"))
}
