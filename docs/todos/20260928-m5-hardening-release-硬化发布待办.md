# 待办：M5 硬化发布（未完成项）

> 建档日期：2026-09-28
> 来源：依据 `docs/roadmap.md` M5 计划与代码现状逐项核对（git 提交、源码、配置文件）。
> M5-b（Anthropic provider）已完成，不在本文件内。
> 每项完成后回到 `docs/roadmap.md` 勾选 M5 验收清单并更新本文件状态。

## M5-a 跨平台 CI（gitcode）— 未开工

gitcode CI 为 Web UI 配置（非仓库 YAML），需人工在面板创建。

- [ ] 查阅 gitcode pipeline 文档确认配置语法：
  - https://docs.gitcode.com/docs/help/home/org_project/pipeline/
  - https://docs.gitcode.com/docs/help/home/org_project/pipeline/examples/nodejs-ci/
- [ ] 创建三平台矩阵 job：`windows-latest` / `macos-latest` / `ubuntu-latest`（稳定工具链）
- [ ] 创建 MSRV job：钉 `1.88`，仅 ubuntu 上跑
- [ ] 每 job 步骤：checkout → 装 Rust（读 `rust-toolchain.toml`）→
      `cargo fmt --check` → `cargo clippy --workspace --all-targets -- -D warnings` →
      `cargo test --workspace`
- [ ] Windows job 额外跑 `pwsh -File scripts/bootstrap-tools.ps1`（binutils/dlltool，全自动无 VS）
- [ ] 独立 release job 跑 `cargo bench -p mrorcode-tui --bench render`：
      首次只存 criterion 基线，后续关键指标回退 > 10% 报错
- [ ] 验收：push/PR 三平台 + MSRV 全绿；mac/linux 若报 native-tls 缺库再补系统依赖

## M5-c 打包分发 — 部分完成

已完成：`panic = "abort"` 已写入 release profile（体积 7.9→7.0 MB，windows-gnu 兼容）；
mimalloc 评估后否决（本机无完整 gcc，与 ring/rustls 同约束）。

- [ ] 待 M5-a CI 就绪后引入 cargo-dist（或 gitcode 产物上传步骤）
- [ ] 三平台 job 末尾构建并上传 `mrorcode[-<triple>].{exe,targz}`
- [ ] 打 tag 时出 gitcode Release
- [ ] 记录三平台 release 二进制体积基线（当前 windows-gnu 基线 ~7.0 MB）
- [ ] 验收：三平台产物可直接执行 `mrorcode --version`，体积无异常膨胀

## M5-d headless JSONL 稳定化 + conformance testkit — 未开工

现状：全仓无 insta / 快照测试 / conformance 相关代码。

- [ ] 冻结 `cli/headless.rs` 的 `--json` 事件 schema
      （`run.started/message.delta/tool.started/tool.completed/turn.completed/run.finished`）
- [ ] 加 insta schema 快照测试防漂移
- [ ] 扩展 kernel `test-support`：在 `ScriptedProvider` 上加「期望事件序列」断言器
- [ ] OpenAI 兼容与 Anthropic 两个 provider 跑同一套 conformance 断言
- [ ] 验收：改事件字段快照测试失败；conformance 套件双 provider 均绿

## M5-e 跨 provider 切换 / daemon — 未开工

现状：无任何 switch/rebuild provider 实现；切换仍需改配置重启。

- [ ] 先出设计文档再动工，两条候选：
  1. 轻量方案（默认倾向）：关闭当前 runtime → 新 provider 重新 `assemble` → resume 当前会话
  2. daemon 方案：常驻进程持 provider 连接池，TUI/headless 作客户端
- [ ] 复用 M3 snapshot/resume 路径做会话状态迁移，不另造状态机
- [ ] 验收：切换后当前会话可继续、上下文不丢；不满足则降级为「需重启」明示提示

---

## 附：待人工冒烟（无法自动化，M4 验收遗留）

- [ ] 真实终端（Windows Terminal）下：鼠标滚轮/点击/拖选、OSC 52 剪贴板复制、
      标签栏切换/关闭、rewind 时间线，各验一次
- [ ] 真实 stdio MCP server（如 filesystem）：拉起 → tools/list → 审批 → 调用链路
