# mrorCode 项目计划

> Rust 编写的终端 AI 编码 Agent，TUI 视觉/交互对标 OpenCode，后端架构借鉴 AtomCode。

---

## 一、项目定位与目标

**核心目标**：用 Rust 打造一个终端 AI 编码 Agent，TUI 视觉/交互对标 OpenCode，后端架构借鉴 AtomCode 的分层设计。

| 维度 | 借鉴来源 | 说明 |
|---|---|---|
| 后端核心架构 | AtomCode (Rust) | kernel → capabilities → coding → tuix 分层 |
| TUI 视觉与交互 | OpenCode (TS) | 侧边栏、多会话标签、命令面板、对话框系统、主题插件 |
| 语言 | Rust | 全栈 Rust，单二进制部署 |
| 终端渲染 | Rust 生态选型 | 见下方「TUI 技术选型」 |

---

## 二、TUI 技术选型（关键决策）

OpenCode 的 TUI 基于 `@opentui/solid`（类 Solid.js 响应式 + TSX 渲染到终端 ANSI），Rust 生态没有对等物。

### 方案对比

| 方案 | 优点 | 缺点 | 推荐度 |
|---|---|---|---|
| **ratatui + 自研状态层** | 生态成熟、组件多、上手快 | immediate-mode，复杂 UI 状态管理累 | ⭐⭐⭐⭐ |
| **ratatui + 自研 retained 抽象** | 兼顾生态和可维护性 | 需要额外抽象层开发 | ⭐⭐⭐⭐⭐ |
| **完全自研渲染器 (atomcode 路线)** | 极致性能、完全可控 | 工程量巨大（retained.rs 就 2.6 万行） | ⭐⭐ |
| **crossterm 直写** | 零依赖、灵活 | 所有组件从零写 | ⭐ |

**推荐方案：ratatui 作为渲染底座 + 自研 retained-mode 状态/组件层**

理由：
- ratatui 是 Rust TUI 事实标准，生态最丰富（文本输入、文件树、对话框等都有现成 crate）
- 在其上加一层响应式状态管理（类似 OpenCode 的 context 模式），既能快速迭代，又保留了未来替换渲染底层的能力
- 比完全自研节省 60%+ 的 TUI 工程量

---

## 三、整体架构（Workspace Crate 划分）

```
mrorcode/
├── Cargo.toml                    # workspace 根
├── crates/
│   ├── mrorcode-kernel/          # L0 核心内核 — Agent 抽象、消息、工具、会话
│   ├── mrorcode-providers/       # L1a — 模型提供商 (OpenAI/Anthropic/DeepSeek/Ollama…)
│   ├── mrorcode-tools/           # L1b — 内置工具 (文件读写、shell、git、grep…)
│   ├── mrorcode-mcp/             # L1c — MCP 协议支持（Model Context Protocol）
│   ├── mrorcode-coding/          # L2  — 编码 Agent 运行时（自律循环、子代理、plan 模式）
│   ├── mrorcode-config/          # 横向 — 配置系统 (TOML + 环境变量)
│   ├── mrorcode-tui/             # UI 层 — ratatui + 自研组件/状态
│   └── mrorcode-cli/             # 入口 — CLI 解析、启动 TUI/守护进程
```

### 各 crate 职责详解

#### `mrorcode-kernel` — 核心内核（最先开发）
- **核心类型**：`Session`、`Message`、`Tool`、`ToolCall`、`Agent` trait
- **事件系统**：`Event` 枚举，用于 UI 与运行时解耦
- **会话持久化**：会话快照、压缩（compaction）、历史记录
- **不依赖**任何业务 crate，纯抽象 + tokio + serde

#### `mrorcode-providers` — 模型提供商
- `Provider` trait：`chat()`、`stream_chat()`
- 实现：OpenAI 兼容、Anthropic、DeepSeek、Ollama、本地文件 mock
- 流式输出（SSE）是核心

#### `mrorcode-tools` — 内置工具集
- 文件操作（读、写、编辑、glob）
- 命令执行（shell，带 sandbox 配置）
- Git 操作（status、diff、log）
- 代码搜索（grep、符号查找）

#### `mrorcode-mcp` — MCP 协议支持
- MCP client 实现（stdio transport）
- 动态工具加载与调用
- 参考 AtomCode 的 capabilities 实现

#### `mrorcode-coding` — 编码 Agent 运行时
- 自律循环：LLM → 工具调用 → 结果 → LLM …
- 子代理 / 团队模式
- Plan 模式（先规划后执行）
- Rewind / 回滚
- 进度事件 → TUI 订阅

#### `mrorcode-config` — 配置系统
- 配置文件路径（`~/.mrorcode/config.toml` + 项目级）
- Provider 配置、模型选择、按键绑定、主题
- 热加载 / 动态修改

#### `mrorcode-tui` — 终端界面（对标 OpenCode）
- 主界面：左侧栏（会话列表/工作区）+ 中间会话视图 + 底部输入框
- 组件：Markdown 渲染、代码高亮、文件树、命令面板、对话框系统
- 主题系统：支持 JSON 主题文件 + 插件扩展
- 输入：多行编辑器、历史记录、剪贴板、附件

#### `mrorcode-cli` — CLI 入口
- 子命令：`mrorcode`（默认启动 TUI）、`mrorcode run`（一次性任务）、`mrorcode init`
- 参数解析（clap）
- 组装各模块并启动

---

## 四、TUI 功能对标清单（按优先级）

### P0 — 可用版本
- [ ] 主界面三栏布局：侧边栏（会话列表）+ 对话区 + 输入框
- [ ] 消息流式渲染（Markdown + 代码块高亮）
- [ ] 工具调用状态展示（spinner + 折叠展开）
- [ ] 输入框：多行输入、历史记录、提交
- [ ] 会话切换 / 新建
- [ ] 键盘快捷键系统

