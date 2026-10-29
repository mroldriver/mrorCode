//! mrorcode 可复用库面：暴露 headless 渲染层与裸 ANSI REPL。
//!
//! 二进制在 `src/main.rs`；把 `headless`/`repl` 放进 lib 是为了能写集成测试
//! （`tests/headless_json.rs`）——bin 里的私有模块无法被 `tests/` 引用。

pub mod headless;
pub mod repl;
pub mod runlog;
