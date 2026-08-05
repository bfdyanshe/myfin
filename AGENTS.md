# AGENTS.md — myfin 项目指南

## 自然语言规范

- 文档、代码注释、日志及面向用户的文本使用准确、自然的简体中文。
- 技术概念按实际职责准确表述；涉及代码、协议或外部接口时可保留原始名称，并保持术语统一。

## 项目维护

长期维护项目需要同步维护文档，编写代码的同时更新相关文档。

任务完成或阶段性修改完成后及时提交 git commit；提交按任务、功能组织，开发前合理拆分阶段。

项目代码要及时格式化。

### git commit 规范

- 主题行：祈使语气（imperative mood）、简短；使用 Conventional Commits 英文前缀（`feat`/`fix`/`docs`/`chore`/`refactor`/`test` 等），标签与标题之间用英文冒号+空格分隔。
- 标题、正文使用中文；前缀标签保留英文。
- 正文：72 字符换行，说明**做了什么与为什么**，使用完整句子。


## Skills

项目级 skills 放在 `.agents/skills/`（随仓库分发）

## 约定

- Rust 优先；Python 只用于 Python SDK 独占数据源；TS 未启用。
- 代码不加冗余注释。
- 提交前跑 `cargo test`；新 py 文件跑 `python3 -m py_compile`。
- **永不提交密钥**：token 放环境变量（如 `TUSHARE_TOKEN`）或 `config/tokens.yaml`（gitignored），不硬编码、不入库。
- 优先使用 toml
