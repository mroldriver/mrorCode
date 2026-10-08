# 待办：TUI 与 opencode 对标差异

> 建档日期：2026-09-28
> 最后更新：2026-10-08——v0.5.11~v0.5.14 已落地 10 项，本文档同步标记完成状态并整理剩余优先级。
> 对标基线：`D:\gitcode\opencode` 中 `packages/tui` v1.18.31（SolidJS + @opentui 渲染栈）
> 对照对象：`crates/mrorcode-tui`（Rust + ratatui）
> 说明：并非所有差异都要补齐——标注「建议」的为值得跟进项，其余按里程碑规划取舍。
> 完成一项勾一项，并在 roadmap 记录归属里程碑。

## 已落地（v0.5.11 ~ v0.5.13）

| 功能 | 版本 | 提交 |
|------|------|------|
| 外部编辑器（`leader e`，`$VISUAL/$EDITOR`） | v0.5.13 | ce335b9 |
| token 分类显示（sidebar Context 区分 input/output） | v0.5.13 | ce335b9 |
| Home 页 footer 键位提示 | v0.5.13 | ce335b9 |
| export transcript 为 Markdown 文件 | v0.5.12 | e246fcf |
| 底部状态栏（`OC \| model \| dir`） | v0.5.12 | e246fcf |
| 侧栏宽度可配置（widen/narrow，持久化 KV） | v0.5.12 | e246fcf |
| 输入草稿 per-session stash（切会话保留/恢复） | v0.5.11 | 243aa22 |
| 会话 pin/unpin（pinned 排序置顶 + pin 标记） | v0.5.11 | 243aa22 |
| 用户主题目录（`~/.mrorcode/themes/*.json`，启动时加载覆盖） | v0.5.11 | 243aa22 |
| `/斜杠命令`提交时真正解析执行（映射到 AppCommand，未知命令 toast 报错） | v0.5.14 | — |
| textarea 完整 Emacs 行内键位（Ctrl+A/E/K/W/U/R、Alt+F/B/D 等，tui-textarea-2 原生） | v0.5.14 | — |

---

## 一、opencode 有、mrorCode 没有

### 子 agent / 会话体系

- [ ] 子 agent 实时子会话投影、点击进入子会话（`task` 现仅回灌最终文本）
- [ ] `SubagentFooter`：父子会话导航（right/left/up）、子会话 token 统计
- [ ] 后台子 agent（opencode `ctrl+b`，会话继续运行）
- [x] ~~会话 pin / 快速槽持久化~~（v0.5.11 已实现 pin；快速槽仍按当前列表顺序）
- [ ] 会话重命名（opencode `ctrl+r`）
- [ ] 会话 fork（从 timeline 分叉继续）
- [ ] 会话 share/unshare（生成分享 URL）
- [x] ~~export transcript 为 Markdown 文件（含 thinking/tool details 选项）~~（v0.5.12 已实现基础导出；thinking/tool details 选项待评估）
- [ ] timeline 对话框（消息级时间线操作）
- [ ] 排队提交 QUEUED：同一会话连续提交多条
- [ ] undo/redo 连带回滚文件改动（现 rewind 只截断消息、不动文件）

### 输入与补全

- [x] ~~【建议】`/斜杠命令`提交时真正解析执行~~（v0.5.14 已实现：提交时识别 `/command-name` 并执行对应 AppCommand，未知命令 toast 报错，忙碌中拒绝；覆盖全部 registry 命令）
- [ ] `@` 候选扩展：子 agent、MCP resources、目录别名、`file#L10` / `#L10-20` 行范围
- [ ] 补全候选 frecency 排序（现仅 fuzzy 分值）
- [ ] shell 模式：前缀输入直接执行 shell 命令
- [x] ~~草稿 stash：切走/清空时保留长草稿~~（v0.5.11 已实现 per-session stash）
- [x] ~~外部编辑器：`$VISUAL/$EDITOR` 打开输入框（opencode leader e）~~（v0.5.13 已实现）
- [ ] 编辑器联动：VS Code/Zed 当前打开文件与选区注入上下文
- [x] ~~【建议】textarea 完整 Emacs 行内键位~~（v0.5.14 验证通过：tui-textarea-2 原生支持 Ctrl+A/E/K/W/F/B/D/N/P/H/U/R/Y、Alt+F/B/D、Ctrl+方向键等；8 条集成测试覆盖核心键位；注意 Ctrl+N/P/C/X 及 Up/Down 为应用级键位，不作文本编辑用）

### 侧边栏

- [ ] 新增分区：MCP（连接状态）、LSP、Todo、Modified Files（git 改动 +/-计数，可折叠）
- [ ] 窄屏下侧栏全屏浮层（现宽 ≤100 直接隐藏；opencode 阈值 120、浮层底 alpha 70）

### Diff

- [ ] 独立 diff viewer 支持 git / branch / last-turn 三种数据源（现仅能从工具卡打开）
- [ ] 文件树侧栏 + hunk 跳转
- [ ] 宽屏内联 diff 自动 split（opencode >120 自动；现仅着色）

### 其他

