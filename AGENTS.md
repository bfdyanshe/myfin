# AGENTS.md — myfin 项目指南

个人量化选股仓库（A 股）。理念：在当前时点选「当前低估但正在回升」的标的，持有期 6 个月，不做市场择时；区分「环境性低估」与「资产性不良」。**全部免费数据源**。个人研究用途，非投资建议。

## 常用命令

```bash
cargo build                              # 构建 mfctl
cargo test                               # 跑全部 Rust 测试
./target/debug/mfctl sources list        # 数据源注册表与优先级链
./target/debug/mfctl doctor              # 数据目录健康审计
./target/debug/mfctl sources check       # 源健康检查（M3 实现）
PYTHONPATH=py/src python3 -m myfin_py.worker list-sources
PYTHONPATH=py/src python3 -m myfin_py.worker fetch-daily --source baostock --symbol 600519.SH --start 2021-01-01 --end 2026-08-05 --out data/staging
```

## 仓库结构

- `crates/mf-core/` — 领域模型与**统一 schema**（bar/financial/symbol/valuation/error）。所有数据源产出必须规范化到此 schema。
- `crates/mf-datasource/` — 数据源注册表解析（registry.rs）、Source trait、优先级链。
- `crates/mf-storage/` — 数据目录布局 + 增量同步状态机（manifest）。Parquet+DuckDB 在 M2 引入。
- `crates/mf-screener/` — 选股流水线配置（config.rs，对应 config/screen.yaml）。流水线逻辑 M4 实现。
- `crates/mf-report/` — Markdown 报告渲染器。
- `crates/mfctl/` — CLI 入口（main.rs）。子命令：`sources list/check`、`sync`、`screen`、`report`、`doctor`、`verify`、`backtest`（后五者 M3/M4 落地）。
- `py/src/myfin_py/` — Python worker：baostock/akshare/mootdx 适配器（`sources/`）+ 统一 schema（`schema.py`）+ CLI（`worker.py`）。写 staging Parquet + `manifest.jsonl`，Rust 侧只读 manifest 编排。
- `config/sources.yaml` — **数据源注册表（AI 维护）**；`config/screen.yaml` — 筛选参数。
- `docs/` — philosophy/strategy/architecture/data-sources + `adr/`（架构决策）。
- `.opencode/skills/data-source-maintenance/SKILL.md` — **维护数据源必须先读**。

## 数据源与口径铁律

1. **统一 schema 以 `crates/mf-core/src/` 为准**（字段名与 Rust serde 定义严格一致，Python 侧对应 `schema.py`）。
2. **一律存不复权 OHLCV + 复权因子表**（`adj_factor`，后复权累计因子，主源 baostock）。禁止混用各源前复权序列；分位/收益计算本地后复权换算。
3. **零成本口径**：Tushare 免费档（120 积分）仅可用不复权日线 `daily`；`daily_basic`/财务/复权需 2000 积分，本项目**不购买**。估值分位 = Baostock 季频财务（净资产/EPS）+ 每日市值自算。
4. **禁用北向资金信号**（2024-08 起停披露）。恢复确认主信号 = 业绩预告/快报净利同比转正。
5. 财务无公告日期：`ann_date` = 报告期末 + 60 天近似（`as_of.ann_date_approx_days`）。所有因子计算遵守 as-of 纪律（只用 T 日及之前可知数据，防前视偏差）。
6. **MVP 排除北交所**（免费源不支持）。universe = 沪深主板 + 科创 + 创业。
7. 东财系接口 5 秒间隔起、akshare 锁版本、单源不得作为唯一依赖（2025 年 Tushare 曾宕机 5 天）。

## 修改数据源（常见任务）

1. 读 `.opencode/skills/data-source-maintenance/SKILL.md`。
2. 编辑 `config/sources.yaml`（注册条目 + 优先级链 `chains`）。
3. `cargo build && ./target/debug/mfctl sources list` 验证解析。
4. Python SDK 源改 `py/src/myfin_py/sources/<name>_source.py`；HTTP 源（tencent/tushare）适配器在 Rust 侧（M3）。
5. 同步更新 `docs/data-sources.md`。

## 里程碑状态

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| M1 | 脚手架：workspace、注册表、worker、docs、skill | ✅ 完成（fd92776） |
| M2 | Parquet 数据层 + DuckDB + 增量状态机落地 | 待做 |
| M3 | Rust HTTP 适配器（tencent/tushare）、sources check、sync、verify | 待做 |
| M4 | 估值分位、筛选流水线 screen、backtest（历史月度截面重建） | 待做 |
| M5 | report（Markdown + 数据质量页） | 待做 |
| M6 | 打磨 | 待做 |

## 约定

- Rust 优先；Python 只用于 Python SDK 独占数据源；TS 未启用。
- 代码不加冗余注释；中文文档。
- 提交前跑 `cargo test`；新 py 文件跑 `python3 -m py_compile`。
- **永不提交密钥**：token 放环境变量（如 `TUSHARE_TOKEN`）或 `config/tokens.yaml`（gitignored），不硬编码、不入库。
- 文档与代码必须一致：改配置/schema 后同步更新 `docs/`。
