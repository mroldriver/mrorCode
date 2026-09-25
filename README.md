# mrorCode

A fast terminal AI coding agent — Rust 编写的终端 AI 编码 Agent，TUI 视觉/交互对标 OpenCode，后端架构借鉴 AtomCode。

单进程、单二进制、无 worker 进程与固定帧率；流式渲染走增量 Markdown 与窗口化绘制，空闲时 0 唤醒。

## 特性

- **多轮工具闭环**：`read_file` / `write_file` / `edit_file` / `glob` / `grep` / `bash`，工具结果自动回灌模型。
- **`task` 只读子 agent**：模型可委派只读研究子任务（读/搜工具集），结果回灌为普通工具卡。
- **流式 TUI**：增量 Markdown、syntect 语法高亮、工具折叠卡、diff 着色、toast 通知（按级别着色）。
- **diff viewer**：`leader d` 打开 unified/split 双视图，万行 diff 窗口化渲染 < 16ms。
- **reasoning 折叠**：思考过程默认折叠为一行，`leader r` 展开。
- **rewind**：`leader w` 回退当前会话到历史某轮之前（仅内存生效，磁盘 transcript 保留）。
- **多会话标签栏**：`leader [` / `leader ]`（或 `leader 1..9`）切换标签页。
- **鼠标与选区复制**：滚轮/点击/拖选可用；选中内容按屏幕所见提取，复制走 OSC 52，退出不丢。
- **主题**：内置 vendored 的 33 个 opencode MIT 主题（dark/light 双模式）。
- **命令面板与键位**：`ctrl+p` 命令面板、leader 键（默认 `ctrl+x`）、用户键位覆盖。
- **多会话 + 磁盘持久化**：会话列表、新建/切换/删除；transcript + 快照落盘，`--resume` 恢复。
- **上下文压缩**：超过阈值自动折叠较早轮次，保留 system + 近 N 轮。
- **MCP stdio**：config 声明 server，动态注册其工具到调用链。
- **输入层**：`@` 文件补全（fuzzy）、`/` 斜杠命令。
- **plan 模式**：`Shift+Tab` 切换，写类工具被 gate 拦截，只读调研。
- **安全**：所有文件工具强制 working_dir 内解析，拒绝 `..`/符号链接逃逸；危险命令走审批。

## 安装

需要 Rust **1.88+**（见 `rust-toolchain.toml`）。

```powershell
# 1) 首次 clone 后修复本机工具链（Windows 需要；见「Windows 链接」）
pwsh -File scripts/bootstrap-tools.ps1

# 2) 构建 release 二进制
cargo build --release

# 3) 安装到用户 PATH（~/.cargo/bin，无需管理员）
pwsh -File scripts/install.ps1
```

安装后任意目录新开终端即可运行 `mrorcode`。

> **Windows 链接**：`.cargo/config.toml` 由 `bootstrap-tools.ps1` 生成（解包 binutils 到 `.tools/mingw`）。缺少它时 windows-gnu `raw-dylib` 链接会失败（`dlltool` 需要同目录的 `as.exe`）。

## 快速开始

```powershell
# 直接启动 TUI
mrorcode

# 一次性任务（无交互），输出流式文本
mrorcode run "解释一下这个项目的结构"

# 版本化 JSONL 事件流（供自动化消费）
mrorcode run --json "hi"

# 恢复历史会话
mrorcode --resume 3

# 切换工作目录 / 模型 / provider 条目
mrorcode -C D:\some\project --model deepseek-chat --provider local
```

## 配置

优先级（高 → 低）：

1. 环境变量：`MRORCODE_BASE_URL` / `MRORCODE_API_KEY` / `MRORCODE_MODEL` / `MRORCODE_PROVIDER`
2. `$MRORCODE_CONFIG` 指定的文件
3. `~/.mrorcode/config.toml`

参考 `config.example.toml`（仅作参考，不会自动安装）：

