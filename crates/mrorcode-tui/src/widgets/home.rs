//! Home 页：垂直居中的 logo + 输入面板（含 model 行） + cwd/hints 行；
//! MCP/版本底栏钉在屏幕最底部边缘。
//! 布局精确对齐 opencode (packages/tui/src/routes/home.tsx + component/prompt/index.tsx)。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::store::{AppState, Route};
use crate::theme::Theme;
use crate::widgets::prompt;

pub fn render(state: &mut AppState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    // 两大区：主内容区（上方留出底栏 1 行），底栏钉屏幕底部
    let [main_zone, bar] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

    // ---------- 输入面板 ----------
    // 对齐 opencode prompt/index.tsx，但去掉了底部 "╹+▀" 装饰行（用户嫌间隙过大）：
    //   outer box: border left (蓝竖线 ┃，粗)
    //   inner box: bg=raised, padding L=2/R=2/T=1/B=1
    //   - textarea
    //   - spacer (paddingTop=1 一行空白)
    //   - metadata row ("Build · model provider")
    //   - 底部 padding 一行灰底（元信息行不贴面板底边）
    let pw = 75u16.min(((area.width as f32 * 0.7) as u16).max(40));
    let textarea_lines = prompt::content_height(state, area.height);
    let pad_top: u16 = 1; // 面板内顶部 padding
    let pad_bottom: u16 = 1; // 面板内底部 padding（元信息行下方留一行灰底）
    let pad_left: u16 = 2;
    let pad_right: u16 = 2;
    let gap_after_textarea: u16 = 1; // paddingTop=1 between textarea and metadata
    let panel_h = pad_top + textarea_lines + gap_after_textarea + 1 + pad_bottom;

    // 主内容区内垂直居中：logo(3) + 间距(1) + 面板(定高) + 外边距(1) + cwd 行(1)
    let chunks = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),       // logo
        Constraint::Length(1),       // logo 与面板间距
        Constraint::Length(panel_h), // 输入面板（定高，避免上下多余空白）
        Constraint::Length(1),       // 面板与 cwd 行的外边距（margin）
        Constraint::Length(1),       // cwd / hints 行
        Constraint::Fill(1),
    ])
    .split(main_zone);

    // ---------- Logo ----------
    let logo = vec![
        Line::from(Span::styled(
            "欧瑞文先生的智能编码助手",
            Style::default()
                .fg(Color::Rgb(96, 165, 250))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "终端 AI 编码助手",
            Style::default().fg(theme.text_muted),
        )),
    ];
    Paragraph::new(logo).centered().render(chunks[1], buf);

    let [slot] = Layout::horizontal([Constraint::Length(pw)])
        .flex(Flex::Center)
        .areas(chunks[3]);
    let panel = Rect {
        x: slot.x,
        y: slot.y,
        width: slot.width,
        height: panel_h,
    };

    // 灰背景填充：从 col1 到右边缘，覆盖整个面板
    for y in panel.y..panel.y + panel_h {
        for x in panel.x + 1..panel.x + panel.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_style(Style::default().bg(theme.background_panel));
            }
        }
    }
    // 蓝竖线 col0：用粗双线 ┃（跟旧 prompt.rs 一致）
    for y in panel.y..panel.y + panel_h {
        if let Some(cell) = buf.cell_mut((panel.x, y)) {
            cell.set_symbol("\u{2503}"); // ┃
            cell.set_style(Style::default().fg(theme.secondary));
        }
    }

    // textarea inset：panel.y + pad_top
    let textarea_inset = Rect {
        x: panel.x + 1 + pad_left,
        y: panel.y + pad_top,
        width: panel.width - 1 - pad_left - pad_right,
        height: textarea_lines,
    };
    (&state.prompt).render(textarea_inset, buf);

    // 元信息行（"Build · model provider"）：textarea + gap_after_textarea + pad_top
    let meta_y = panel.y + pad_top + textarea_lines + gap_after_textarea;
    let meta_inset = Rect {
        x: panel.x + 1 + pad_left,
        y: meta_y,
        width: panel.width - 1 - pad_left - pad_right,
        height: 1,
    };
    // 配色对齐 opencode：只有权限模式徽章用主色（加粗），
    // 模型名用普通前景色、provider 用 muted——避免整行像链接一样发蓝。
    let mut model_spans = vec![
        Span::styled(
            state.mode.label(),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(theme.text_muted)),
        Span::styled(state.model.clone(), Style::default().fg(theme.text)),
    ];
    if !state.current_provider.is_empty() {
        model_spans.push(Span::styled(
            format!(" {}", state.current_provider),
            Style::default().fg(theme.text_muted),
        ));
    }
    Line::from(model_spans).render(meta_inset, buf);

    // ---------- cwd:branch 左 + 键位提示 右（与面板同宽居中） ----------
    // chunks[4] 是面板与本行之间的外边距（空白行），本行在 chunks[5]
    let [meta_slot] = Layout::horizontal([Constraint::Length(pw)])
        .flex(Flex::Center)
        .areas(chunks[5]);

    let cwd_label = match &state.git_branch {
        Some(br) => format!("{}:{}", state.working_dir.display(), br),
        None => state.working_dir.display().to_string(),
    };
    let left = Line::from(Span::styled(
        cwd_label,
        Style::default().fg(theme.text_muted),
    ));
    let right_text = "shift+tab agents · ctrl+p commands";
    let right = Line::from(Span::styled(
        right_text,
        Style::default().fg(theme.text_muted),
    ));
    let [l, r] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(right_text.chars().count() as u16),
    ])
    .areas(meta_slot);
    left.render(l, buf);
    right.render(r, buf);

    // ---------- 最底栏：MCP 左 + 版本 右（钉在屏幕底部） ----------
    let mcp_line = if state.mcp_servers.is_empty() {
        Line::from("")
    } else {
        let label = format!(
            " {} MCP {}",
            state.mcp_servers.len(),
            state.mcp_servers.join(", ")
        );
        Line::from(vec![
            Span::styled("◉", Style::default().fg(theme.primary)),
            Span::styled(label, Style::default().fg(theme.text_muted)),
        ])
    };
    let ver = env!("CARGO_PKG_VERSION").to_string();
    let ver_line = Line::from(Span::styled(
        ver.clone(),
        Style::default().fg(theme.text_muted),
    ));
    let [bl, br_right] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(ver.chars().count() as u16),
    ])
    .areas(bar);
    mcp_line.render(bl, buf);
    ver_line.render(br_right, buf);

    let _ = Route::Home;
}
