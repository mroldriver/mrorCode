//! 基准测试支持（criterion 外部 bench target 只能访问公开 API）。
//!
//! 生产代码不调用本模块的任何内容。

use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::store::{AppState, MessageView, Role};
use crate::theme::Theme;
use crate::widgets::markdown::MarkdownView;
use crate::widgets::session;

/// 构造含 N 条 assistant 消息的状态（每条 10 行普通文本）。
pub fn long_history_state(n: usize) -> AppState {
    let path = std::env::temp_dir().join(format!(
        "mrorcode-bench-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let (store, _) = crate::storage::Store::load_from(&path);
    let mut state = AppState::new("bench-model".into(), Vec::new(), PathBuf::from("."), store);
    for i in 0..n {
        let text = (0..10)
            .map(|j| format!("message {i} line {j} with some markdown *emphasis* text"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut msg = MessageView::assistant(i as u64 + 1);
        msg.role = Role::Assistant;
        msg.doc.update(&text);
        msg.text = text;
        state.messages.push(msg);
    }
    state
}

/// 在 120x40 的 TestBackend 上渲染一帧（窗口化路径）。
pub fn render_long_history_frame(state: &mut AppState) {
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| {
        let area = f.area();
        let theme = Theme::tokyonight();
        session::render(state, area, f.buffer_mut(), &theme);
    })
    .unwrap();
}

/// 对照基准：对整段文本从零解析+折行（模拟 opencode 式每帧全量重排）。
pub fn full_reparse(text: &str, width: u16) -> usize {
    let mut doc = MarkdownView::new();
    doc.update(text);
    doc.line_count(width, 0, &Theme::tokyonight())
}

/// 增量路径：只 push 尾字符再更新（block 前缀缓存命中）。
pub fn incremental_update(doc: &mut MarkdownView, text: &str, width: u16) -> usize {
    doc.update(text);
    doc.line_count(width, 0, &Theme::tokyonight())
}

pub fn new_doc() -> MarkdownView {
    MarkdownView::new()
}

/// 暴露内置主题包供基准使用（名称, JSON）。
pub fn bundled_themes() -> &'static [(&'static str, &'static str)] {
    crate::theme::BUNDLED
}

/// 按暗色模式解析单个主题 JSON。
pub fn parse_theme_dark(json: &str) -> Option<crate::theme::Theme> {
    crate::theme::parse(json, crate::theme::Mode::Dark)
}

/// 对一段 Rust 源码从零做 syntect 高亮（不含 markdown block 缓存）。
/// 返回生成的行数。
pub fn highlight_rust(source: &str) -> usize {
    crate::widgets::highlight::highlight_lines("rust", source, &Theme::tokyonight())
        .map(|l| l.len())
        .unwrap_or(0)
}

/// 构造 N 条合成名称供 fuzzy 基准（计时外准备）。
pub fn fuzzy_pool(n: usize) -> Vec<String> {
    let pool = [
        "new_session",
        "switch_theme",
        "toggle_sidebar",
        "list_sessions",
        "delete_session",
        "switch_model",
        "open_help",
        "quit_now",
        "read_file",
        "run_bash_command",
    ];
    (0..n)
        .map(|i| format!("{}/{i}.txt", pool[i % pool.len()]))
        .collect()
}

/// 在已有条目上跑一次 fuzzy 过滤，返回命中数。
pub fn fuzzy_filter_count<I, S>(query: &str, items: I) -> usize
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    crate::fuzzy::filter(query, items).len()
}

// ---------- M4-12 新增基准 ----------

/// 一段合成 unified diff（多文件、多 hunk、上下文/增删行混合）。
pub fn synth_diff() -> &'static str {
    let mut out =
        String::from("diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n");
    for i in 0..400 {
        out.push_str(&format!("@@ -{i},10 +{i},11 @@ fn fn_{i}()\n"));
        for _j in 0..10 {
            out.push_str(" context line of ordinary text with `code` and *emph*\n");
        }
        out.push_str("+added line\n-removed line\n");
    }
    Box::leak(out.into_boxed_str())
}

/// diff 解析耗时（DiffDoc 构造）。
pub fn diff_parse_count(text: &str) -> usize {
    crate::widgets::diff::parse(text).files.len()
}

/// 窗口化 diff 帧渲染（120x40 TestBackend，50 个文件的拆窗视图）。
pub fn render_diff_frame(doc: &crate::widgets::diff::DiffDoc) {
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let state = crate::widgets::dialog::DiffState::new("bench", doc.clone());
    term.draw(|f| {
        crate::widgets::dialog::diff::render(
            &state,
            f.area(),
            f.buffer_mut(),
            &Theme::tokyonight(),
        );
    })
    .unwrap();
}

/// 构造一个大 diff 的解析结果供渲染基准使用。
pub fn syn_diff_doc() -> crate::widgets::diff::DiffDoc {
    crate::widgets::diff::parse(synth_diff())
}

/// 选区提取基准：从 120x40 全屏 buffer 提取行文本（M4-6 选中屏守恒）。
pub fn selection_extract_count(buf: &ratatui::buffer::Buffer) -> usize {
    crate::selection::rows_from_buffer(buf).len()
}

/// 构造一个全屏 TestBackend buffer 供选区提取基准。
pub fn selection_buffer() -> ratatui::buffer::Buffer {
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| {
        let area = f.area();
        let theme = Theme::tokyonight();
        session::render(&mut long_history_state(50), area, f.buffer_mut(), &theme);
    })
    .unwrap();
    term.backend().buffer().clone()
}

/// reasoning 折叠/展开高度计算（M4-1）：对含 reasoning 的 MessageView 求 height。
pub fn reasoning_height(m: &mut MessageView, width: u16, expanded: bool) -> u16 {
    crate::widgets::reasoning::height(m, width, expanded, 0, &Theme::tokyonight())
}

/// 构造带较长 reasoning 文本的 MessageView。
pub fn reasoning_view(n_lines: usize) -> MessageView {
    let mut text: String = "思考：".into();
    for i in 0..n_lines {
        text.push_str(&format!(
            "步骤 {i}：某种分析与推理过程，包含一些长文本以便折行。\n"
        ));
    }
    let mut m = MessageView::assistant(7);
    m.reasoning = text;
    m.reasoning_doc.update(&m.reasoning);
    m
}
