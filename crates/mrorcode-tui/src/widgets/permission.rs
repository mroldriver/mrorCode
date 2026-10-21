//! 审批弹窗（对齐 opencode 三阶段交互：permission → always 确认 / reject 输入原因）。

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::store::{AppState, PermissionStage};
use crate::theme::Theme;

/// 按工具类型生成展示标题（对齐 opencode permissionPresentation）。
fn present(tool: &str, args: &serde_json::Value) -> (String, String) {
    // 返回 (icon, title)
    match tool {
        "edit_file" | "write_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .or_else(|| args.get("file_path").and_then(|v| v.as_str()))
                .unwrap_or("?");
            if tool == "edit_file" {
                ("→".into(), format!("编辑 {}", path))
            } else {
                ("→".into(), format!("写入 {}", path))
            }
        }
        "bash" | "shell" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("?");
            (
                "#".into(),
                format!("Shell: {}", crate::render::truncate_end(cmd, 60)),
            )
        }
        "read" | "glob" | "grep" => {
            let pattern = args
                .get("path")
                .and_then(|v| v.as_str())
                .or_else(|| args.get("pattern").and_then(|v| v.as_str()))
                .unwrap_or("?");
            ("◈".into(), format!("{} {}", tool, pattern))
        }
        "task" | "subagent" => {
            let agent = args
                .get("agent")
                .and_then(|v| v.as_str())
                .unwrap_or("general");
            ("◉".into(), format!("子代理 {}", agent))
        }
        _ => ("⚙".into(), format!("调用工具 {}", tool)),
    }
}

pub fn render(state: &AppState, area: Rect, buf: &mut Buffer, theme: &Theme) {
    let Some(req) = &state.permission else {
        return;
    };
    Clear.render(area, buf);

    // 卡片尺寸：宽度 60 封顶，高度根据阶段自适应。
    let stage = req.stage;
    let width = 64u16.min(area.width.saturating_sub(4)).max(24);
    // 基础行：标题(1) + 空行(1) + 工具行(1) + 详情(1) + 空行(1) + 按钮行(1) + 提示行(1) = 7
    // reject 阶段多一行输入框：+2
    let height = if stage == PermissionStage::Reject {
        11
    } else {
        9
    };
    let height = height.min(area.height.saturating_sub(2)).max(7);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 3;
    let card = Rect {
        x,
        y,
        width,
        height,
    };

    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning))
        .style(Style::default().bg(theme.background_menu))
        .title_alignment(Alignment::Left)
        .render(card, buf);

    let inner = Rect {
        x: card.x + 1,
        y: card.y + 1,
        width: card.width - 2,
        height: card.height - 2,
    };

    // ---- 标题行（△ + 阶段标题）----
    let title = match stage {
        PermissionStage::Permission => "需要授权",
        PermissionStage::AlwaysConfirm => "始终允许？",
        PermissionStage::Reject => "拒绝授权",
    };

    let mut lines: Vec<ratatui::text::Line> = vec![
        ratatui::text::Line::from(vec![
            ratatui::text::Span::styled("△ ", Style::default().fg(theme.warning)),
            ratatui::text::Span::styled(
                title,
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        ratatui::text::Line::raw(""),
    ];

    // ---- 工具展示区 ----
    if stage != PermissionStage::AlwaysConfirm {
        let args: serde_json::Value = serde_json::from_str(&req.arguments).unwrap_or_default();
        let (icon, display_title) = present(&req.tool, &args);

        lines.push(ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(format!("{icon} "), Style::default().fg(theme.text_muted)),
            ratatui::text::Span::styled(display_title, Style::default().fg(theme.text)),
        ]));

        // reject 阶段：输入原因提示 + 当前输入
        if stage == PermissionStage::Reject {
            lines.push(ratatui::text::Line::raw(""));
            lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
                "告诉它该如何调整（可选，留空直接拒绝）：",
                Style::default().fg(theme.text_muted),
            )));
            let display_msg = if req.reject_message.is_empty() {
                "（输入原因…）".to_string()
            } else {
                req.reject_message.clone()
            };
            let msg_style = if req.reject_message.is_empty() {
                Style::default().fg(theme.text_muted)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
                crate::render::truncate_end(&display_msg, (width - 4) as usize),
                msg_style,
            )));
        }

        // always 阶段：提示始终允许的影响
    } else {
        lines.push(ratatui::text::Line::raw(""));
        lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
            "此操作将对当前项目永久放行，不再询问。",
            Style::default().fg(theme.text_muted),
        )));
    }

    lines.push(ratatui::text::Line::raw(""));

    // ---- 横向按钮行 ----
    let opts = state.permission_options();
    let cursor = state.permission_cursor.min(opts.len().saturating_sub(1));

    let mut button_spans: Vec<ratatui::text::Span> = Vec::new();
    for (i, opt) in opts.iter().enumerate() {
        let selected = i == cursor;
        let (fg, bg, modifier) = if selected {
            (theme.text, theme.primary, Modifier::BOLD)
        } else {
            (theme.text_muted, theme.background_menu, Modifier::empty())
        };
        button_spans.push(ratatui::text::Span::styled(
            format!(" {opt} "),
            Style::default().fg(fg).bg(bg).add_modifier(modifier),
        ));
        if i + 1 < opts.len() {
            button_spans.push(ratatui::text::Span::raw(" "));
        }
    }
    lines.push(ratatui::text::Line::from(button_spans));

    // ---- 底部提示行 ----
    let hint = match stage {
        PermissionStage::Permission => "←→/hj 选择 · Enter 确认 · Esc 拒绝",
        PermissionStage::AlwaysConfirm => "←→/hj 选择 · Enter 确认 · Esc 返回",
        PermissionStage::Reject => "输入原因 · Enter 拒绝 · Esc 返回",
    };
    lines.push(ratatui::text::Line::raw(""));
    lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
        hint,
        Style::default().fg(theme.text_muted),
    )));

    Paragraph::new(lines).render(inner, buf);
}
