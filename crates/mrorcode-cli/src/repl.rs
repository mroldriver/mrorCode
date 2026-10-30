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
use std::collections::HashMap;
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

use crate::runlog::RunLog;

/// REPL 复用同一个会话 id，使多轮 prompt 共享上下文（对应原 TUI 单会话）。
const REPL_SESSION: u64 = 1;

/// 是否启用 ANSI 颜色：stdout 是终端且未设置 `NO_COLOR`（https://no-color.org）。
///
/// 管道/重定向输出自动退化为纯文本，避免把转义码写进日志或 CI 产物。
fn color_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
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
    /// 审批：`summary` 已是「读取文件(path)」形态的现成文本，行编辑线程只负责排版。
    Approval {
        summary: String,
    },
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
pub async fn run_repl(assembly: &mut Assembly, log: Option<RunLog>) -> anyhow::Result<()> {
    let history_path = history_path();
    let (req_tx, req_rx) = std_mpsc::channel::<ReplRequest>();
    let (out_tx, out_rx) = std_mpsc::channel::<ReplOutcome>();

    // 行编辑线程：持有 rustyline Editor，串行处理请求。
    thread::spawn(move || {
        let mut rl: Editor<ReplHelper, rustyline::history::DefaultHistory> = match Editor::new() {
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
                ReplRequest::Approval { summary } => {
                    let plain = format!("⚠ 允许 {summary} 吗？[y/N] ");
                    // colored_prompt 必须与 plain 的可见文本完全一致，否则宽度会算错。
                    if let Some(h) = rl.helper_mut() {
                        h.colored_prompt = format!(
                            "{} 允许 {} 吗？{} ",
                            yellow("⚠"),
                            bold(&summary),
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
        dim("（裸 ANSI 交互界面）")
    );
    let _ = writeln!(
        out,
        "{}  {}\n{}\n",
        dim("权限："),
        green(mode_name(mode)),
        dim("· /help 查看命令 · /exit 或 Ctrl-D 退出")
    );

    // 输出排版状态（行首感知 + 工具卡表），跨轮复用。
    let mut render = ReplRender::new();

    loop {
        req_tx.send(ReplRequest::Prompt).ok();
        let line = match recv_outcome(&out_rx) {
            ReplOutcome::Eof => {
                let _ = writeln!(out, "\n再见。");
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
            if let Some(log) = &log {
                log.slash(body);
            }
            if run_slash(body, assembly, &mut out, &mut mode, &mut render)? {
                let _ = writeln!(out, "再见。");
                break;
            }
            continue;
        }
        if let Some(log) = &log {
            log.user(trimmed);
        }
        let _ = writeln!(out);
        // 主循环自己写了这行空行，同步排版状态（下一张工具卡不会再补一个）。
        render.note_blank();
        assembly.handle.submit(REPL_SESSION, trimmed.to_string());

        // ---- working：drain 事件直到终态 ----
        while let Some(ev) = assembly.events.recv().await {
            match ev {
                RuntimeEvent::Agent { event, .. } => {
                    if handle_event(
                        assembly,
                        event,
                        &req_tx,
                        &out_rx,
                        &mut out,
                        &mut render,
                        log.as_ref(),
                    )? {
                        break;
                    }
                }
                RuntimeEvent::Exited => break,
            }
        }
    }

    if let Some(log) = &log {
        log.flush_assistant();
    }
    assembly.handle.shutdown();
    Ok(())
}

/// REPL 输出排版状态：**只**负责「下一段从哪一行开始」和工具卡文本。
///
/// 流式正文可能停在句子中间（`我来查一下。`），而工具卡/结果行必须是独立的一行，
/// 轮末还要留空行给下一次输入——三者都依赖「光标现在是否在行首」这一个事实，
/// 所以集中放在这里，避免每个分支各自数换行。
struct ReplRender {
    /// 上一次输出之后光标是否已经在行首（初始为 true：屏幕上是干净的一行）。
    at_line_start: bool,
    /// 上一次写出的内容就是空行——`blank()` 据此保持幂等（连写两次只留一个空行）。
    blank_emitted: bool,
    /// 工具调用 id → 工具卡备忘（结果行据此去重参数），供 `ToolResult` 复用同一行前缀。
    pending: HashMap<String, PendingTool>,
    /// 本轮已流式写出的正文（用于长回复折叠与 `/full` 重放）。
    turn_text: String,
    /// 本轮已实时写出的行数（折叠阈值判断）。
    emitted_newlines: usize,
    /// 已超过阈值：后续正文只累积不输出。
    folded: bool,
    /// 上一轮的完整正文；`/full` 重放它。
    last_turn: Option<String>,
    /// 折叠阈值（行）。可用 `MRORCODE_REPL_PREVIEW_LINES` 覆盖。
    preview_lines: usize,
}

/// 长回复折叠提示里的省略号前缀。
const FOLD_HINT: &str = "… 已折叠";

/// 取 `s` 的前 `max_newlines` 行（含行尾换行符），返回（前缀，是否还有剩余）。
///
/// 用来把「一次 delta 跨越多行」按行切开，保证折叠点落在行边界上：
/// 既不会因为一次给了 100 行就整段丢弃，也不会整段放出来。
fn take_lines(s: &str, max_newlines: usize) -> (&str, bool) {
    if max_newlines == 0 {
        return ("", !s.is_empty());
    }
    let mut count = 0;
    for (idx, ch) in s.char_indices() {
        if ch == '\n' {
            count += 1;
            if count == max_newlines {
                let end = idx + ch.len_utf8();
                return (&s[..end], end < s.len());
            }
        }
    }
    (s, false)
}

impl ReplRender {
    fn new() -> Self {
        let preview_lines = std::env::var("MRORCODE_REPL_PREVIEW_LINES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(15);
        Self {
            at_line_start: true,
            blank_emitted: false,
            pending: HashMap::new(),
            turn_text: String::new(),
            emitted_newlines: 0,
            folded: false,
            last_turn: None,
            preview_lines,
        }
    }

    /// 流式正文：原样写出，并记录落点是否在行首。
    /// 跳过 LLM 输出的纯 `...` 过渡行（如思考停顿的省略号）。
    ///
    /// 超过折叠阈值后不再实时输出（issue #11 的 4-A）：文本继续累积，轮末给一行
    /// 灰色提示，用户可以 `/full` 重放。短回复（大多数情况）行为完全不变。
    /// 单个 delta 跨过阈值时按行切开——只写到阈值那一行，避免「一次给 100 行」时
    /// 要么全丢、要么全放。
    fn text(&mut self, out: &mut dyn Write, t: &str) {
        let trimmed = t.trim();
        if trimmed == "..." || trimmed == "…" {
            return;
        }
        self.turn_text.push_str(t);
        if self.folded {
            return;
        }
        let budget = self.preview_lines.saturating_sub(self.emitted_newlines);
        let (head, truncated) = take_lines(t, budget);
        if !head.is_empty() {
            let _ = write!(out, "{head}");
            let _ = out.flush();
            self.blank_emitted = false;
            if let Some(last) = head.chars().last() {
                self.at_line_start = last == '\n';
            }
        }
        self.emitted_newlines += head.matches('\n').count();
        if truncated {
            // 收尾当前行，避免折叠提示粘在正文中间。
            self.folded = true;
            self.break_line(out);
        }
    }

    /// 保证下一段输出另起一行；已在新行首时是空操作（不产生多余空行）。
    fn break_line(&mut self, out: &mut dyn Write) {
        if !self.at_line_start {
            let _ = writeln!(out);
            self.at_line_start = true;
        }
    }

    /// 独立成行（工具卡、结果行）：先另起一行，再整行写出。
    fn line(&mut self, out: &mut dyn Write, s: &str) {
        self.break_line(out);
        let _ = writeln!(out, "{s}");
        self.at_line_start = true;
        self.blank_emitted = false;
    }

    /// 空行——「呼吸感」：段落之间、轮末与下一次输入之间各留一行。
    ///
    /// 幂等：已经有一个空行时再调用不会叠出第二个（结果行后本就留了空行）。
    fn blank(&mut self, out: &mut dyn Write) {
        self.break_line(out);
        if self.blank_emitted {
            return;
        }
        let _ = writeln!(out);
        self.at_line_start = true;
        self.blank_emitted = true;
    }

    /// 外部（主循环的 `writeln!`、rustyline 审批提示）已经写过换行/空行，同步状态。
    fn note_blank(&mut self) {
        self.at_line_start = true;
        self.blank_emitted = true;
    }

    /// 本轮结束：若发生过折叠，补一行提示；并记下完整正文供 `/full`。
    fn finish_turn(&mut self, out: &mut dyn Write) {
        let text = std::mem::take(&mut self.turn_text);
        self.emitted_newlines = 0;
        if text.trim().is_empty() {
            self.last_turn = None;
            self.folded = false;
            return;
        }
        if self.folded {
            let lines = text.lines().count();
            let hidden = lines.saturating_sub(self.preview_lines);
            self.break_line(out);
            let _ = writeln!(
                out,
                "{}",
                dim(&format!(
                    "{FOLD_HINT} {hidden} 行（共 {lines} 行；输入 /full 查看全文）"
                ))
            );
            self.at_line_start = true;
            self.blank_emitted = false;
        }
        self.last_turn = Some(text);
        self.folded = false;
    }

    /// `/full`：把上一轮被折叠的完整正文重放到屏幕。
    ///
    /// 返回 `false` 表示没有可展开的内容。
    fn replay_last_turn(&mut self, out: &mut dyn Write) -> bool {
        match self.last_turn.take() {
            Some(text) if !text.trim().is_empty() => {
                self.blank(out);
                let _ = write!(out, "{text}");
                if !text.ends_with('\n') {
                    let _ = writeln!(out);
                }
                self.at_line_start = true;
                self.blank_emitted = false;
                true
            }
            _ => false,
        }
    }

    /// 外部（rustyline 审批提示）已经把光标送回了行首，这里同步状态。
    fn mark_line_start(&mut self) {
        self.at_line_start = true;
    }
}

/// 处理单个事件；返回 `true` 表示这一轮已结束（回到 idle 读下一个 prompt）。
fn handle_event(
    assembly: &Assembly,
    ev: AgentEvent,
    req_tx: &std_mpsc::Sender<ReplRequest>,
    out_rx: &std_mpsc::Receiver<ReplOutcome>,
    out: &mut std::io::Stdout,
    render: &mut ReplRender,
    log: Option<&RunLog>,
) -> anyhow::Result<bool> {
    if let Some(log) = log {
        log.event(&ev);
    }
    match ev {
        AgentEvent::TextDelta(t) => render.text(out, &t),
        AgentEvent::Reasoning(_) => {}
        AgentEvent::ToolStarted {
            id,
            name,
            arguments,
        } => {
            // 首行优先用模型自述的 intent（「为什么调用」）；没给就从参数反推「工具名(详情)」。
            let intent = tool_intent(&arguments);
            let header = intent
                .clone()
                .unwrap_or_else(|| tool_card(&name, &arguments));
            // 模型正文常停在句子中间，工具卡必须自己另起一行。
            // 正文与工具卡之间保持一个空行（issue #11 的 3），与「用户输入↔正文」一致。
            render.blank(out);
            render.line(out, &format!("{} {}", cyan("●"), bold(&header)));
            // 记下「首行已经表达了参数」这件事：结果行据此决定要不要再重复一遍参数。
            render.pending.insert(
                id,
                PendingTool {
                    card: tool_card(&name, &arguments),
                    intent_shown: intent.is_some(),
                },
            );
        }
        AgentEvent::ToolResult {
            id,
            name,
            ref content,
            is_error,
        } => {
            // 结果行复用开始时的工具卡文本；理论上 id 一定在表里，取不到就退回标签。
            let pending = render.pending.remove(&id).unwrap_or(PendingTool {
                card: tool_card(&name, "{}"),
                intent_shown: false,
            });
            let trimmed = content.trim();
            let mark = if is_error { red("✗") } else { green("✓") };
            let suffix = if is_error {
                dim("→ 失败")
            } else {
                let n = trimmed.lines().filter(|l| !l.trim().is_empty()).count();
                dim(&format!("→ {} 行", n.max(1)))
            };
            // 首行已经是 intent 时，参数只出现一次（issue #6）；没有 intent 时必须保留，
            // 否则参数彻底不可见——此时首行就是参数。
            let detail = result_detail(&pending);
            render.line(out, &format!("{} {mark}{detail} {suffix}", dim("└")));
            // 成功与失败都只保留一行：成功显示行数，失败只显示「→ 失败」，不展开正文。
            // 结果行之后留一行空白：下面的正文/下一张工具卡不贴着结果。
            render.blank(out);
        }
        AgentEvent::Request { id, kind, payload } if kind == "approval" => {
            // payload.arguments 是 JSON 对象；拼成「中文标签(详情)」交回行编辑线程排版。
            let card = payload
                .get("tool")
                .and_then(|v| v.as_str())
                .map(|tool| {
                    let args = payload
                        .get("arguments")
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    tool_card(tool, &args)
                })
                .unwrap_or_else(|| "未知操作".to_string());
            req_tx.send(ReplRequest::Approval { summary: card }).ok();
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
            // 用户在 y/n 提示里按了回车，光标已经回到行首——同步给排版状态。
            render.mark_line_start();
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
            // 长回复：补折叠提示；短回复这里什么也不做（issue #11 的 4-A）。
            render.finish_turn(out);
            // 本轮输出与下一次输入之间留一行空白。
            render.blank(out);
            return Ok(true);
        }
        AgentEvent::Cancelled => {
            render.finish_turn(out);
            render.line(out, &dim("（已取消）"));
            render.blank(out);
            return Ok(true);
        }
        AgentEvent::Error { message, .. } => {
            eprintln!("{} {message}", red("错误："));
        }
        AgentEvent::SubAgent { event, .. } => {
            // 子 agent 事件递归渲染（REPL 不嵌套审批，子请求不会出现）。
            return handle_event(assembly, *event, req_tx, out_rx, out, render, log);
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
    render: &mut ReplRender,
) -> anyhow::Result<bool> {
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let arg = parts.next().unwrap_or("").trim();

    match name.as_str() {
        "exit" | "quit" | "q" => return Ok(true),
        "help" | "h" | "?" => {
            let _ = writeln!(
                out,
                "{}   {}\n{} {}\n{} {}\n{} {}\n{} {}\n{} {}",
                bold(&cyan("/help")),
                dim("show this help"),
                bold(&cyan("/model [name]")),
                dim("show or switch model"),
                bold(&cyan("/mode [ro|rw|full]")),
                dim("show or set permission mode (no arg = cycle)"),
                bold(&cyan("/interrupt")),
                dim("cancel the running turn"),
                bold(&cyan("/full")),
                dim("replay the full text of the last turn (when folded)"),
                bold(&cyan("/exit, /quit")),
                dim("quit"),
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
                        let _ =
                            writeln!(out, "{} `{arg}` (use ro | rw | full)", red("unknown mode:"));
                        return Ok(false);
                    }
                }
            }
            assembly.handle.set_mode(*mode);
            let _ = writeln!(
                out,
                "{} {}",
                dim("mode →"),
                mode_color(*mode, mode_name(*mode))
            );
        }
        "interrupt" | "cancel" | "esc" => {
            assembly.handle.cancel();
            let _ = writeln!(out, "{}", dim("（已发送中断）"));
        }
        "full" => {
            if !render.replay_last_turn(out) {
                let _ = writeln!(out, "{}", dim("(没有可展开的内容)"));
            }
        }
        other => {
            let _ = writeln!(
                out,
                "{} `/{other}` {}",
                red("unknown command:"),
                dim("(/help for list)")
            );
        }
    }
    Ok(false)
}

/// 权限模式的显示名。
fn mode_name(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "只读",
        PermissionMode::Workspace => "工作区",
        PermissionMode::Full => "完全",
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

// ---- 工具卡：一行说清「调用了哪个工具、对谁动手」 ------------------------
//
// 形态对标 atomcode 的 `● Read(mod.rs)`，只把工具名换成中文动词短语：
//
//   ● 读取文件(crates/mrorcode-cli/src/repl.rs)
//   └ ✓ 读取文件(crates/mrorcode-cli/src/repl.rs) → 484 行
//
// 结果行复用开始时那行文本（`pending` 按 ToolStarted 的 id 存），裸 ANSI 流里
// 上下两行才对得上号；只有失败才展开正文，成功保持一行。

/// 工具中文标签；未知工具（含将来的 MCP 工具）返回 `None`，由调用方回退到原名。
fn tool_label(name: &str) -> Option<&'static str> {
    Some(match name {
        "read" => "读取文件",
        "write" => "写入文件",
        "edit" => "编辑文件",
        "bash" => "执行命令",
        "grep" => "搜索内容",
        "glob" => "查找文件",
        "task" => "派生任务",
        _ => return None,
    })
}

/// 从参数 JSON 取一行「在干什么」的详情；取不到时返回空串（工具卡只留标签）。
fn tool_detail(name: &str, arguments: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(arguments) else {
        return String::new();
    };
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    match name {
        // 路径保留原样：模型给的多是相对工作目录的写法，尾部才是关键信息。
        "read" | "write" | "edit" => clip_tail(get("path"), 100),
        "bash" => clip_head(get("command"), 120),
        "grep" | "glob" => clip_head(get("pattern"), 100),
        // 子任务优先用简短标签，没有就取 prompt 第一行。
        "task" => {
            let label = get("description");
            if !label.is_empty() {
                clip_head(label, 60)
            } else {
                clip_head(get("prompt").lines().next().unwrap_or(""), 100)
            }
        }
        // 未知工具：按常见参数名猜一个能辨认的细节。
        _ => {
            let s = ["command", "path", "file_path", "pattern", "query", "url"]
                .iter()
                .map(|k| get(k))
                .find(|s| !s.is_empty())
                .unwrap_or("");
            clip_head(s, 100)
        }
    }
}

/// 工具卡正文：`读取文件(repl.rs)`；详情为空时只留标签。
fn tool_card(name: &str, arguments: &str) -> String {
    let label = tool_label(name)
        .map(str::to_string)
        .unwrap_or_else(|| format!("调用 {name}"));
    let detail = tool_detail(name, arguments);
    if detail.is_empty() {
        label
    } else {
        format!("{label}({detail})")
    }
}

/// 工具调用「已开始、未结束」期间的备忘。
struct PendingTool {
    /// 工具卡文本（`读取文件(x.rs)`）；首行没有 intent 时，结果行要靠它显示参数。
    card: String,
    /// 首行是否已经是模型给的 intent——是的话参数只需出现一次。
    intent_shown: bool,
}

/// 结果行里的参数部分：首行已显示 intent 时留空（去重），否则回退到工具卡。
///
/// 去重只在「首行已是意图」时成立：没有 intent 时首行就是参数，结果行再不显示，
/// 参数就从屏幕上彻底消失了。
fn result_detail(pending: &PendingTool) -> String {
    if pending.intent_shown {
        String::new()
    } else {
        format!(" {}", pending.card)
    }
}

/// 模型自述的调用意图（工具参数里的 `intent`，由 kernel 的 `defs()` 统一注入）。
///
/// 空白串按缺失处理：宁可回退到「工具名(参数)」，也不要在屏幕上留一个空的 `●`。
fn tool_intent(arguments: &str) -> Option<String> {
    let v: Value = serde_json::from_str(arguments).ok()?;
    let intent = v.get(mrorcode_kernel::tool::INTENT_PARAM)?.as_str()?.trim();
    (!intent.is_empty()).then(|| clip_head(intent, 120))
}

/// 按**字符**（非字节）截断，超长时保留开头——命令、正则这类开头就说明用途。
fn clip_head(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// 按字符截断，超长时保留结尾——路径用（文件名比开头更有信息量）。
fn clip_tail(s: &str, max: usize) -> String {
    let s = s.trim();
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let mut out = String::from("…");
    out.extend(s.chars().skip(n - max.saturating_sub(1)));
    out
}

/// 历史文件路径：`~/.mrorcode/history`。
fn history_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let dir = home.join(".mrorcode");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("history"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tool_labels_are_chinese() {
        assert_eq!(tool_label("read"), Some("读取文件"));
        assert_eq!(tool_label("bash"), Some("执行命令"));
        assert_eq!(tool_label("grep"), Some("搜索内容"));
        // 未知工具不猜中文名，由 tool_card 回退成 `调用 <原名>`。
        assert_eq!(tool_label("mcp__x__y"), None);
    }

    #[test]
    fn permission_mode_names_are_chinese() {
        assert_eq!(mode_name(PermissionMode::ReadOnly), "只读");
        assert_eq!(mode_name(PermissionMode::Workspace), "工作区");
        assert_eq!(mode_name(PermissionMode::Full), "完全");
    }

    #[test]
    fn tool_card_puts_detail_in_parens() {
        let card = tool_card("read", r#"{"path":"crates/mrorcode-cli/src/repl.rs"}"#);
        assert_eq!(card, "读取文件(crates/mrorcode-cli/src/repl.rs)");
        let card = tool_card("bash", r#"{"command":"cargo check --workspace"}"#);
        assert_eq!(card, "执行命令(cargo check --workspace)");
        let card = tool_card("glob", r#"{"pattern":"**/*.rs"}"#);
        assert_eq!(card, "查找文件(**/*.rs)");
    }

    #[test]
    fn tool_card_falls_back_to_label_or_raw_name() {
        // 参数非法/为空：只留标签，不出现空括号。
        assert_eq!(tool_card("write", "not json"), "写入文件");
        assert_eq!(tool_card("write", "{}"), "写入文件");
        // 未知工具保留原名，便于识别（将来接 MCP 也不会显示成空壳）。
        assert_eq!(
            tool_card("mcp__fs__stat", r#"{"path":"a.txt"}"#),
            "调用 mcp__fs__stat(a.txt)"
        );
    }

    #[test]
    fn task_card_prefers_description_then_prompt_first_line() {
        assert_eq!(
            tool_card(
                "task",
                r#"{"description":"查日志","prompt":"第一行\n第二行"}"#
            ),
            "派生任务(查日志)"
        );
        assert_eq!(
            tool_card("task", r#"{"prompt":"第一行\n第二行"}"#),
            "派生任务(第一行)"
        );
    }

    #[test]
    fn clip_keeps_the_informative_end_for_paths() {
        let long = "a/very/deep/path/that/goes/on/and/on/file.rs";
        let clipped = clip_tail(long, 20);
        assert_eq!(clipped.chars().count(), 20);
        assert!(clipped.starts_with('…'));
        assert!(clipped.ends_with("file.rs"));
        // 未超长时原样返回（不去动正常路径）。
        assert_eq!(clip_tail("src/main.rs", 100), "src/main.rs");
    }

    #[test]
    fn clip_head_is_char_safe_for_chinese() {
        // 按字符而不是字节截断：中文不会出现半个字（也不会 panic）。
        let clipped = clip_head("搜索内容搜索内容搜索内容", 5);
        assert_eq!(clipped.chars().count(), 5);
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn intent_becomes_the_card_header() {
        // 模型给了 intent：首行就是那句意图，不再显示「工具名(参数)」。
        let args = r#"{"command":"cargo test","intent":"跑一遍工作区测试"}"#;
        let header = tool_intent(args).unwrap_or_else(|| tool_card("bash", args));
        assert_eq!(header, "跑一遍工作区测试");
    }

    #[test]
    fn header_falls_back_to_tool_card_without_usable_intent() {
        // 漏填 / 全空格 / 参数不是 JSON：一律回退，绝不出现空首行。
        let args = r#"{"command":"cargo test"}"#;
        assert_eq!(tool_intent(args), None);
        let header = tool_intent(args).unwrap_or_else(|| tool_card("bash", args));
        assert_eq!(header, "执行命令(cargo test)");
        assert_eq!(tool_intent(r#"{"command":"ls","intent":"   "}"#), None);
        assert_eq!(tool_intent("not json"), None);
    }

    #[test]
    fn result_line_skips_arguments_only_when_intent_was_shown() {
        // 首行是 intent：结果行只留状态与行数，参数不重复（issue #6）。
        let with_intent = PendingTool {
            card: "执行命令(git log -5 --stat)".to_string(),
            intent_shown: true,
        };
        assert_eq!(result_detail(&with_intent), "");

        // 没有 intent：首行就是参数，结果行必须保留，否则参数从屏幕上消失。
        let without_intent = PendingTool {
            card: "执行命令(git log -5 --stat)".to_string(),
            intent_shown: false,
        };
        assert_eq!(
            result_detail(&without_intent),
            " 执行命令(git log -5 --stat)"
        );
    }

    /// 跑一段事件序列，返回写到「屏幕」上的纯文本（不经过配色函数，断言稳定）。
    fn render_segments(segments: &[(char, &str)]) -> String {
        let mut render = ReplRender::new();
        let mut out: Vec<u8> = Vec::new();
        for (kind, text) in segments {
            match kind {
                't' => render.text(&mut out, text),
                'l' => render.line(&mut out, text),
                'b' => render.blank(&mut out),
                _ => unreachable!("未知片段类型: {kind}"),
            }
        }
        String::from_utf8(out).expect("渲染输出必须是合法 UTF-8")
    }

    #[test]
    fn tool_card_gets_its_own_line_even_mid_sentence() {
        // 模型正文停在句子中间时，`● 工具卡` 必须自己换行，不能粘在正文后面。
        let text = render_segments(&[('t', "我来看看。"), ('l', "● 执行命令(ls)")]);
        assert_eq!(text, "我来看看。\n● 执行命令(ls)\n");
    }

    #[test]
    fn card_and_turn_end_each_leave_one_blank_line() {
        // 结果行后空一行、整轮结束再空一行——给人读的节奏。
        let text = render_segments(&[
            ('t', "我来看看。"),
            ('l', "● 执行命令(git status)"),
            ('l', "└ ✓ 执行命令(git status) → 14 行"),
            ('b', ""),
            ('t', "工作区是干净的。"),
            ('b', ""),
        ]);
        assert_eq!(
            text,
            "我来看看。\n● 执行命令(git status)\n└ ✓ 执行命令(git status) → 14 行\n\n\
             工作区是干净的。\n\n"
        );
    }

    #[test]
    fn line_start_is_tracked_so_no_double_newline_is_inserted() {
        // 正文已经以 \n 结尾时，工具卡不再补一个空行（否则会出现两行空白）。
        let text = render_segments(&[('t', "第一行\n"), ('l', "● 执行命令(ls)")]);
        assert_eq!(text, "第一行\n● 执行命令(ls)\n");
        // blank() 幂等：连写两次仍然只有一个空行（结果行后本就留了空行）。
        let text = render_segments(&[('b', ""), ('b', "")]);
        assert_eq!(text, "\n");
    }

    #[test]
    fn blank_line_separates_body_and_tool_card() {
        // issue #11 的 3：正文与工具卡之间恰好一个空行，和「用户输入↔正文」一致。
        let text = render_segments(&[('t', "我先看看项目结构。"), ('b', ""), ('l', "● 查看目录")]);
        assert_eq!(text, "我先看看项目结构。\n\n● 查看目录\n");
        // 正文停在句子中间也一样：break_line 收尾再空一行，不会粘成一句。
        let text = render_segments(&[('t', "我先看看"), ('b', ""), ('l', "● 查看目录")]);
        assert_eq!(text, "我先看看\n\n● 查看目录\n");
    }

    #[test]
    fn long_turn_is_folded_with_hint_and_replayable() {
        // issue #11 的 4-A：超过阈值不再实时输出，轮末给折叠提示，/full 能重放全文。
        let mut render = ReplRender::new();
        render.preview_lines = 3;

        let mut out: Vec<u8> = Vec::new();
        for i in 0..10 {
            render.text(&mut out, &format!("line {i}\n"));
        }
        let streamed = String::from_utf8(out.clone()).unwrap();
        assert_eq!(streamed.lines().count(), 3, "streamed: {streamed}");

        render.finish_turn(&mut out);
        let after_turn = String::from_utf8(out).unwrap();
        assert!(after_turn.contains("已折叠 7 行（共 10 行"), "{after_turn}");
        assert!(after_turn.contains("/full"), "{after_turn}");

        let mut replay: Vec<u8> = Vec::new();
        assert!(render.replay_last_turn(&mut replay));
        let full = String::from_utf8(replay).unwrap();
        assert_eq!(
            full.lines().filter(|l| l.starts_with("line ")).count(),
            10,
            "full: {full}"
        );
        // 重放后清空：再次 /full 没有内容。
        assert!(!render.replay_last_turn(&mut Vec::new()));
    }

    #[test]
    fn single_big_delta_is_cut_at_the_fold_boundary() {
        // 一次 delta 给了 5 行：只写出前 3 行（不整段丢弃，也不整段放出来）。
        let mut render = ReplRender::new();
        render.preview_lines = 3;
        let mut out: Vec<u8> = Vec::new();
        render.text(&mut out, "a\nb\nc\nd\ne\n");
        assert_eq!(String::from_utf8(out.clone()).unwrap(), "a\nb\nc\n");
        render.finish_turn(&mut out);
        let all = String::from_utf8(out).unwrap();
        assert!(all.contains("已折叠 2 行（共 5 行"), "{all}");
    }

    #[test]
    fn short_turn_is_not_folded() {
        let mut render = ReplRender::new();
        render.preview_lines = 5;
        let mut out: Vec<u8> = Vec::new();
        render.text(&mut out, "短回复第一行\n第二行\n");
        render.finish_turn(&mut out);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "短回复第一行\n第二行\n");
        assert!(!text.contains("已折叠"));
    }
}
