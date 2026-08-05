# AGENTS.md — myfin 项目指南

个人量化选股仓库（A 股）。理念：在当前时点选「当前低估但正在回升」的标的，持有期 6 个月，不做市场择时；区分「环境性低估」与「资产性不良」。**全部免费数据源**。个人研究用途，非投资建议。

## 自然语言规范

文档、代码注释、日志及面向用户的文本，应使用准确、自然、符合中文技术语境的简体中文。

表达技术概念时，应先理解其在当前上下文中的实际职责，再选择能够准确传达其含义的中文，不得机械直译或为了中文化牺牲技术准确性。

需要精确对应代码、协议或外部接口时，可以保留原始名称；同一概念在项目中应保持统一称谓。

## 项目维护

对于需要长程维护的项目，需要有良好组织的文档。文档和代码一样需要维护，编写代码的同时需要更新文档。

改动要及时提交 git commit，commit 需要以任务、功能维度良好组织。动手开发之前就要设计好开发阶段以便于提交。

### git commit 规范

- 主题行：祈使语气（imperative mood）、简短；使用 Conventional Commits 英文前缀（`feat`/`fix`/`docs`/`chore`/`refactor`/`test` 等），标签与标题之间用英文冒号+空格分隔。
- 标题、正文使用中文；前缀标签保留英文。
- 正文：72 字符换行，说明**做了什么与为什么**，使用完整句子。

示例：

```
feat: 新增 tencent 行情适配器

实现腾讯财经日K拉取与实时快照解析，作为 daily 优先级链的备源，
在主源 mootdx 服务器失联时自动兜底。

fix: 修复 manifest 增量追加导致的历史覆盖

record() 原先直接重写整个文件，现在改为追加式 JSONL，
断点续跑不再丢失已完成日期的同步状态。
```

## Skills

项目级 skills 放在 `.agents/skills/`（随仓库分发）；个人全局 skills 放在 `~/.agents/skills`。

本仓库内置的数据源维护 skill 位于 `.agents/skills/data-source-maintenance/`；**维护数据源前必须先读该 skill**，它覆盖注册表修改、优先级链调整、健康检查与故障切换的完整流程，不必在 AGENTS.md 重复。

## 数据源与口径铁律

1. **统一 schema 以 `crates/mf-core/src/` 为准**（字段名与 Rust serde 定义严格一致，Python 侧对应 `schema.py`）。
2. **一律存不复权 OHLCV + 复权因子表**（`adj_factor`，后复权累计因子，主源 baostock）。禁止混用各源前复权序列；分位/收益计算本地后复权换算。
3. **零成本口径**：Tushare 免费档（120 积分）仅可用不复权日线 `daily`；`daily_basic`/财务/复权需 2000 积分，本项目**不购买**。估值分位 = Baostock 季频财务（净资产/EPS）+ 每日市值自算。
4. **禁用北向资金信号**（2024-08 起停披露）。恢复确认主信号 = 业绩预告/快报净利同比转正。
5. 财务无公告日期：`ann_date` = 报告期末 + 60 天近似（`as_of.ann_date_approx_days`）。所有因子计算遵守 as-of 纪律（只用 T 日及之前可知数据，防前视偏差）。
6. **MVP 排除北交所**（免费源不支持）。universe = 沪深主板 + 科创 + 创业。
7. 东财系接口 5 秒间隔起、akshare 锁版本、单源不得作为唯一依赖（2025 年 Tushare 曾宕机 5 天）。

## 常用命令（速查）

```bash
cargo build
cargo test
./target/debug/mfctl sources list
./target/debug/mfctl doctor
```

完整命令清单（含 worker 用法、py_compile 校验）见 `docs/architecture.md` 的「常用命令」章节；仓库结构见「仓库布局」章节；里程碑状态见「里程碑」章节。

## 约定

- Rust 优先；Python 只用于 Python SDK 独占数据源；TS 未启用。
- 代码不加冗余注释。
- 提交前跑 `cargo test`；新 py 文件跑 `python3 -m py_compile`。
- **永不提交密钥**：token 放环境变量（如 `TUSHARE_TOKEN`）或 `config/tokens.yaml`（gitignored），不硬编码、不入库。
