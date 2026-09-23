//! 精简 markdown 渲染器（M1）。
//!
//! 性能设计：原文按 block（段落/代码围栏）切分；每个 block 的解析+折行结果
//! 以 `(block 源, 宽度)` 为键缓存。流式追加通常只改最后一个 block，
//! 已闭合 block 零重排。M2 在此之上加 syntect 高亮（结果同样走缓存）。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::render::str_width;
use crate::theme::Theme;

enum Block {
    Code { lang: String, body: String },
    Markup(String),
}

/// 一条消息的可缓存 markdown 视图。
pub struct MarkdownView {
    raw: String,
    blocks: Vec<Block>,
    cache: Vec<Option<(u16, Vec<Line<'static>>)>>,
}

impl Default for MarkdownView {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownView {
    pub fn new() -> Self {
        Self {
            raw: String::new(),
            blocks: Vec::new(),
            cache: Vec::new(),
        }
    }

    /// 零分配快路径更新：文本未变时仅一次字符串比较，无 clone。
    /// 返回是否变化。
    pub fn update(&mut self, raw: &str) -> bool {
        if raw == self.raw {
            return false;
        }
        self.raw.clear();
        self.raw.push_str(raw);
        let new_blocks = split_blocks(&self.raw);
        // 复用前缀未变 block 的缓存
        let mut new_cache = Vec::with_capacity(new_blocks.len());
        for (i, block) in new_blocks.iter().enumerate() {
            let same = self
                .blocks
                .get(i)
                .is_some_and(|old| block_key(old) == block_key(block));
            if same {
                new_cache.push(self.cache.get_mut(i).and_then(Option::take));
            } else {
                new_cache.push(None);
            }
        }
        self.blocks = new_blocks;
        self.cache = new_cache;
        true
    }

    /// 渲染为指定宽度的终端行（宽度不变时直接走缓存）。
    pub fn lines(&mut self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        for i in 0..self.blocks.len() {
            let needs = match &self.cache[i] {
                Some((w, _)) => *w != width,
                None => true,
            };
            if needs {
                let lines = match &self.blocks[i] {
                    Block::Code { body, .. } => layout_code(body, width, theme),
                    Block::Markup(src) => layout_markup(src, width, theme),
                };
                self.cache[i] = Some((width, lines));
            }
            if let Some((_, lines)) = &self.cache[i] {
                out.extend(lines.iter().cloned());
            }
            out.push(Line::raw(""));
        }
        if out.last().is_some_and(|l| l.spans.is_empty()) {
            out.pop();
        }
        out
    }

