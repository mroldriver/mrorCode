//! 运行日志（自动留痕）：每次进程启动写 `~/.mrorcode/logs/run-<时间戳>.log`。
//!
//! 设计取舍：
//! - **内容**：会话转录（用户输入 / 工具卡 / 助手正文 / 错误 / 用量）+ tracing 诊断行，
//!   两者落进同一个文件——排障时按时间戳就能把「用户说了什么 → 调了什么工具 → 报了什么错」
//!   串成一条链路。
//! - **脱敏（默认开启）**：`api_key` / `Authorization` / `Bearer` 与常见密钥前缀一律打码；
//!   工具输出只留摘要（首个非空行 + 总行数），不落整段文件内容。
//! - **热路径**：助手正文先攒在内存，工具卡 / 终态 / 错误才落盘；流式 delta 不写盘
//!   （与 AGENTS.md「性能是发布门禁」一致）。
//! - **滚动**：保留最近 [`KEEP`] 份，启动时清理更早的。
//! - **关闭 / 调整**：`MRORCODE_LOG=0|off|false` 关闭；`MRORCODE_LOG_DIR` 改目录；
//!   `MRORCODE_LOG_LEVEL` 调文件层级别（默认 `debug`，stderr 仍由 `RUST_LOG` 控制）。
//!
//! 时间戳统一用 UTC（不引入 `time`/`chrono`：跨线程取本地时区在 Windows 上不可靠）。

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use mrorcode_kernel::event::AgentEvent;
use regex::Regex;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

/// 保留的历史日志份数（按文件名倒序，多出的删掉）。
pub const KEEP: usize = 20;

/// 日志文件名前缀；滚动清理只认这个前缀。
const FILE_PREFIX: &str = "run-";

/// 单行最大字符数：工具参数/输出可能极大，超长截断。
const MAX_LINE_CHARS: usize = 4000;

/// 工具卡/结果摘要的最大字符数。
const MAX_SUMMARY_CHARS: usize = 160;

/// 助手正文缓冲上限：超过就先落盘，避免长回复把内存顶起来。
const ASSISTANT_FLUSH_CHARS: usize = 4000;

/// 一次进程运行的日志句柄（`Clone` 廉价：内部全是 `Arc`）。
#[derive(Clone)]
pub struct RunLog {
    inner: Arc<Inner>,
}

struct Inner {
    file: Mutex<File>,
    /// 助手流式正文缓冲；工具卡/终态时 flush。
    assistant: Mutex<String>,
    path: PathBuf,
}

impl RunLog {
    /// 启动本次运行日志；返回 `None` 表示被环境变量关闭或目录不可写。
    pub fn start() -> Option<Self> {
        if disabled_by_env() {
            return None;
        }
        Self::with_dir(&log_dir()?)
    }

