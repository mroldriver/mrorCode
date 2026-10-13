//! grep/glob 大仓基准（bench profile）。
//!
//! 目标：合成 ~1 万文件的仓库，验证文件树工具的单次调用开销，
//! 守住"大仓内一轮搜索/列目录不应拖垮对话"的预算。

use std::path::PathBuf;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mrorcode_capabilities::tools::{glob::GlobTool, grep::GrepTool};
use mrorcode_kernel::{request::RequestBroker, tool::ToolContext, Tool};
use tokio_util::sync::CancellationToken;

/// 合成一个 N 目录 × M 文件的仓库，返回根目录。
fn synth_repo(dirs: usize, per_dir: usize) -> PathBuf {
    let root = std::env::temp_dir().join(format!("mrorcode-search-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for d in 0..dirs {
        let dir = root.join(format!("mod{d:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..per_dir {
            std::fs::write(
                dir.join(format!("item{f:04}.rs")),
                format!("use mrorcode_agent::runtime; fn hello{f}() -> usize {{ {d} + {f} }}\n"),
            )
            .unwrap();
        }
    }
    root
}

fn requester() -> mrorcode_kernel::Requester {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    RequestBroker::new(tx).handle()
}

fn repo_search(c: &mut Criterion) {
    // ~1 万文件（100 目录 × 100 文件）
    let root = synth_repo(100, 100);
    let cancel = CancellationToken::new();
    let req = requester();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let ctx = ToolContext {
        working_dir: &root,
        cancel: &cancel,
        request: &req,
        tool_id: "",
        event_sink: None,
    };

    c.bench_function("search/glob_10000_rs", |b| {
        b.iter(|| {
            let r = rt.block_on(GlobTool.execute(black_box(r#"{"pattern":"**/*.rs"}"#), &ctx));
            assert!(!r.is_error);
        });
    });

    c.bench_function("search/grep_10000_rs_specific", |b| {
        b.iter(|| {
            let r = rt.block_on(GrepTool.execute(black_box(r#"{"pattern":"hello42"}"#), &ctx));
            assert!(!r.is_error);
        });
    });

    c.bench_function("search/grep_10000_rs_absent", |b| {
        b.iter(|| {
            let r = rt
                .block_on(GrepTool.execute(black_box(r#"{"pattern":"zzz_not_found_zzz"}"#), &ctx));
            assert!(!r.is_error);
        });
    });

    let _ = std::fs::remove_dir_all(&root);
}

criterion_group!(benches, repo_search);
criterion_main!(benches);
