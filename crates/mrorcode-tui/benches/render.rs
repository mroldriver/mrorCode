//! 渲染热路径基准（M1 步骤 8/9/11 逐步充实）。
use criterion::{criterion_group, criterion_main, Criterion};

fn placeholder(c: &mut Criterion) {
    c.bench_function("placeholder", |b| b.iter(|| 1 + 1));
}

criterion_group!(benches, placeholder);
criterion_main!(benches);