    /// 在指定目录下开一份新日志（测试与 `MRORCODE_LOG_DIR` 用）。
    pub fn with_dir(dir: &Path) -> Option<Self> {
        std::fs::create_dir_all(dir).ok()?;
        prune(dir, KEEP);
        let path = unique_path(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        let log = Self {
            inner: Arc::new(Inner {
                file: Mutex::new(file),
                assistant: Mutex::new(String::new()),
                path,
            }),
        };
        log.write_line(&format!(
            "mrorcode {} run log（时间戳 UTC）",
            env!("CARGO_PKG_VERSION")
        ));
        if let Ok(cwd) = std::env::current_dir() {
            log.write_line(&format!("cwd: {}", cwd.display()));
        }
        log.write_line(&format!("pid: {}", std::process::id()));
        Some(log)
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// 写一行（自动加 UTC 时间戳 + 脱敏 + 截断）。
    pub fn write_line(&self, text: &str) {
        let cleaned = clamp(&redact(text), MAX_LINE_CHARS);
        let mut file = lock(&self.inner.file);
        let _ = writeln!(file, "{} {cleaned}", utc_stamp());
        // 立即 flush：崩溃/panic 时日志仍然完整（不在热路径上，代价可接受）。
        let _ = file.flush();
    }

    /// 记一条用户输入。
    pub fn user(&self, text: &str) {
        self.flush_assistant();
        self.write_line(&format!(">>> {}", one_line(text)));
    }

    /// 记一条斜杠命令（不经模型，但仍要留痕）。
    pub fn slash(&self, body: &str) {
        self.write_line(&format!("[slash] /{body}"));
    }

    /// 把一个 [`AgentEvent`] 投影成日志行。
    ///
    /// 正文增量只进缓冲（流式每秒几十上百条），其余事件即时落盘。
    pub fn event(&self, ev: &AgentEvent) {
        match ev {
            AgentEvent::TextDelta(t) => self.push_assistant(t),
            AgentEvent::Reasoning(_) => {}
            AgentEvent::TurnStarted => {
                self.flush_assistant();
                self.write_line("[turn start]");
            }
            AgentEvent::ToolStarted {
                name, arguments, ..
            } => {
                self.flush_assistant();
                self.write_line(&format!(
                    "● {name}({})",
                    clamp(&one_line(arguments), MAX_SUMMARY_CHARS)
                ));
            }
            AgentEvent::ToolResult {
                name,
                content,
                is_error,
                ..
            } => {
                let mark = if *is_error { "✗" } else { "✓" };
                self.write_line(&format!("└ {mark} {name} → {}", summarize(content)));
            }
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => self.write_line(&format!("[usage] in={input_tokens} out={output_tokens}")),
            AgentEvent::Request { kind, payload, .. } => self.write_line(&format!(
                "[request:{kind}] {}",
                clamp(&one_line(&payload.to_string()), MAX_SUMMARY_CHARS)
            )),
            AgentEvent::TurnComplete { stop_reason } => {
                self.flush_assistant();
                self.write_line(&format!("[turn complete: {stop_reason:?}]"));
            }
            AgentEvent::Error { message, retryable } => {
                self.flush_assistant();
                self.write_line(&format!("[error retryable={retryable}] {message}"));
            }
            AgentEvent::Cancelled => {
                self.flush_assistant();
                self.write_line("[cancelled]");
            }
            AgentEvent::SubAgent {
                parent_tool_id,
                event,
            } => {
                self.write_line(&format!("[subagent {parent_tool_id}]"));
                self.event(event);
            }
        }
    }

    /// 把助手正文缓冲落盘（空缓冲是空操作）。
    pub fn flush_assistant(&self) {
        let text = {
            let mut buf = lock(&self.inner.assistant);
            std::mem::take(&mut *buf)
        };
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        // 多行正文缩进续行，保持「一行一个时间戳」的可 grep 形态。
        self.write_line(&format!("assistant: {}", text.replace('\n', "\n    ")));
    }

    /// tracing 文件层用的 [`MakeWriter`](tracing_subscriber::fmt::MakeWriter)。
    pub fn writer(&self) -> RunLogWriter {
        RunLogWriter { log: self.clone() }
    }

    fn push_assistant(&self, delta: &str) {
        let ready = {
            let mut buf = lock(&self.inner.assistant);
            buf.push_str(delta);
            if buf.chars().count() >= ASSISTANT_FLUSH_CHARS {
                Some(std::mem::take(&mut *buf))
            } else {
                None
            }
        };
        if let Some(text) = ready {
            self.write_line(&format!("assistant: {}", text.trim()));
        }
    }
}

/// 取锁；中毒（panic 后）也继续用，日志不能因为一次 panic 就断掉。
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `MRORCODE_LOG=0|off|false|no|disable|disabled` 关闭日志。
fn disabled_by_env() -> bool {
    std::env::var("MRORCODE_LOG")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .is_some_and(|v| {
            matches!(
                v.as_str(),
                "0" | "off" | "false" | "no" | "disable" | "disabled"
            )
        })
}

/// 日志目录：`MRORCODE_LOG_DIR` 优先，否则 `~/.mrorcode/logs`。
fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MRORCODE_LOG_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(".mrorcode").join("logs"))
}

