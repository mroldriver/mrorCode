# 内置工具协议

> 本文档详细列出 mrorCode 全部内置工具的调用契约：参数 schema、风险等级、返回格式、行为约定。
> 所有工具均遵循 [OpenAI Function Calling](https://platform.openai.com/docs/guides/function-calling) 协议形态。

---

## 总览

| 工具 | 风险等级 | 简介 |
|---|---|---|
| [read_file](#read_file) | Safe | 读取文件，支持分页 |
| [write_file](#write_file) | WorkspaceWrite | 整文件写入，产 diff |
| [edit_file](#edit_file) | WorkspaceWrite | 精确串替换，产 diff |
| [bash](#bash) | Risky* | 执行 shell 命令 |
| [glob](#glob) | Safe | 按模式列文件 |
| [grep](#grep) | Safe | 内容正则搜索 |
| [task](#task) | Risky | 只读子 agent 委派研究 |

\* bash 的风险等级由**命令词表**动态判定：只读命令（`ls`/`cat`/`grep`/`git status` 等）降为 Safe，其余为 Risky。

---

## 通用约定

### 路径安全

所有文件工具的 `path` 参数都经过 **路径安全 gate**（`path_guard.rs`）校验：

- 相对路径自动拼接到 `working_dir` 下
- 绝对路径直接使用，但必须落在 `working_dir` 内
- 拒绝 `..` 逃逸
- 拒绝符号链接指向工作区外
- Windows 额外拒绝 UNC / verbatim 前缀（`\\server\share`、`\\?\C:\...`）

违规返回结构化错误，模型可据此自纠。

### 结果截断

工具结果默认 **64 KiB head+tail 中央截断**（`DEFAULT_MAX_TOOL_RESULT_BYTES`）：

```
[头部 budget 字节]
...[N bytes truncated]...
[尾部 budget 字节]
```

截断是 UTF-8 安全的。设计动机：工具输出往往开头有结构信息、末尾有结论，中间最可能冗余。

### 错误返回

工具永不 panic。所有错误以 `is_error: true` 的 `ToolResult` 返回，错误文本回灌模型，模型可据此重试或调整策略。

---

## read_file

**风险等级**：Safe（纯只读）

读取工作目录内的 UTF-8 文本文件。

### 参数

```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "文件路径（相对工作目录或绝对路径）"
    },
    "offset": {
      "type": "integer",
      "description": "1-based 起始行号，默认从第 1 行开始",
      "minimum": 1
    },
    "limit": {
      "type": "integer",
      "description": "最大读取行数，默认全量，上限 2000",
      "minimum": 1,
      "maximum": 2000
    }
  },
  "required": ["path"]
}
```

### 行为

- 文件不存在 → `is_error: true`，错误信息含路径
- 非 UTF-8 → 用 `encoding_rs` 容错解码
- 超出总行数 → 读到文件末尾
- 大文件（默认 > 2000 行）→ 提示模型使用分页

### 返回格式

```
path/to/file.rs (42 lines total)
---
 1  | use std::collections::HashMap;
 2  | ...
 3  | ...
...
```

### 副作用

- 会更新 **read-before-write 追踪器**（`ReadTracker`）的文件指纹。写文件时会校验指纹一致，防止外部修改导致意外覆盖。

---

## write_file

**风险等级**：WorkspaceWrite（工作区内写）

写入或覆盖工作目录内的文件。

### 参数

```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "文件路径（相对工作目录或绝对路径）"
    },
    "content": {
      "type": "string",
      "description": "要写入的完整文件内容"
    }
  },
  "required": ["path", "content"]
}
```

### 行为

- **新建文件**：父目录必须存在，否则报错
- **覆盖已存在文件**：必须先读过该文件（read-before-write 校验），否则拒绝并提示先读
  - 校验通过 read_tracker 的文件指纹（大小 + mtime + 首/尾字节摘要）
  - 指纹不一致 → 拒绝并提示「文件已被外部修改，请重新读取」
- 产出 unified diff 作为结果的一部分

### 返回格式

成功时：

```
Wrote path/to/file.rs (42 lines)

diff --new file...
--- /dev/null
+++ path/to/file.rs
@@ ... @@
+content
...
```

失败时：`is_error: true` + 结构化错误原因（路径越界、需先读取、父目录不存在等）。

---

## edit_file

**风险等级**：WorkspaceWrite（工作区内写）

精确替换文件中的一段文本。比整文件写入更安全，因为失败时不会修改文件。

### 参数

```json
{
  "type": "object",
  "properties": {
    "path": {
      "type": "string",
      "description": "文件路径（相对工作目录或绝对路径）"
    },
    "old": {
      "type": "string",
      "description": "要查找的精确文本（必须完全匹配，含空格/缩进/换行）"
    },
    "new": {
      "type": "string",
      "description": "替换后的文本"
    },
    "replace_all": {
      "type": "boolean",
      "description": "是否替换所有匹配项，默认 false（只替换唯一匹配）"
    }
  },
  "required": ["path", "old", "new"]
}
```

### 行为

- 必须先读过该文件（read-before-write 校验，同 write_file）
- `replace_all = false`（默认）：
  - `old` 出现 0 次 → 结构化错误：「not found，上下文是什么」
  - `old` 出现 > 1 次 → 结构化错误：「ambiguous，出现了 N 次，请扩大匹配范围」
  - 恰好 1 次 → 替换成功，返回 diff
- `replace_all = true`：替换所有出现次数，返回 diff

### 返回格式

同 write_file：成功返回 diff，失败返回结构化错误。

---

## bash

**风险等级**：动态（按命令词表判定）

执行 shell 命令，返回合并的 stdout/stderr。

### 参数

```json
{
  "type": "object",
  "properties": {
    "command": {
      "type": "string",
      "description": "要执行的 shell 命令"
    },
    "timeout": {
      "type": "integer",
      "description": "超时秒数，默认 120，最大 600"
    }
  },
  "required": ["command"]
}
```

### 行为

- **Shell**：Windows 用 `cmd /C`，Unix 用 `sh -c`，`$SHELL` 优先
- **工作目录**：runtime 的 `working_dir`
- **超时**：默认 120 秒，最大 600 秒，超时进程被杀，返回错误
- **编码**：用 `encoding_rs` 容错解码输出

### 风险分类

`classify_command(command)` 用保守词表判定：

- **Safe**：`ls` / `cat` / `head` / `tail` / `grep` / `find` / `git status` / `git log` / `git diff` / `git branch` 等只读命令
- **Risky**：其余全部（写文件、网络调用、构建命令等）

> ⚠️ 词表是保守的。拿不准的命令一律按 Risky 处理，走审批。这是安全默认。

### 审批

- Risky 命令 → 弹审批弹窗（`once` / `always` / `reject`）
- 已授权的命令（`always` grant）→ 下次同命令不再询问
- `Full` 权限模式 → 跳过审批
- `ReadOnly` 权限模式 → 只放行 Safe 命令，其余直接拒绝（不弹审批）

---

## glob

**风险等级**：Safe（纯只读）

按 glob 模式列文件。

### 参数

```json
{
  "type": "object",
  "properties": {
    "pattern": {
      "type": "string",
      "description": "glob 模式，如 **/*.rs、src/*.toml"
    },
    "path": {
      "type": "string",
      "description": "搜索的子目录，默认工作目录"
    }
  },
  "required": ["pattern"]
}
```

### 行为

- 尊重 `.gitignore`（用 `ignore` crate 的 `WalkBuilder`）
- 返回按 mtime 倒序（最新修改的在前）
- 上限 2000 条，超出提示总数

### 返回格式

```
Found 142 files (showing 142)

src/main.rs
src/lib.rs
src/agent.rs
...
```

---

## grep

**风险等级**：Safe（纯只读）

用正则表达式搜索文件内容。

### 参数

```json
{
  "type": "object",
  "properties": {
    "pattern": {
      "type": "string",
      "description": "Rust regex 语法的正则表达式"
    },
    "path": {
      "type": "string",
      "description": "搜索的子目录，默认工作目录"
    },
    "include": {
      "type": "string",
      "description": "只搜索匹配该 glob 的文件，如 *.rs"
    },
    "ignore_case": {
      "type": "boolean",
      "description": "忽略大小写，默认 false"
    }
  },
  "required": ["pattern"]
}
```

### 行为

- 尊重 `.gitignore`
- 跳过二进制文件（按内容启发式检测）
- 返回格式 `path:line: text`
- 上限 200 行，超出给出总数

### 返回格式

```
Found 42 matches in 7 files

src/main.rs:15: use tokio::select;
src/agent.rs:89:     select! {
...
```

---

## task

**风险等级**：Risky（消耗 token / 时间，需审批）

委派一个只读研究子任务给子 agent。子 agent 只能使用只读工具（`read_file` / `glob` / `grep`），绝不编辑或执行命令。

### 参数

```json
{
  "type": "object",
  "properties": {
    "description": {
      "type": "string",
      "description": "可选的简短标签，显示在工具卡上（不发给子 agent）"
    },
    "prompt": {
      "type": "string",
      "description": "给只读子 agent 的任务指令，自然语言"
    }
  },
  "required": ["prompt"]
}
```

### 行为

- 子 agent 复用同一个 LLM provider
- 子 agent 有自己的 system prompt（「只读研究助手」人格）
- 最大 15 轮（`max_rounds = 15`），防止死循环
- 子 agent 完成后，最终文本作为工具结果回灌给主 agent

### 返回格式

子 agent 的最终回答文本。如果子 agent 因错误/取消终止，返回对应的错误信息。

> 注：当前版本 task 工具只返回最终文本（M4 默认回退方案）。实时嵌套视图（流式显示子 agent 的进度）在 M6 候选池中。

---

## MCP 工具

除了上述内置工具，mrorCode 还支持 **MCP（Model Context Protocol）stdio servers**。

- 配置：`config.toml` 的 `[[mcp.servers]]` 段
- 命名：`mcp__<server_name>__<tool_name>`（双下划线分隔，避免与内置工具冲突）
- 风险等级：默认 `Risky`，除非 server 声明 `annotations.readOnlyHint`
- 容错：单个 MCP server 启动失败只产生 warning，不阻塞整体运行

MCP 工具的参数 schema 由 server 在 `tools/list` 时动态提供，运行时注册到 `ToolRegistry`。
