# REPL 输出排版改进（2026-10-28）

> **Issue**：[#11 REPL 输出排版改进：空行间距 / 用户输入颜色 / 工具卡间距 / 长 Markdown 折叠](https://gitcode.com/mroldriver/mrorCode/issues/11)
> 来源：`D:\_mroldriver\mrorCode\.local\logs\mrorcode.log`（v0.6.5 bare ANSI REPL 实测）
> 范围：`crates/mrorcode-cli/src/repl.rs`（bare ANSI REPL，非 TUI）
> 性质：纯输出排版与可读性改进，不涉及内核/会话逻辑。

---

## 问题 1：启动横幅与提示符之间缺空行（P2）

**现象**（日志第 5–7 行）：

```text
permission:  full
· /help for commands · /exit or Ctrl-D to quit
› hello
```

`· /help ...` 行与 `› hello` 提示符紧贴，视觉上没有「界面已就绪、请输入」的呼吸感。

**期望**：

```text
permission:  full
· /help for commands · /exit or Ctrl-D to quit

› hello
```

**位置**：`crates/mrorcode-cli/src/repl.rs:201-207`

```rust
let _ = writeln!(
    out,
    "{}  {}\n{}",          // ← 当前：permission 行 + \n + help 行，help 行直接贴下一个 prompt
    dim("permission:"),
    green(mode_name(mode)),
    dim("· /help for commands · /exit or Ctrl-D to quit")
);
```

**修复方向**：把 `\n{}` 改为 `\n\n{}\n`（help 行后再补一个空行），或在主循环第一次 `Prompt` 前统一 `writeln!(out)`。

**验收**：启动后 help 行与第一个 `›` 之间恰好一个空行，不多不少（后续轮次已由 `render.blank()` 控制，不受影响）。

---

## 问题 2：用户输入的文本没有颜色（P1）

**现象**（日志第 7、13 行）：

```text
› hello
...
› 还有哪些issues
```

`› ` 提示符是青色（`cyan("› ")`，`repl.rs:150`），但用户敲的内容（`hello` / `还有哪些issues`）是终端默认前景色，与模型输出（也默认色）混在一起，无法一眼区分「我问了什么」vs「模型答了什么」。

**期望**：用户输入的文本带一个区分色（如亮白/粗体或浅绿），与模型正文、工具卡形成三档视觉层级。

**难点**：行输入走 rustyline（`repl.rs:152` `rl.readline("› ")`），用户已敲的字符由 rustyline 直接控制回显，REPL 主循环拿不到「用户刚输入了什么」的文本去事后染色。可选方案：

| 方案 | 做法 | 代价 |
|---|---|---|
| A. 输入前行内 echo | 用户按回车后，主循环立刻 `writeln!(out, "› {line}")` 重打一行带颜色的 | rustyline 自己已回显过一次，会出现「灰字一遍 + 彩字一遍」重复 |
| B. 关闭 rustyline 回显 | `readline` 改成裸读，自己管理行编辑 | 失去方向键/历史/补全，工作量大 |
| C. 仅染色提示符后的「已提交快照」 | 在 submit 前把 `trimmed` 以粗体/彩色重打到独立行 | 接受 rustyline 原始回显为「草稿」，回车后另起一行彩色「确认」——视觉上类似「提交日志」 |
| D. 染色仅在日志/落盘层 | 不动交互回显，只在 log 文件里给 user 行加 ANSI 色 | 交互体验不变，但 log 文件可读性提升 |

**建议**：先做 D（零风险），再评估 C（提交后另起一行彩色 `› <用户文本>`，把 rustyline 的原始回显视为「正在输入」的临时态）。

**位置**：`repl.rs:150-158`（提示符与提交）、`repl.rs:235-236`（submit 前）。

---

## 问题 3：模型过渡句与工具卡之间缺空行（P1）

**现象**（日志第 15–16 行）：

```text
我先看看项目里有没有 issue 相关的文件（比如 TODO.md、ISSUES.md 等），同时了解一下项目结构。
● 查看项目根目录结构
└ ✗ 执行命令(ls -la "D:\_mroldriver\mrorCode") → 失败
```

模型最后一句正文与第一张工具卡 `● ...` 紧贴，读起来像「句子的一部分」而非「动作开始」。

**对比**：第 13 行 `› 还有哪些issues` 与第 15 行正文之间有一个空行（这是 `repl.rs:235` 的 `writeln!(out)` 打的），但正文→工具卡之间没有。

**位置**：`repl.rs:338-339`

```rust
// 模型正文常停在句子中间，工具卡必须自己另起一行。
render.line(out, &format!("{} {}", cyan("●"), bold(&header)));
```

`render.line()` 只保证「另起一行」（`break_line`），但不保证「正文与工具卡之间有一个空行」——因为正文流式输出后 `at_line_start` 可能是 false（句子中间），`break_line` 只补一个 `\n` 把光标送到行首。

**期望**：工具卡前的间距 = 正文行结束后 + 恰好一个空行（与「用户输入↔正文」的间距一致）。

**修复方向**：给 `ReplRender` 加一个 `gap_before_card: bool` 状态（或在 `ToolStarted` 分支里检测 `!at_line_start` 时打两个 `\n` 而非一个）。即：

- 正文以 `\n` 结尾（段落自然结束）→ 工具卡前不需要额外空行（已有一个）；
- 正文停在句子中间（`at_line_start == false`）→ 打 `\n\n`（一个把光标送到行首，一个形成空行）。

**验收**：用日志第 13–16 行的形态做单测断言：`›` 行、空行、正文、空行、`●` 行，四行间距一致。

---

## 问题 4：任务完成后的大段 Markdown 如何处理（P1，设计待定）

**现象**（日志第 46–161 行）：模型在一轮末尾输出了 ~115 行的 Markdown 报告（含 `##` 标题、`---` 分隔线、表格、列表、**粗体**）。当前 REPL 把它当作纯文本流式写出（`render.text()` 原样 `write!`），结果是：

- `## 🔴 最高优先级...` 的 `##` 两个井号原样显示，不渲染成标题；
- `---` 三横线原样显示，不是分隔线；
- `**来源：...**` 的星号原样显示；
- `| 编号 | 问题 |` 表格的竖线原样显示，不对齐；
- 一整屏滚过去，用户要回看只能往上翻终端 scrollback。

**这不是 bug，是 bare REPL 的设计取舍**：TUI 有 `MarkdownView` 增量渲染，REPL 没有。但 v0.6.5 的实测体验是「大段 MD 直接糊脸」。

**候选方案**（按实现成本从低到高）：

| 方案 | 做法 | 优点 | 缺点 |
|---|---|---|---|
| A. **不渲染，仅折叠** | 检测到 turn 末尾输出 > N 行（如 40）时，只显示前 10 行 + `… (共 115 行，按 /full 展开)` | 零解析成本，屏幕不被刷屏 | 用户要看全文得敲命令 |
| B. **轻量行级装饰** | 不解析 inline，只做行首替换：`## ` → 加粗+青色、`### ` → 加粗、`---` → `────────`、`- [ ]` → `☐`、`- [x]` → `☑`、`**x**` → 去星号加粗 | 覆盖 80% 场景，手写 50 行 match 即可 | 表格仍不对齐、嵌套列表缩进不处理 |
| C. **复用 TUI 的 markdown crate** | 把 `mrorcode-tui` 的 markdown 解析逻辑抽成独立 crate，REPL 调用它生成 ANSI span | 与 TUI 视觉一致 | 工程量大（TUI 的 markdown 是为 ratatui 布局写的，不是为流式 ANSI 写的） |
| D. **落盘 + 提示路径** | 大段输出同时写到 `~/.mrorcode/last-turn.md`，屏幕只显示前 N 行 + 文件路径 | 用户可在编辑器里看完整版 | 需要管理文件生命周期 |

**建议**：

1. **先做 A（折叠）**：改动最小，立刻解决「刷屏」问题。阈值可配置（环境变量 `MRORCODE_REPL_PREVIEW_LINES`，默认 15）。
2. **再做 B（行级装饰）**：在折叠的预览部分应用行首替换，让「前 10 行」至少有标题色、列表符。
3. **C/D 列入 backlog**，等真实使用反馈再定。

**位置**：`repl.rs:278-288`（`ReplRender::text`）、`repl.rs:409-413`（`TurnComplete` 分支）。需要新增一个「本轮累积行数」计数器，在 `TurnComplete` 时判断是否超过阈值。

**验收**：

- 短回复（< 15 行）行为不变；
- 长回复（115 行）只显示前 15 行 + 一行灰色 `… (共 115 行；/full 查看全部)`；
- `/full` 命令把本轮完整文本打到 stdout（或从缓冲重放）；
- 单测：`render.text` 喂 100 行 `TextDelta` 后，`TurnComplete` 的输出含折叠提示。

---

## 落地顺序建议

1. **问题 1**（5 分钟）：启动横幅补空行，立刻见效。
2. **问题 3**（半小时）：正文→工具卡间距，改 `ReplRender` 状态机 + 单测。
3. **问题 2-D**（10 分钟）：日志文件给 user 行加 ANSI 色（如果 log 是 ANSI 流的话；纯文本 log 则改为加前缀 `[user]`）。
4. **问题 4-A**（半天）：长输出折叠 + `/full` 命令，含单测。
5. **问题 2-C / 4-B**（后续迭代）：评估真实使用反馈再做。

---

## 关联文件

- `crates/mrorcode-cli/src/repl.rs`：150（提示符）、201-207（启动横幅）、235（submit 前空行）、278-288（text 流式）、338-339（工具卡）、409-413（TurnComplete）
- 日志样本：`.local/logs/mrorcode.log`（不在仓库内，仅作证据）
