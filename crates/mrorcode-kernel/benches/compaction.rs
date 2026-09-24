//! compaction 基准（bench profile）。
//!
//! - estimate_tokens / compact 在 ~1000 条长对话上的开销，
//!   守住「压缩不得拖慢 turn 开始」的预算。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mrorcode_kernel::{Conversation, Message};

/// 合成 N 个 user turn、每个 turn 夹一条 assistant 工具调用 + 结果的长对话。
fn long_conversation(turns: usize) -> Conversation {
    let mut c = Conversation::new();
    c.push(Message::system("你是一个编码助手。"));
    for i in 0..turns {
        c.push(Message::user(format!(
            "第 {i} 个问题：解释这段代码的意图与影响。"
        )));
        c.push(Message::assistant_tool_calls(vec![
            mrorcode_kernel::ToolCall {
                id: format!("call_{i}"),
                name: "read_file".into(),
                arguments: format!(r#"{{"path":"src/module{i}.rs"}}"#),
            },
        ]));
        c.push(Message::tool_result(&mrorcode_kernel::ToolResult::ok(
            format!("call_{i}"),
            format!("// 文件内容片段 {i}\nfn module_{i}() {{ /* body */ }}\n"),
        )));
        c.push(Message::assistant_text(format!(
            "回答 {i}：这里做了 {i} 次处理，注意其副作用与边界。"
        )));
    }
    c
}

fn compaction_bench(c: &mut Criterion) {
    // ~500 turn ≈ 2000 条消息（user/assistant/tool 各一层）
    let conv = long_conversation(500);
    let tokens_before = conv.estimate_tokens();
    assert!(tokens_before > 10_000, "tokens={tokens_before}");

    c.bench_function("compaction/estimate_tokens_500_turns", |b| {
        b.iter(|| black_box(conv.estimate_tokens()))
    });

    c.bench_function("compaction/compact_keep_6_of_500_turns", |b| {
        b.iter(|| {
            // 每轮从同一 baseline 重建，保证幂等（compact 是可变的）
            let mut fresh = long_conversation(500);
            let folded = fresh.compact(6);
            assert!(folded > 0);
            black_box(folded)
        })
    });

    c.bench_function("compaction/compact_small_noop_5_turns", |b| {
        b.iter(|| {
            let mut small = long_conversation(5);
            let folded = small.compact(6);
            assert_eq!(folded, 0);
            black_box(folded)
        })
    });
}

criterion_group!(benches, compaction_bench);
criterion_main!(benches);
