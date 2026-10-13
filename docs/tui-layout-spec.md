# TUI 视觉布局规范

> 建档日期：2026-10-13
> 基准：本地 opencode 源码 `D:\opencode\packages\tui`（SolidJS + @opentui），对照 `crates/mrorcode-tui`（Rust + ratatui）
> 目的：固定已对齐 opencode 的视觉细节，避免后续凭猜测改布局。**改布局前先查这份文档和 opencode 源码，不要靠目测猜。**

---

## 1. 全局外壳（会话页）

来源：`crates/mrorcode-tui/src/app.rs` 的 `draw_session`。

```
+---------------------------------------+----------+
|  top pad (HEADER_H = 1)               |  (pad)   |
+---------------------------------------+          |
|  chat area (CONTENT_PAD = 1 both)     |  sidebar |
|                                       |  panel   |
|  prompt box                           |          |
|  meta line                            |          |
+---------------------------------------+----------+
```

- 顶部留白 `HEADER_H = 1`：主区与侧栏整体下移 1 行，作为视觉呼吸（对齐 opencode）。
- 主区左右 padding `CONTENT_PAD = 1`：聊天内容不贴屏幕边缘。
- 侧栏内联阈值：宽 > 120 内联（挤压主区），≤ 120 浮层覆盖右侧。
- 主区与侧栏之间 1 列背景间隔 `SIDEBAR_GAP = 1`。

## 2. 用户消息气泡（历史）

来源：`widgets/session.rs` 的 `render_bubble_row`；opencode `routes/session/index.tsx` 的 `UserMessage`。

```
[蓝线 ┃][灰面板背景] 文字……
 col0     col1+      col3
```

- 蓝线 `┃` 在 `area.x`（最左列），颜色 `theme.secondary`（agent 分类色，默认蓝）。
- 灰面板背景从蓝线右侧立刻开始，**没有空列**。
- 文字从蓝线后 2 列开始（`area.x + 2`）。
- 用户消息上下各留 1 行空的灰面板行（`pad_top/bottom = 1`）做呼吸。

## 3. 输入框 Prompt

来源：`widgets/prompt.rs` 的 `render_prompt`；opencode `component/prompt/index.tsx`。

```
[蓝线 ┃][灰面板背景] [pad2] 文字/光标……
 col0     col1+           col3
```

- 蓝线 `┃` 在 `area.x`，颜色 `theme.secondary`。
- 灰面板背景从 `col1` 紧贴蓝线开始（**中间不空列**）。
- 面板内 `paddingLeft = 2`，文字从 `col3` 开始。
- 面板内 `paddingTop = 1`、`paddingRight = 2`。
- 灰面板色 `theme.background_panel`；opencode 里外层 box 无背景、内层 box 有 `promptBg`，视觉等价于这里的手写填充。
- **注意**：曾误加过 col1 空列（终端背景），实际 opencode 没有——见 v0.5.43 修正。

## 4. 输入光标

来源：`widgets/prompt.rs` 的 `sync_cursor_style`。

- 亮帧：白色实心方块 `bg = Color::White`，fg 用 `theme.background_panel`（方块内字符反相可读）。
- 灭帧：`Style::default()`（不可见）。
- 闪烁由应用层节拍驱动（`CURSOR_BLINK_TICK = 600ms`），不用终端 SGR blink。
- 曾用 `theme.primary`（桃橙），v0.5.32 起改为白色。

## 5. 侧栏（右侧面板）

来源：`widgets/sidebar.rs`。

- 整体背景 `theme.background_panel`，无外边框。
- 顶部品牌行 `mrorcode`（primary 色加粗）。
- 三个可折叠分区，标题（中文）：**会话 / 上下文 / 文件**。
- Context 分区字段：`词元`（总量）/ `输入` / `输出` / `已压缩`（是/否）。
  - **不显示费用估算**——硬编码 $3/$15 每 M token 对豆包等 provider 不准，v0.5.35 已删 `estimate_usd`。
- 底部：`模型` / `目录` / `ctrl+p 命令 · ctrl+n 新会话`。
- 分区折叠状态按标题字符串存 KV；改标题名会一次性重置折叠状态（可接受）。

## 6. 权限弹窗

来源：`widgets/permission.rs`、`app.rs` 权限分派。

- 居中卡片，橙色边框 `theme.warning`。
- 标题 `Permission required` + `tool: <名>` + 命令详情。
- 竖向三个选项：`本次允许 / 始终允许 / 拒绝`，当前高亮项 `▶` + primary 色加粗。
- 按键：`↑/k`、`↓/j` 循环选择；`Enter` 确认；`Esc` 拒绝。
- 兼容快捷键：`y`=本次、`a`=始终、`n`=拒绝。
- 状态：`AppState.permission_cursor: usize`，新请求来时重置为 0。

## 7. 滚动与键位

来源：`keymap.rs`、`app.rs` apply_action。

- `PageUp / PageDown`：半屏翻聊天（随时生效）。
- `↑ / ↓`：输入框为空时滚聊天；有内容时交给 textarea 移光标（多行编辑不冲突）。
- 输入历史浏览：`HistoryPrev/Next` action 仍在枚举里，但默认不绑键（用户可自行配）。
- 不再默认绑 Ctrl+P/N（v0.5.38 起）。

## 8. 主题

来源：`theme.rs`、`build.rs`、`src/themes/`。

- **只内置一个主题：`opencode`**（`src/themes/opencode.json`）。
- `ThemeSet::bundled()` 不再加 `system` 主题（v0.5.32 起，`Theme::system()` 已删）。
- 用户可在 `~/.mrorcode/themes/*.json` 自加主题覆盖同名内置。
- 默认主题名 `opencode`；KV 里存了别的旧主题名会回退到 opencode。
- `Theme::opencode()`（Rust 内置构造器）是 JSON 解析失败的兜底，仍保留。

---

## 维护约定

- 视觉改动对齐 opencode 时：先在 `D:\opencode\packages\tui` 找到对应组件，记录源码位置再改 Rust 侧。
- 改完在本文档更新对应小节，并在版本历史里注明（Cargo.toml 版本号按 AGENTS.md 规则 bump）。
- 不要凭截图目测推断 opencode 布局——SolidJS 的 box 嵌套和 ratatui 的 Rect 嵌套映射容易错位。