/// 按 `run-<时间戳>.log` 生成不冲突的路径（同秒多次启动加 `-N` 后缀）。
fn unique_path(dir: &Path) -> Option<PathBuf> {
    let base = format!("{FILE_PREFIX}{}", file_stamp(now_secs()));
    let mut path = dir.join(format!("{base}.log"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.log"));
        n += 1;
        if n > 1000 {
            return None;
        }
    }
    Some(path)
}

/// 只保留文件名最大的 `keep` 份（时间戳可字典序比较 = 时间序）。
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.starts_with(FILE_PREFIX) && n.ends_with(".log"))
        .collect();
    if names.len() <= keep {
        return;
    }
    names.sort();
    let excess = names.len() - keep;
    for name in names.into_iter().take(excess) {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 当前 UTC 时间，ISO8601（`2026-10-29T16:30:45Z`）。
fn utc_stamp() -> String {
    utc_stamp_of(now_secs())
}

fn utc_stamp_of(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// 文件名时间戳（`20261029-163045`）：字典序即时间序。
fn file_stamp(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(secs);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// 纪元秒 → (年, 月, 日, 时, 分, 秒)，UTC。
///
/// 用 Howard Hinnant 的 civil_from_days 算法，避免为一次格式化引入时间库。
fn utc_parts(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, h as u32, mi as u32, s as u32)
}

/// 超长截断（按字符，不劈开 UTF-8），并标注被砍掉多少字符。
fn clamp(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}…(+{} 字符)", total - max_chars)
}

/// 压成单行（工具参数/输出里的换行会把「一行一条」打散）。
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

/// 工具输出摘要：首个非空行 + 总行数，不落整段内容。
fn summarize(content: &str) -> String {
    let lines = content.lines().count();
    let first = content
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    format!(
        "{}（{lines} 行）",
        clamp(&one_line(first.trim()), MAX_SUMMARY_CHARS)
    )
}

/// 脱敏规则集合（编译一次，复用）。
///
/// 顺序有意义：先整头打码 `Authorization`（否则其 `Bearer` 值会被当成普通文本漏出去），
/// 再处理赋值形态，最后兜底裸密钥前缀。
struct Rules {
    /// `Authorization: Bearer xxx` / `Proxy-Authorization: xxx`
    auth: Regex,
    /// `api_key = "xxx"` / `"api_key": "xxx"`（带引号的值）
    quoted: Regex,
    /// `api_key = xxx`（无引号的值）
    bare: Regex,
    /// 独立的 `Bearer xxx`
    bearer: Regex,
    /// 裸密钥前缀：`sk-xxx` / `ark-xxx` / `ghp_xxx` / `xoxb-xxx` …
    keylike: Regex,
}

/// 参与赋值形态打码的键名。
const KEY_NAMES: &str = "api[_-]?key|api[_-]?secret|secret|token|password|passwd";

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| Rules {
        auth: Regex::new(r"(?i)\b((?:proxy-)?authorization)(\s*[:=]\s*)(?:bearer\s+)?\S+")
            .expect("脱敏正则固定，不会编译失败"),
        quoted: Regex::new(&format!(
            r#"(?i)\b({KEY_NAMES})("?)(\s*[:=]\s*)("[^"]{{6,}}")"#
        ))
        .expect("脱敏正则固定，不会编译失败"),
        bare: Regex::new(&format!(
            r#"(?i)\b({KEY_NAMES})("?)(\s*[:=]\s*)([^\s",;)]{{6,}})"#
        ))
        .expect("脱敏正则固定，不会编译失败"),
        bearer: Regex::new(r"(?i)\b(bearer)(\s+)([A-Za-z0-9._~+/=-]{8,})")
            .expect("脱敏正则固定，不会编译失败"),
        keylike: Regex::new(
            r"(?i)\b(sk|ark|ghp|gho|ghs|pat|xoxb|xoxp|glpat)([-_])[A-Za-z0-9_-]{8,}",
        )
        .expect("脱敏正则固定，不会编译失败"),
    })
}