### P1 — 完整体验
- [ ] 命令面板（Ctrl+K / Ctrl+P），模糊搜索
- [ ] 对话框系统（模型选择、Provider 配置、设置、文件选择）
- [ ] 侧边栏：工作区切换、文件树
- [ ] 会话标签页（多会话并排）
- [ ] 主题系统（内置暗色/亮色 + 自定义 JSON）
- [ ] 代码审查视图（diff 高亮）
- [ ] 剪贴板支持（复制代码块）

### P2 — 高级特性
- [ ] 插件系统（类似 OpenCode 的 tui.json 插件，但是用 Rust 动态库 / 配置驱动）
- [ ] 思考过程折叠 / 展开（think 标签）
- [ ] 子代理 / 团队视图
- [ ] 重写 / Rewind 时间线
- [ ] 图片附件渲染（sixel / iterm2 / unicode block）
- [ ] 国际化（i18n）
- [ ] 鼠标交互（滚动、点击、选择）

---

## 五、开发阶段规划

### 第一阶段：骨架搭建（1-2 周）
**目标：跑通最小闭环 — 发消息 → 调 LLM → 显示回复**

1. 初始化 workspace + 所有空 crate
2. kernel：定义 `Session`、`Message`、`Tool`、`Agent` trait 核心类型
3. providers：实现 OpenAI 兼容 + 流式输出
4. tools：实现 `read_file` + `bash` 两个核心工具
5. coding：最小 Agent 循环（单轮 + 简单工具调用）
6. tui：ratatui 基础框架 + 最简对话界面 + 输入框
7. cli：`mrorcode` 命令能启动 TUI 并对话

### 第二阶段：TUI 核心体验（2-3 周）
**目标：TUI 达到日常可用水平**

1. Markdown 渲染（pulldown-cmark → 终端样式）
2. 代码块语法高亮（syntect）
3. 工具调用卡片（展开/折叠、状态动画）
4. 侧边栏 + 会话列表 / 切换
5. 输入框增强（多行、历史、Ctrl+Enter 提交）
6. 配置系统 + 模型选择对话框
7. 命令面板雏形

### 第三阶段：Agent 能力增强（2-3 周）
**目标：编码 Agent 功能完善**

1. 完整工具集（文件编辑、git、grep、glob）
2. 自律循环优化（上下文管理、自动压缩）
3. MCP 协议支持
4. 子代理 / 任务分解
5. Plan 模式
6. 会话持久化（保存到磁盘）

### 第四阶段：TUI 精致化（2-3 周）
**目标：视觉/交互接近 OpenCode 水平**

1. 会话标签页（多会话并排管理）
2. 完整对话框系统（设置、Provider 管理、文件选择器）
3. 文件树侧边栏
4. 主题系统 + 多套内置主题
5. Diff 查看器
6. 命令面板完善（模糊搜索 + 所有命令注册）
7. 鼠标支持

### 第五阶段：打磨与发布（1-2 周）
1. 错误处理与日志系统
2. 性能优化（大会话流畅度）
3. 跨平台测试（macOS / Linux / Windows）
4. 打包与发布（cargo dist / GitHub Actions）
5. 文档与示例

---

## 六、关键技术栈

| 类别 | 选型 | 说明 |
|---|---|---|
| 异步运行时 | tokio | 全项目统一 |
| 终端 I/O | crossterm | 跨平台终端事件 |
| TUI 渲染 | ratatui | 主流成熟方案 |
| 文本输入 | ratatui-input 或自研 | 多行编辑器需要自研 |
| Markdown | pulldown-cmark | 解析 + 自写终端渲染 |
| 语法高亮 | syntect | 代码块高亮 |
| 配置 | serde + toml | TOML 配置文件 |
| CLI 解析 | clap | 标准选型 |
| 错误处理 | anyhow + thiserror | 库用 thiserror，应用用 anyhow |
| HTTP | reqwest (rustls) | 调用 LLM API |
| 数据流 | tokio-stream + futures | 流式输出 |
| 状态管理 | 自研响应式状态树 | 类似 OpenCode 的 context 模式 |
| 数据库（可选） | sqlite + rusqlite | 会话持久化 / 历史 |

---

## 七、与两个参考项目的对应关系

| 功能模块 | AtomCode 对应 | OpenCode 对应 | 我们的位置 |
|---|---|---|---|
| 核心内核 | `atomcode-kernel` | `packages/opencode/src/services/agent/` | `mrorcode-kernel` |
| 编码 Agent | `atomcode-coding` | `packages/opencode/src/agent/` | `mrorcode-coding` |
| 工具集 | `atomcode-capabilities` | `packages/opencode/src/tools/` | `mrorcode-tools` |
| Provider | `atomcode-capabilities/providers` | `packages/opencode/src/providers/` | `mrorcode-providers` |
| MCP | `atomcode-capabilities/mcp` | `packages/opencode/src/mcp/` | `mrorcode-mcp` |
| TUI | `atomcode-tuix` | `packages/tui/` | `mrorcode-tui` |
| CLI | `atomcode-cli` | `packages/opencode/src/cli/` | `mrorcode-cli` |
| 配置 | `atomcode-config` | `packages/opencode/src/config/` | `mrorcode-config` |

---

## 八、待确认事项

- [ ] TUI 渲染层选型确认：ratatui + 自研状态层 vs 完全自研
- [ ] 第一阶段范围确认：最小可用（单轮对话）vs 多轮 + 工具
- [ ] 项目命名确认：`mrorCode` / `mrorcode`
- [ ] 骨架代码是否立即开始