    /// 行数统计的零分配快路径：缓存命中时只数行，不克隆。
    pub fn line_count(&mut self, width: u16, theme: &Theme) -> usize {
        let mut total = 0;
        let mut miss = false;
        for cached in &self.cache {
            match cached {
                Some((w, lines)) if *w == width => total += lines.len() + 1,
                _ => {
                    miss = true;
                    break;
                }
            }
        }
        if miss {
            // 冷路径：补齐布局后再数
            self.lines(width, theme).len()
        } else {
            total.saturating_sub(1) // 末尾 block 间不留空行
        }
    }
}

fn block_key(b: &Block) -> String {
    match b {
        Block::Code { lang, body } => format!("code:{lang}:{body}"),
        Block::Markup(s) => format!("markup:{s}"),
    }
}

/// 按空行切段，围栏代码块整体保留。
fn split_blocks(src: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut markup = String::new();
    let mut in_fence = false;
    let mut fence_lang = String::new();
    let mut code = String::new();

    let flush = |m: &mut String, out: &mut Vec<Block>| {
        if !m.trim().is_empty() {
            out.push(Block::Markup(std::mem::take(m)));
        }
    };

    for line in src.lines() {
        let fence_marker = line.trim_start().starts_with("```");
        if fence_marker {
            if !in_fence {
                flush(&mut markup, &mut blocks);
                in_fence = true;
                fence_lang = line.trim().trim_start_matches('`').trim().to_string();
                code.clear();
            } else {
                in_fence = false;
                blocks.push(Block::Code {
                    lang: std::mem::take(&mut fence_lang),
                    body: std::mem::take(&mut code),
                });
            }
            continue;
        }
        if in_fence {
            code.push_str(line);
            code.push('\n');
        } else if line.trim().is_empty() {
            flush(&mut markup, &mut blocks);
        } else {
            markup.push_str(line);
            markup.push('\n');
        }
    }
    if in_fence {
        // 未闭合围栏：到流结束为止都当代码
        blocks.push(Block::Code {
            lang: fence_lang,
            body: code,
        });
    } else {
        flush(&mut markup, &mut blocks);
    }
    blocks
}

fn layout_code(body: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let bg = Style::default().fg(theme.md_code).bg(theme.md_code_bg);
    body.trim_end_matches('\n')
        .lines()
        .map(|l| {
            let clipped = clip_line(l, width as usize);
            Line::from(vec![Span::styled(clipped, bg)])
        })
        .collect()
}

fn layout_markup(src: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(src, options);

    let mut styles: Vec<Style> = vec![Style::default().fg(theme.text)];
    let mut spans: Vec<(String, Style)> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                let s = *styles.last().unwrap();
                let next = match tag {
                    Tag::Heading { .. } => s.fg(theme.md_heading).add_modifier(Modifier::BOLD),
                    Tag::Link { .. } => s.fg(theme.md_link),
                    Tag::CodeBlock(_) => s.fg(theme.md_code).bg(theme.md_code_bg),
                    Tag::BlockQuote(_) => s.fg(theme.md_blockquote),
                    Tag::Strong => s.add_modifier(Modifier::BOLD),
                    Tag::Emphasis => s.add_modifier(Modifier::ITALIC),
                    Tag::List(_) | Tag::Item => s,
                    _ => s,
                };
                styles.push(next);
            }
            Event::End(tag) => {
                let post = matches!(
                    tag,
                    TagEnd::Heading(_)
                        | TagEnd::Link
                        | TagEnd::CodeBlock
                        | TagEnd::BlockQuote(_)
                        | TagEnd::Strong
                        | TagEnd::Emphasis
                );
                if post && styles.len() > 1 {
                    styles.pop();
                }
            }
            Event::Text(t) => spans.push((t.to_string(), *styles.last().unwrap())),
            Event::Code(t) => spans.push((t.to_string(), styles.last().unwrap().fg(theme.md_code))),
            Event::SoftBreak | Event::HardBreak => {
                spans.push(("\n".to_string(), *styles.last().unwrap()))
            }
            _ => {}
        }
    }

    wrap_spans(&spans, width)
}

/// 词级折行：把带样式的文本段排版到固定宽度。
fn wrap_spans(spans: &[(String, Style)], width: u16) -> Vec<Line<'static>> {
    let width = width.max(4) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;

    let push_line = |current: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>| {
        lines.push(Line::from(std::mem::take(current)));
    };

    for (text, style) in spans {
        let mut piece = text.as_str();
        while !piece.is_empty() {
            let newline = piece.find('\n');
            let segment = match newline {
                Some(i) => &piece[..i],
                None => piece,
            };
            for word in segment.split_inclusive(char::is_whitespace) {
                if word.is_empty() {
                    continue;
                }
                let w = str_width(word);
                if col + w > width && !current.is_empty() {
                    push_line(&mut current, &mut lines);
                    col = 0;
                }
                let word = word.trim_start_matches('\n');
                let mut w = str_width(word);
                let mut emitted = word;
                if w > width {
                    emitted = crate::render::take_by_width(word, width);
                    w = str_width(emitted);
                }
                if w > 0 {
                    current.push(Span::styled(emitted.to_string(), *style));
                    col += w;
                }
            }
            match newline {
                Some(i) => {
                    push_line(&mut current, &mut lines);
                    col = 0;
                    piece = &piece[i + 1..];
                }
                None => break,
            }
        }
    }
    if !current.is_empty() {
        push_line(&mut current, &mut lines);
    }
    if lines.is_empty() {
        lines.push(Line::raw(""));
    }
    lines
}

fn clip_line(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            crate::render::take_by_width(s, max.saturating_sub(1))
        )
    }
}

/// 仅供测试可见的颜色常量引用，避免 theme 死代码告警（M2 全面启用）。
#[allow(dead_code)]
fn _color_use(c: Color) -> Color {
    c
}