/// 把疑似凭据打码：保留键名/引号/分隔符形态便于定位，只替换值。
pub fn redact(text: &str) -> String {
    let r = rules();
    let s = r.auth.replace_all(text, "${1}${2}***");
    let s = r.quoted.replace_all(&s, "${1}${2}${3}\"***\"");
    let s = r.bare.replace_all(&s, "${1}${2}${3}***");
    let s = r.bearer.replace_all(&s, "${1}${2}***");
    r.keylike.replace_all(&s, "${1}${2}***").into_owned()
}

/// tracing 的 `MakeWriter`：把诊断行写进同一个运行日志文件。
#[derive(Clone)]
pub struct RunLogWriter {
    log: RunLog,
}

impl<'a> fmt::MakeWriter<'a> for RunLogWriter {
    type Writer = RunLogSink<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RunLogSink {
            log: &self.log,
            buf: Vec::new(),
        }
    }
}

/// 按行缓冲的写入器：tracing 一次 `write` 未必是完整行，攒到 `\n` 才落盘。
pub struct RunLogSink<'a> {
    log: &'a RunLog,
    buf: Vec<u8>,
}

impl Write for RunLogSink<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            self.log.write_line(&text);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let text = String::from_utf8_lossy(&self.buf).into_owned();
            self.buf.clear();
            self.log.write_line(&text);
        }
        Ok(())
    }
}

