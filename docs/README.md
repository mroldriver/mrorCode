# docs 索引

本目录存放 mrorCode 的设计与规划文档。开发者上手请先看仓库根的 [AGENTS.md](../AGENTS.md)，
安装/用法/配置请看 [README.md](../README.md)。

| 文档 | 内容 | 状态 |
|---|---|---|
| [project-plan-atomcode.md](project-plan-atomcode.md) | 产品总体设计：定位、TUI 技术选型、6-crate 架构、功能对标清单、技术栈 | 现行（随实现维护） |
| [roadmap.md](roadmap.md) | M1–M4 实施计划与完成报告（M1–M4 存档），M5 硬化发布实施计划（a→e，待开工） | 混合（历史存档 + 进行中计划） |

## 维护约定

- **project-plan-atomcode.md** 描述「现在是什么样」，架构/选型/状态变化时同步更新。
- **roadmap.md** 中 M1–M4 是已完结里程碑的历史存档，保留原文与实测数据，不回改历史；
  新里程碑在末尾追加新章节（M5 为方向）。
- 文档为中文（与既有注释、设计文档保持一致）；相对链接必须可跳转。
- 项目结构、构建/测试命令、架构边界或开发约定变化时，同步更新根 [AGENTS.md](../AGENTS.md)。