```toml
default_model = "deepseek-chat"

[providers.default]
type = "openai_compat"
base_url = "https://api.deepseek.com/v1"
api_key = "sk-your-key"
model = "deepseek-chat"
context_window = 128000
# 同 provider 下可切换的额外模型（模型选择对话框数据源）
models = ["deepseek-reasoner"]
```

本地 OpenAI 兼容端点（Ollama / vLLM 等，`api_key` 可留空）：

```toml
[providers.local]
type = "openai_compat"
base_url = "http://127.0.0.1:11434/v1"
api_key = ""
model = "qwen2.5-coder:7b"
```

### 上下文压缩

```toml
[compaction]
keep_recent = 6           # 保留最近 N 个 user turn 原文
threshold_tokens = 100000 # 估算 token 超过则压缩；0 = 关闭
```

### MCP stdio server

```toml
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
env = { FOO = "bar" }
```

启动时拉起子进程、`initialize` 协商版本、`tools/list` 动态注册为 `mcp__<server>__<tool>`；
断连 fail-closed 并告警（调用返回 is_error）。MCP 工具默认 `Risky`（需审批），除非 server 声明只读。

## 键位（默认）

| 键 | 动作 |
|---|---|
| `enter` | 提交 |
| `ctrl+j` / `alt+enter` | 换行 |
| `esc esc` | 中断当前请求 |
| `ctrl+c` | 退出 |
| `ctrl+p` / `leader p` | 命令面板 |
| `ctrl+n` / `leader n` | 新建会话 |
| `ctrl+l` / `leader l` | 会话列表 |
| `leader t` / `leader m` | 切换主题 / 模型 |
| `leader r` | 展开/收起 reasoning 思考块 |
| `leader d` | 打开当前工具卡的 diff viewer |
| `leader w` | rewind：回退当前会话到历史某轮之前 |
| `leader [` / `leader ]`（或 `leader 1..9`） | 上/下会话标签切换 |
| `f2` / `leader b` | 切换侧栏 |
| `shift+tab` | 切换 plan 模式 |
| `pageup` / `pagedown` | 滚动消息 |
| 鼠标滚轮 / 点击 / 拖选 | 滚动 / 交互 / 选区（复制走 OSC 52） |

leader 键默认 `ctrl+x`（2s 超时窗口）。键位可通过 `~/.mrorcode/state.json` 的 `keybinds` 覆盖。

## 开发

```powershell
cargo test --workspace                          # 全部测试
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build --release                           # -> target/release/mrorcode.exe
cargo bench -p mrorcode-tui --bench render      # 性能基准（release gate）
```

无真实 API 的端到端验证：provider 的单测用 `wiremock` 打本地 mock HTTP；
kernel 的 `test-support` feature 提供 `ScriptedProvider`/`EchoTool` 供集成测试（见
[AGENTS.md](AGENTS.md) 的 Gotchas）。

> 根目录 `cargo build` / `cargo test` 只覆盖 `mrorcode-cli`（`default-members`），
> 请始终加 `--workspace` 或 `-p <crate>`。

## 文档

- [AGENTS.md](AGENTS.md) — 开发者/Agent 快速上手：命令、架构规则、注意事项
- [docs/project-plan-atomcode.md](docs/project-plan-atomcode.md) — 产品总体设计与技术选型
- [docs/roadmap.md](docs/roadmap.md) — M1–M4 里程碑计划与完成报告、性能基线，M5 方向
- [docs/README.md](docs/README.md) — 文档索引

## 状态

`0.5.2` — M1（骨架）、M2（TUI 核心体验）、M3（Agent 能力增强）、M4（TUI 精致化）、M5-b（Anthropic provider）已完成；
reasoning 折叠、鼠标/选区/剪贴板、which-key、侧边栏、rewind、`task` 子 agent）已完成；M5（硬化发布）规划中，
见 [docs/roadmap.md](docs/roadmap.md) 末节。

## License

MIT（vendored 主题来自 opencode，见 `crates/mrorcode-tui/licenses/`）。