- [ ] 图片 / PDF 剪贴板粘贴为附件
- [ ] 跨平台原生剪贴板路径（powershell Set-Clipboard / osascript / wl-copy / xclip，OSC52 之外的兜底；现状：OSC 52 + optional arboard feature）
- [ ] token 花费 USD 显示（v0.5.13 已有 input/output token 分类显示，但无 USD 换算）
- [ ] 声音提醒（完成/审批/错误音效）
- [ ] 失焦时系统通知（审批、完成）
- [ ] 终端标题动态更新（`OC | 标题`）
- [ ] 启动加载屏（v0.5.13 已有 Home 页 tips；独立启动加载屏待评估）
- [x] ~~用户主题目录发现（`.opencode/themes` 式：`~/.mrorcode/themes/*.json`）~~（v0.5.11 已实现）
- [ ] `system` 主题：从终端调色板自动生成
- [ ] SIGUSR2 主题热刷新
- [ ] 插件 / slot 扩展系统（opencode 核心扩展机制；mrorcode 已明确不做，列出备查）

---

## 二、mrorCode 有、opencode 没有（保持即可，勿回退）

- [x] 顶部会话标签栏（`widgets/tabs.rs`；opencode 明确无 tab bar）
- [x] 真正 push/pop 的对话框栈（opencode 仅 replace/clear 单对话框）
- [x] Help 对话框为完整键位表（opencode help 极简）
- [x] 底部状态栏（`OC | model | dir`，v0.5.12；opencode 无对应）
- [x] 侧栏宽度可配置（v0.5.12；opencode 固定宽度）

---

## 三、同名功能、语义/键位不同

| 项 | mrorCode 现状 | opencode | 状态 |
|---|---|---|---|
| F2 / Shift+F2 | 循环最近使用模型（正/反向，MRU 顺序持久化） | 循环最近 model（`model.json` 的 `recent`） | [x] 已对齐（0.5.4 / 0.5.5） |
| Tab / Shift+Tab | 循环 agent（build/plan） | 循环 agent | [x] 已对齐（0.5.4） |
| plan 模式进入 | Tab 手动 + 工具事件自动 | `plan_enter/exit` 工具自动切 | [x] 两种均支持（0.5.4） |
| toast | 右上角单条、5s、自动折行 | 右上角单条、5s | [x] 已对齐（0.5.4） |
| which-key | 默认关闭、ctrl+alt+k 开关、ctrl+alt+shift+k 切 dock/overlay | 同 | [x] 已对齐（0.5.4 / 0.5.5） |
| which-key 面板形态 | 高度 = 屏幕 30%（8..16）、按 category 分组页签、3 列、组内滚动、页脚提示 | 同 | [x] 已对齐（0.5.5） |
| 工具输出展开 | 展开显示全部（无行数上限） | 点击完整展开 | [x] 已对齐（0.5.4） |
| 侧栏键位 | leader b（原 F2） | leader b | [x] 已对齐（0.5.4） |
| 高亮引擎 | syntect（本地） | tree-sitter WASM（远程） | 无需跟进（无网依赖是优势） |
| markdown | pulldown-cmark（本地） | opentui 内置 | 无需跟进 |

> 备注：
> - Tab 改为 agent 循环后，工具卡聚焦不再有键盘快捷键，经鼠标点击聚焦。
> - which-key 由「leader 窗口内弹出」改为 opencode 的 pinned 语义：开关打开后常驻，
>   面板关闭时 `ctrl+alt+←/→/↑/↓/PgUp/PgDn/Home/End` 不生效。
> - 未采纳：pending 序列自动预览（opencode 默认关闭，依赖 mrorCode 没有的 pending 机制）、
>   分组页签鼠标点击（暂用键盘导航）。

### 已一致（无需改动）

- leader = ctrl+x，2s 超时
- Esc 双击中断（5s 窗口）
- ctrl+p 命令面板 / ctrl+n 新会话 / ctrl+l 会话列表
- 33 个主题 JSON、defs 引用、dark/light 双值

---

## 剩余功能优先级建议

### P0 — 体验缺口明显、实现成本低

1. ~~【建议】`/斜杠命令`真正解析执行~~（v0.5.14 已完成）
2. ~~【建议】textarea 完整 Emacs 行内键位~~（v0.5.14 已验证通过）

> P0 已全部完成。下一批建议从 P1 中选取。

### P1 — 对标 opencode 核心体验

3. **会话重命名**（`ctrl+r`）— 会话管理基础功能，实现成本低
4. **窄屏下侧栏全屏浮层** — 当前窄屏直接隐藏侧栏，opencode 用浮层兜底
5. **独立 diff viewer（git / branch / last-turn）** — 代码审查场景核心功能
6. **跨平台原生剪贴板路径** — OSC 52 在部分终端不支持，需要 powershell/osascript/wl-copy/xclip 兜底

### P2 — 增强功能，按需取舍

7. 子 agent 实时子会话投影（需改 kernel 核心 trait，架构改动大）
8. 后台子 agent（`ctrl+b`）
9. timeline 对话框（消息级时间线操作）
10. undo/redo 连带回滚文件改动
11. 图片 / PDF 剪贴板粘贴为附件
12. token 花费 USD 显示
13. 失焦时系统通知
14. `system` 主题（从终端调色板自动生成）

### P3 — 长尾 / 明确不做

15. 会话 fork / share-unshare
16. 排队提交 QUEUED
17. shell 模式
18. 编辑器联动（VS Code/Zed）
19. 声音提醒
20. 终端标题动态更新
21. 启动加载屏
22. SIGUSR2 主题热刷新
23. 插件 / slot 扩展系统（**明确不做**）
24. `@` 候选扩展（子 agent / MCP resources / 行范围）
25. 补全候选 frecency 排序
26. 侧边栏新增分区（MCP / LSP / Todo / Modified Files）
27. 宽屏内联 diff 自动 split