impl Drop for RunLogSink<'_> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// 初始化 tracing：stderr 保持原样（默认 `warn`），文件层默认 `debug` 并并入运行日志。
pub fn init_tracing(log: Option<&RunLog>) {
    let stderr_layer = fmt::layer()
        .with_writer(io::stderr)
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")));
    let Some(log) = log else {
        tracing_subscriber::registry().with(stderr_layer).init();
        return;
    };
    let level = std::env::var("MRORCODE_LOG_LEVEL").unwrap_or_else(|_| "debug".to_string());
    let file_filter = EnvFilter::try_new(&level).unwrap_or_else(|_| EnvFilter::new("debug"));
    let file_layer = fmt::layer()
        .with_ansi(false)
        .without_time()
        .with_writer(log.writer())
        .with_filter(file_filter);
    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .init();
    log.write_line(&format!("[tracing] 文件层级别 = {level}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mrorcode_kernel::event::StopReason;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mrorcode-runlog-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn redacts_assigned_secrets() {
        // TOML 形态：保留键名与引号，只打码值
        assert_eq!(
            redact(r#"api_key = "sk-abcdefghijk123""#),
            r#"api_key = "***""#
        );
        // JSON 形态
        assert_eq!(
            redact(r#"{"api_key":"ark-78e86793-5b76-4a85-a5be"}"#),
            r#"{"api_key":"***"}"#
        );
        // 无引号形态
        assert_eq!(redact("api_key=abcdef123456"), "api_key=***");
        // Authorization 头整值打码（否则其 Bearer 值会漏出去）
        assert_eq!(
            redact("Authorization: Bearer abcdefgh12345678"),
            "Authorization: ***"
        );
    }

    #[test]
    fn redacts_bare_key_prefixes() {
        assert_eq!(
            redact("token 是 sk-proj-abc123456789xyz"),
            "token 是 sk-***"
        );
        assert_eq!(
            redact("export GHP=ghp_abcdefgh12345678"),
            "export GHP=ghp_***"
        );
    }

    #[test]
    fn keeps_ordinary_text_intact() {
        // 正常文本不该被打码（`token` 后无 `:`/`=` 不算赋值）
        assert_eq!(
            redact("token 用量 12，tokenizer 正常"),
            "token 用量 12，tokenizer 正常"
        );
        assert_eq!(
            redact("读取文件(src/main.rs) → 42 行"),
            "读取文件(src/main.rs) → 42 行"
        );
    }

    #[test]
    fn clamp_is_char_safe() {
        let text = "一二三四五";
        assert_eq!(clamp(text, 10), text);
        let out = clamp(text, 2);
        assert!(out.starts_with("一二"), "按字符截断：{out}");
        assert!(out.contains("+3 字符"), "标注剩余长度：{out}");
    }

    #[test]
    fn utc_conversion_matches_known_instants() {
        assert_eq!(utc_stamp_of(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp_of(1_700_000_000), "2023-11-14T22:13:20Z");
        // 文件时间戳与 ISO 时间戳必须同源，且字典序 = 时间序
        assert_eq!(file_stamp(1_700_000_000), "20231114-221320");
        assert!(file_stamp(1_700_000_000) < file_stamp(1_700_000_001));
    }

    #[test]
    fn prune_keeps_newest_files() {
        let dir = temp_dir("prune");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..25 {
            std::fs::write(
                dir.join(format!("run-202610{:02}-000000.log", 10 + i)),
                b"x",
            )
            .unwrap();
        }
        // 无关文件不能被动
        std::fs::write(dir.join("keep-me.txt"), b"x").unwrap();

        prune(&dir, KEEP);

        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(FILE_PREFIX))
            .collect();
        names.sort();
        assert_eq!(names.len(), KEEP, "只留 {KEEP} 份");
        // 留下的是最大的 20 个（即最近的）
        assert_eq!(names.first().unwrap(), "run-20261015-000000.log");
        assert_eq!(names.last().unwrap(), "run-20261034-000000.log");
        assert!(dir.join("keep-me.txt").exists(), "非日志文件不被清理");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_redacted_transcript_and_tool_summary() {
        let dir = temp_dir("write");
        let log = RunLog::with_dir(&dir).expect("日志文件应能创建");
        assert!(log.path().starts_with(&dir));

        log.user(r#"用 api_key = "sk-abcdefghijk123" 跑一下"#);
        log.event(&AgentEvent::ToolStarted {
            id: "t1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"ls\nrm -rf /"}"#.into(),
        });
        log.event(&AgentEvent::ToolResult {
            id: "t1".into(),
            name: "bash".into(),
            content: "第一行结果\n第二行\n第三行\n".into(),
            is_error: false,
        });
        log.event(&AgentEvent::TextDelta("我来查一下".into()));
        log.event(&AgentEvent::TurnComplete {
            stop_reason: StopReason::Stopped,
        });

        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("run log"), "首行应有版本头：{text}");
        assert!(!text.contains("sk-abcdefghijk123"), "密钥必须打码：{text}");
        assert!(
            text.contains(r#"api_key = "***""#),
            "保留键名便于定位：{text}"
        );
        assert!(text.contains(">>> "), "用户输入要留痕");
        assert!(text.contains("● bash("), "工具卡要留痕");
        // 工具输出只留摘要：首行 + 行数，不落全部内容
        assert!(
            text.contains("✓ bash → 第一行结果（3 行）"),
            "工具摘要：{text}"
        );
        assert!(!text.contains("第二行"), "工具输出不应整段落盘：{text}");
        // 助手正文在终态才落盘
        assert!(text.contains("assistant: 我来查一下"), "助手正文：{text}");
        assert!(
            text.contains("[turn complete: Stopped]"),
            "终态标记：{text}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_path_survives_same_second_collision() {
        let dir = temp_dir("collide");
        std::fs::create_dir_all(&dir).unwrap();
        let first = unique_path(&dir).expect("应能生成路径");
        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(&dir).expect("同秒应退让出 -1 后缀");
        assert_ne!(first, second);
        assert!(second.to_string_lossy().contains("-1.log"), "{second:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
