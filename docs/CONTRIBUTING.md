# 贡献指南

感谢你对 mrorCode 的兴趣！本文档介绍如何参与贡献，包括开发环境搭建、项目结构、代码规范、提交流程和调试技巧。

---

## 一、快速开始

### 1.1 环境要求

- **Rust**：MSRV 1.88（`rust-toolchain.toml` 已钉死工具链版本，rustup 会自动使用）
- **操作系统**：Windows（主要开发平台）、Linux、macOS
- **Windows 额外**：需要先运行 `scripts/bootstrap-tools.ps1`（见下文）

### 1.2 克隆与初始化

```powershell
# 克隆仓库
git clone <repo-url>
cd mrorCode

# Windows：初始化本机工具链（解决 windows-gnu 的 dlltool 链接问题）
pwsh -File scripts/bootstrap-tools.ps1

# 验证构建
cargo build --release
```

`bootstrap-tools.ps1` 做什么：
- 下载 MSYS2 binutils/gcc-libs/zstd 等 6 个小包（约 10MB）到 `.tools/mingw/`
- 生成 `.cargo/config.toml`，配置 `dlltool` 路径
- 零系统级改动，不修改 PATH，所有东西都在项目目录内

### 1.3 配置 API

mrorCode 需要一个 LLM provider 才能运行。最快的方式是用环境变量：

```powershell
$env:MRORCODE_BASE_URL = "https://api.deepseek.com/v1"
$env:MRORCODE_API_KEY = "sk-your-key"
$env:MRORCODE_MODEL = "deepseek-chat"
```

或者创建配置文件：

```powershell
# 复制样例
New-Item -ItemType Directory -Force ~/.mrorcode
cp config.example.toml ~/.mrorcode/config.toml
# 然后编辑填入你的 key
```

验证：

```powershell
cargo run -- run "你好"
```

---

## 二、项目结构速览

```
mrorCode/
├── crates/
│   ├── mrorcode-kernel/       # L0 中立内核（agent 循环、消息、trait 抽象）
│   ├── mrorcode-capabilities/ # L1 具体能力（provider、工具、MCP、审批）
│   ├── mrorcode-agent/        # L2 组装层（把 kernel + capabilities 装起来）
│   ├── mrorcode-config/       # leaf 配置（TOML + 环境变量）
│   ├── mrorcode-tui/          # L3 ratatui 终端界面
│   └── mrorcode-cli/          # L3 CLI 入口 + headless
├── docs/                       # 设计文档、路线图、待办
├── scripts/                    # bootstrap、install 脚本
├── Cargo.toml                  # workspace 根（版本号在这里）
├── rust-toolchain.toml         # 钉死 Rust 版本和目标三元组
└── AGENTS.md                   # 开发者快速参考（必读）
```

**依赖方向**（单向，不可反向）：

```
cli → tui → agent → capabilities → kernel
              config（leaf，谁都可以读）
```

**核心铁律**：
1. 内核（kernel）不包含任何产品策略
2. agent/会话的**唯一权威状态**在 kernel runtime，TUI/CLI 只做事件投影 + 发命令
3. 不在 driver 层建第二份状态机

---

## 三、常用开发命令

### 3.1 构建与测试

```powershell
# 全 workspace 测试（必加 --workspace，默认只测 cli）
cargo test --workspace

# 只测某个 crate
cargo test -p mrorcode-kernel
cargo test -p mrorcode-tui

# 某个具体测试
cargo test -p mrorcode-kernel compact_keeps_system_and_recent_turns

# clippy 门禁（零告警）
cargo clippy --workspace --all-targets -- -D warnings

# 格式检查
cargo fmt --check

# 格式化
cargo fmt

# release 构建
cargo build --release
```

### 3.2 性能基准

```powershell
# TUI 渲染基准（< 16ms/frame 是 release gate）
cargo bench -p mrorcode-tui --bench render

# 内核压缩基准
cargo bench -p mrorcode-kernel --bench compaction

# 搜索工具基准
cargo bench -p mrorcode-capabilities --bench search
```

### 3.3 运行

```powershell
# 启动 TUI（默认）
cargo run

# headless 一次性任务
cargo run -- run "解释一下这个项目"

# JSONL 模式
cargo run -- run --json "hi"

# 指定工作目录 / 模型 / provider
cargo run -- -C D:\some\project --model deepseek-reasoner --provider local
```

### 3.4 更新 headless 黄金文件

```powershell
# 确认改动合理后，刷新黄金基线
$env:UPDATE_GOLDEN = "1"
cargo test -p mrorcode-cli --test headless_json
```

---

## 四、代码规范

### 4.1 通用规则

- **注释语言**：中文（与既有注释、设计文档保持一致）
- **错误处理**：用 `thiserror` 定义库错误，用 `anyhow` 在应用层（CLI/TUI）做最终错误
- **unsafe**：原则上不使用；确有必要时加详细注释说明安全性论证
- **日志**：用 `tracing`，debug/info 级按需加；热路径不要打日志

### 4.2 测试原则

- 新功能必须带测试，bug 修复必须加回归测试
- 内核功能优先用 `test-support` feature 的 `ScriptedProvider`/`EchoTool` 测，不依赖真实 API
- Provider 用 `wiremock` 打本地 mock HTTP 测，不联网
- TUI 用 `TestBackend` 做渲染断言，不依赖真实终端
- 纯函数优先用单元测试，集成测试放在 `tests/` 目录

### 4.3 性能文化

这是项目最核心的文化之一——**速度是第一性约束，不是后期优化项**。

