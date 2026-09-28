//! mrorcode 可复用库面：目前只暴露 headless 渲染层。
//!
//! 二进制在 `src/main.rs`；把 `headless` 放进 lib 是为了能写集成测试
//! （`tests/headless_json.rs`）——bin 里的私有模块无法被 `tests/` 引用。

pub mod headless;
