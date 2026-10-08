# 待办：M5 硬化发布（已全部完成）

> 建档日期：2026-09-28
> 最后更新：2026-10-08——M5-a~f 全部子项均已落地，本文档更新为完成记录存档。
> 来源：依据 `docs/roadmap.md` M5 计划与代码现状逐项核对。
> 每项完成后回到 `docs/roadmap.md` 勾选 M5 验收清单并更新本文件状态。

## M5-a 跨平台 CI（gitcode）— 已完成（2026-09-26）

实施记录见 `docs/roadmap.md`「M5-a 实施记录」。

- [x] 查阅 gitcode pipeline 文档确认配置语法——结论：gitcode 流水线为 YAML 文件
      （`.gitcode/workflows/*.yml`），语法与 GitHub Actions 几乎一致；官方托管 Runner 仅 Linux
- [x] 创建 CI 流水线（`.gitcode/workflows/ci.yml`），三个 job：
      - `linux`（ubuntu x64）：fmt + clippy + test + release 构建
      - `linux-arm64`（ubuntu arm64）：test
      - `msrv`（ubuntu x64）：钉 `RUSTUP_TOOLCHAIN=1.88.0`，test + clippy
- [x] Linux CI 用 `RUSTUP_TOOLCHAIN` 环境变量覆盖 `rust-toolchain.toml` 的 windows-gnu host
- [x] 决策：gitcode 无 Win/macOS 托管 runner，不引入 GitHub Actions、不自建 runner；
      Win/macOS 验证维持本地手工（`cargo test/clippy/fmt` + release 构建）
- [ ] 待人工：push 到 main 后在 gitcode「流水线」Tab 确认 workflow 被识别并跑通
      （当前工作分支 develop，workflow 触发分支为 main）

## M5-b Anthropic provider — 已完成（2026-09-25）

- [x] `type = "anthropic"` 可用，config `resolve()` 白名单放行
- [x] `capabilities/provider/anthropic.rs`：Messages API、手写 SSE 解码、tool_use/tool_result 映射
- [x] `assemble.rs` 按 provider_type 选 provider
- [x] wiremock 单测覆盖（首段文本、tool_use、tool_result 回灌、收尾）
- [x] 241 测试全绿

## M5-c 打包分发 — 已完成（2026-09-26，v0.5.6）

- [x] `panic = "abort"` 写入 release profile（体积 7.9→7.0 MB，windows-gnu 兼容，
      v0.5.4 release 实测 6.99 MB）
- [x] mimalloc 评估后否决（本机无完整 gcc，与 ring/rustls 同约束）
- [x] 新增 `.gitcode/workflows/release.yml`，tag `v*` 或手动触发，三个产物 job：
      - linux-x64、linux-arm64（原生 release，打 .tar.gz）
      - windows-x64（mingw-w64 交叉编译 `x86_64-pc-windows-gnu`，打 .zip）
      各 `upload-artifact` 保留 30 天
- [x] 取舍：cargo-dist 否决（优势依赖多 OS runner，Linux-only 下手写更简单）；
      macOS 无法在 Linux 合法交叉（缺 SDK），维持本地 `cargo build --release` 手工出包
- [ ] 待人工：push tag（如 `v0.5.13`）后在 gitcode「流水线」确认三个 job 跑通，
      并回填 Linux/交叉产物的实测体积基线

## M5-d headless JSONL 稳定化 + conformance testkit — 已完成（2026-09-28）

实施记录见 `docs/roadmap.md`「M5-d 实施记录」。

- [x] 冻结 `cli/headless.rs` 的 `--json` 事件 schema（`SCHEMA_VERSION = 1`，11 种事件，
      每行带 `v` + `type`，`run.finished` 恒末行）
- [x] 黄金文件 + 字段级快照防漂移（不引入 insta，`UPDATE_GOLDEN=1` 显式刷新）
- [x] 终态语义修正：仅 `turn.completed{stop_reason:"stopped"}` 成功；`error` 降为诊断行
- [x] 扩展 kernel `test-support`：`stream_signature` / `assert_stream_sequence` /
      `event_signature` / `assert_event_sequence`
- [x] OpenAI 兼容与 Anthropic 两个 provider 跑同一套 conformance 断言
- [x] 验收：282 测试全绿
- 顺带修：审批 middleware 未接线的安全问题；OpenAI 流中途 `{"error":{...}}` 被静默丢弃

## M5-e 跨 provider 切换 / daemon — 已完成（2026-09-26，v0.5.5，轻量方案）

设计与实施记录见 `docs/roadmap.md`「M5-e 设计」。

- [x] 轻量方案落地：关闭当前 runtime → 新 provider 重新 `assemble` → resume 当前会话
- [x] config 新增 `resolve_named(name)`（按条目解析，不受 default_provider/default_model 影响）
- [x] agent 新增 `rebuild_assembly()`（只构建新 Assembly，不关旧的）
- [x] TUI 新增命令「switch provider」（`leader s`）+ provider 选择器
- [x] app.rs 事件循环的 handle/events 改为可替换
- [x] 切换流程：先建成功新 runtime → 再关停旧的 → resume 当前会话 → 更新模型投影；plan 模式重新下发
- [x] 失败降级：构建失败时旧 runtime 原样保留；busy 时拒绝切换
- [x] 审批 grant 走共享内存 store，新 runtime 自动继承已授权命令
- [x] 新增测试：config `resolve_named` 2 个、TUI provider picker/拒绝切换/成功替换/失败保留 4 个
- daemon 方案不做（仅在有明确多客户端需求时另立里程碑）

## M5-f TUI 与 opencode 对标 — 已完成（2026-09-28）

差异清单见 `docs/todos/20260928-tui-parity-opencode-对标差异.md`。

- [x] F2 / Shift+F2：循环最近使用模型（MRU 持久化）
- [x] Tab / Shift+Tab：循环 agent（build/plan）
- [x] plan 模式进入：Tab 手动 + 工具事件自动
- [x] toast：右上角单条、5s、自动折行
- [x] which-key：开关 + dock/overlay 切换 + 面板形态对齐
- [x] 工具输出展开：点击完整展开
- [x] 侧栏键位：leader b
- [x] 258 测试全绿

---

## 附：待人工冒烟（无法自动化，M4 验收遗留）

- [ ] 真实终端（Windows Terminal）下：鼠标滚轮/点击/拖选、OSC 52 剪贴板复制、
      标签栏切换/关闭、rewind 时间线，各验一次
- [ ] 真实 stdio MCP server（如 filesystem）：拉起 → tools/list → 审批 → 调用链路