- **先量化再优化**：改性能前先写基准，有基线再动手
- **无 benchmark 证明，不加缓存**：unicode-width 缓存曾被实测慢 10 倍，已删除
- **渲染帧预算 < 16ms**：万行消息窗口化渲染必须达标
- **空闲 0 唤醒**：没有事件时事件循环完全阻塞，不挂定时器

### 4.4 版本号规则

- **唯一真相**：根 `Cargo.toml` 第 7 行的 `[workspace.package] version`
- 所有 crate 都用 `version.workspace = true`，不要单独写版本
- **项目规则**：每次碰代码的改动都 bump 版本（docs-only 不用）
- bump 后跑一次 `cargo check` 让 `Cargo.lock` 同步

```toml
# Cargo.toml
[workspace.package]
version = "0.5.63"   # ← 改这一行
```

---

## 五、提交流程

### 5.1 提交前检查清单

每次提交前确保：

```powershell
# 1. 测试全绿
cargo test --workspace

# 2. clippy 零告警
cargo clippy --workspace --all-targets -- -D warnings

# 3. 格式正确
cargo fmt --check

# 4. release 能构建
cargo build --release
```

### 5.2 Commit Message

没有严格的 commitizen 格式，但建议：

- 标题简明扼要，说明做了什么
- 必要时 body 里解释为什么这么做、有什么权衡
- 如果是 bug 修复，说明触发条件和修复思路

示例：

```
fix: 修复审批中间件未接线导致 Risky 工具直行的安全问题

agent::runtime::start() 漏调了 AgentBuilder::add_middleware，
导致 ApprovalMiddleware 形同虚设。已接线并加回归测试
middleware_is_wired_into_the_agent，撤掉修复即失败。
```

### 5.3 文档同步

以下情况需要同步更新文档：

- 项目结构变化 → `AGENTS.md`
- 架构/选型变化 → `docs/project-plan-atomcode.md`
- 新里程碑/完成里程碑 → `docs/roadmap.md`
- TUI 布局改动 → `docs/tui-layout-spec.md`
- 新增文档 → `docs/README.md` 索引

---

## 六、调试技巧

### 6.1 日志

```powershell
# 开启 debug 日志
$env:RUST_LOG = "debug"
cargo run -- run "hi"

# 只看某个 crate
$env:RUST_LOG = "mrorcode_kernel=debug,mrorcode_capabilities=info"
```

日志输出到 stderr，不影响 headless 的 stdout 输出。

### 6.2 无 API 调试

没有真实 API key 也可以做大部分开发和测试：

- **内核测试**：用 `test-support` feature 的 `ScriptedProvider`，完全可控
- **Provider 测试**：用 `wiremock` 本地 mock HTTP
- **TUI 测试**：用 `TestBackend`，不需要真实终端
- **端到端冒烟**：可以搭一个本地 mock SSE 服务器（Python 几十行就行）

### 6.3 panic 时恢复终端

TUI panic 时会尝试恢复终端（raw mode / alternate screen / 光标）。
如果失败了手动恢复：

```powershell
# 在 PowerShell 里
Reset-ComputerMachinePassword  # 不行就关掉终端重开
```

### 6.4 性能分析

```powershell
# 用 criterion 找热点
cargo bench -p mrorcode-tui --bench render

# 或者用 samply（需要先安装）
samply record cargo run --release
```

---

## 七、架构边界与常见误区

### 7.1 什么应该放哪里？

| 场景 | 放哪里 |
|---|---|
| 新的 agent 机制（如取消、压缩、多会话） | kernel |
| 新的 LLM provider 实现 | capabilities/provider/ |
| 新的内置工具 | capabilities/tools/ |
| 审批策略、权限规则 | capabilities + middleware |
| 把 provider + tools + 审批装起来 | agent/assemble.rs |
| 配置解析 | config crate |
| UI 组件、状态管理 | tui/ |
| 命令行参数、headless 输出 | cli/ |

### 7.2 常见反模式

1. ❌ **在 TUI store 里维护会话状态**：权威状态在 kernel，store 只存投影
2. ❌ **直接操作 `Conversation`**：必须通过 `AgentCommand` 发给 kernel
3. ❌ **凭感觉加缓存**：先写 benchmark 证明有收益再加
4. ❌ **用 unwrap()**：除了确定不可能失败的情况（test 代码、初始化期），都用 `?` 或显式处理
5. ❌ **绕过 path_guard**：所有文件操作必须走路径安全校验

---

## 八、找什么？从哪里开始？

### 想加一个新工具

1. 在 `crates/mrorcode-capabilities/src/tools/` 新建文件
2. 实现 `Tool` trait（`name` / `description` / `parameters_schema` / `risk` / `execute`）
3. 在 `tools/mod.rs` 的 `builtin_tools()` 里注册
4. 加单测
5. 如果有 UI 特殊展示需求（如 diff），在 `tui/src/widgets/` 对应地方接线

### 想加一个新 provider

1. 在 `crates/mrorcode-capabilities/src/provider/` 新建文件
2. 实现 `LlmProvider` trait，复用 `sse.rs` 的解码器
3. 在 `config` 里加 provider type 白名单
4. 在 `agent/assemble.rs` 里加装配分支
5. 用 wiremock 写集成测试
6. 跑 conformance 套件验证事件序列正确

### 想改 TUI 布局

1. 先读 `docs/tui-layout-spec.md`
2. 改之前先查 opencode 源码（如果是对标功能）
3. 用 `TestBackend` 写渲染断言
4. 跑 render benchmark 确认没有性能回退

---

## 九、获取帮助

- 读 `AGENTS.md`——最核心的开发速查
- 读 `docs/` 下的设计文档
- 看现有测试——很多用法示例都在测试里
- 看 `docs/todos/` 里的待办清单，了解进行中的工作
