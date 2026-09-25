//! 渲染热路径基准（release/bench profile）。
//!
//! - markdown_incremental：增量 block 缓存 vs 全量重解析（固化收益，防退化）
//! - long_history_scroll：1 万条消息下单帧窗口化布局耗时（预算 < 16ms）
//! - width_cache：unicode 宽度缓存命中 vs 裸 unicode-width

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mrorcode_tui::bench_support;

fn markdown_incremental(c: &mut Criterion) {
    let width = 118;
    let base: String = (0..200)
        .map(|i| format!("## 标题 {i}\n\n段落文字 *强调* 与 `code`，再加一段足够长的普通文本用于折行测试，重复若干内容让 block 数量可观。\n"))
        .collect::<Vec<_>>()
        .join("\n");

    c.bench_function("markdown/full_reparse_each_frame", |b| {
        b.iter(|| {
            bench_support::full_reparse(black_box(&base), black_box(width));
        })
    });

    c.bench_function("markdown/incremental_tail_block", |b| {
        let mut doc = bench_support::new_doc();
        doc.update(&base);
        let mut text = base.clone();
        b.iter(|| {
            text.push('。');
            bench_support::incremental_update(&mut doc, &text, width);
        })
    });
}

fn long_history_scroll(c: &mut Criterion) {
    let mut state = bench_support::long_history_state(10_000);
    c.bench_with_input(
        BenchmarkId::new("windowed_frame", "10000_msgs"),
        &(),
        |b, _| {
            b.iter(|| {
                bench_support::render_long_history_frame(black_box(&mut state));
            })
        },
    );
}

fn width_cache(c: &mut Criterion) {
    let sample = "中文字符と日本語と한글 mixed with ascii text 日本語";

    c.bench_function("width/mixed_chars", |b| {
        b.iter(|| {
            for ch in sample.chars() {
                black_box(mrorcode_tui::render::char_width(ch));
            }
        })
    });
}

fn theme_parse_all(c: &mut Criterion) {
    c.bench_function("theme/parse_33_bundled", |b| {
        b.iter(|| {
            for (_, json) in mrorcode_tui::bench_support::bundled_themes() {
                black_box(mrorcode_tui::bench_support::parse_theme_dark(json));
            }
        })
    });
}

fn syntax_highlight(c: &mut Criterion) {
    // 200 行合成 Rust 源码（注释/字符串/数字/函数等 token 混合）
    let source: String = (0..200)
        .map(|i| {
            format!(
                "// line {i}\nfn item_{i}(n: u64) -> u64 {{\n    let s = \"value {i}\";\n    n + {i} * 3\n}}\n"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    c.bench_function("syntax/highlight_rust_200_lines", |b| {
        b.iter(|| {
            black_box(bench_support::highlight_rust(black_box(&source)));
        })
    });
}

fn fuzzy_filter(c: &mut Criterion) {
    // 条目池在计时外构造；只测过滤本身
    let pool = bench_support::fuzzy_pool(10_000);
    c.bench_function("fuzzy/filter_10000_items", |b| {
        b.iter(|| {
            black_box(bench_support::fuzzy_filter_count(
                black_box("swtm"),
                black_box(pool.iter()),
            ));
        })
    });
}

// ---------- M4-12 新增基准 ----------

fn diff_parse(c: &mut Criterion) {
    let text = bench_support::synth_diff();
    c.bench_function("diff/parse_400_hunks", |b| {
        b.iter(|| {
            black_box(bench_support::diff_parse_count(black_box(text)));
        })
    });
}

fn diff_windowed_frame(c: &mut Criterion) {
    let doc = bench_support::syn_diff_doc();
    c.bench_function("diff/windowed_frame", |b| {
        b.iter(|| {
            bench_support::render_diff_frame(black_box(&doc));
        })
    });
}

fn selection_extract(c: &mut Criterion) {
    let buf = bench_support::selection_buffer();
    c.bench_function("selection/extract_full_screen", |b| {
        b.iter(|| {
            black_box(bench_support::selection_extract_count(black_box(&buf)));
        })
    });
}

fn reasoning_fold(c: &mut Criterion) {
    c.bench_function("reasoning/collapsed_height", |b| {
        let mut m = bench_support::reasoning_view(200);
        b.iter(|| {
            black_box(bench_support::reasoning_height(
                black_box(&mut m),
                118,
                false,
            ));
        })
    });
    c.bench_function("reasoning/expanded_height", |b| {
        let mut m = bench_support::reasoning_view(200);
        b.iter(|| {
            black_box(bench_support::reasoning_height(
                black_box(&mut m),
                118,
                true,
            ));
        })
    });
}

criterion_group!(
    benches,
    markdown_incremental,
    long_history_scroll,
    width_cache,
    theme_parse_all,
    syntax_highlight,
    fuzzy_filter,
    diff_parse,
    diff_windowed_frame,
    selection_extract,
    reasoning_fold
);
criterion_main!(benches);
