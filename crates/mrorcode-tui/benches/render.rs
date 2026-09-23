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

criterion_group!(
    benches,
    markdown_incremental,
    long_history_scroll,
    width_cache
);
criterion_main!(benches);
