# myfin

个人量化选股仓库：在当前时点选出「当前低估但正在回升」的 A 股标的，持有期 6 个月，不做市场择时；区分「环境性低估」与「资产性不良」。

## 投资理念

- 买入标的应同时满足两层条件：**低估**（估值分位处于历史低位）与**回升**（基本面或价格趋势出现反转信号），缺一不可。
- 持有期固定 6 个月，不做市场择时；买入决策只依赖标的自身的估值分位与复苏信号。
- 区分两类下跌：行业/市场整体拖累造成的**环境性低估**（可买），公司基本面恶化的**资产性不良**（回避）。
- 全流程基于免费数据源，可复现、可回测、可审计；估值分位本地自算，不依赖任何付费指标。
- 完整阐述见 [docs/philosophy.md](docs/philosophy.md)。

## 快速开始

```bash
# 本地仓库（当前无远端，也可直接在该目录操作）
git clone /Users/luozuojin/repo/myfin myfin && cd myfin

cargo build                                          # 构建 mfctl（需 Rust 1.85+，包含 DuckDB/Parquet）
./target/debug/mfctl sources list                    # 数据源注册表与优先级链
./target/debug/mfctl doctor                          # 数据目录健康审计
./target/debug/mfctl screen --symbol 600519.SH        # 从本地数据组装单标的筛选输入
```

Python 数据源使用 uv 管理。安装 uv 后，在仓库根目录执行：

```bash
uv sync --project py
uv run --project py python -c "import pandas; print(pandas.__version__)"
```

这会创建 `py/.venv` 并按 `py/pyproject.toml` 和 `py/uv.lock` 安装依赖。若 uv 不在 PATH，
可用 `python -m uv` 替代 `uv`。`mfctl` 在未设置 `MYFIN_PYTHON` 时会自动优先使用该虚拟环境。
仅 tushare 兜底源需要额外设置 `TUSHARE_TOKEN` 环境变量。

## CLI 子命令

状态与 crates/mfctl/src/main.rs 实际实现一致。

| 命令 | 用途 | 状态 |
| --- | --- | --- |
| `sources list` | 列出来源与优先级链 | 完成 |
| `sources check` | 健康检查（基准股探针） | M3（Python + Rust HTTP 已接入） |
| `sync` | 增量同步单源单标的数据 | M3（默认按优先级链自动切换；也可指定单源） |
| `screen` | 运行选股流水线 | M4（纯函数引擎 + 本地单标的数据组装） |
| `report` | 生成候选与数据质量 Markdown 报告 | M5（JSON 输入已接入） |
| `doctor` | 数据目录健康审计 | 完成 |
| `verify` | 跨源抽查对账 | M3 完成 |
| `backtest` | 历史月度截面重建回测 | M4（JSON 输入 + Markdown 报告） |

`verify` 读取本地 Parquet 中的同日多源记录，默认检查收盘价相对差异不超过 1%；
需要先用不同源同步同一标的，数据不足两源时会明确失败。

全局参数：`--registry <path>`（默认 `config/sources.toml`）、`--data-dir <path>`（默认 `$MYFIN_DATA` 或 `data/`）。

`screen` 默认从本地 Parquet 读取日线、股本、复权因子、财务和业绩公告；
若要提供全市场与行业估值样本，可使用 `--input <ScreenInput.json>` 传入完整筛选输入。
缺少这些样本时，命令会输出明确的淘汰结果，不会把单标的历史序列冒充横截面样本。

`backtest` 使用 `--input <HistoricalCandidate-array.json>` 读取带 as-of 横截面样本的历史候选输入，按每月最后交易日重建截面，
统计六个月持有收益、年度分层和 18 格敏感性网格，报告默认写入 `data/reports/backtest-YYYYMMDD.md`。

## 数据源

与 config/sources.toml 注册表一致（5 个源，全部免费）。Python SDK 适配器（baostock/akshare/mootdx）与 Rust HTTP 日线适配器（tencent/tushare）均已就位；Tushare 需要设置 `TUSHARE_TOKEN`。

| 源 | 角色 | 鉴权 | 数据集 | 优先级链 |
| --- | --- | --- | --- | --- |
| mootdx | 行情主源（通达信 TCP） | 无 | daily | daily: mootdx → tencent → tushare |
| tencent | 行情备源（HTTP） | 无 | daily | — |
| baostock | 财务/复权因子/业绩预告主源 | 无 | daily, adj_factor, financial, earnings_notice, price_val | financial / adj_factor: baostock |
| tushare | 行情校准/兜底（免费档） | `TUSHARE_TOKEN` | daily | earnings_notice: baostock → akshare |
| akshare | 宏观/新闻辅助 | 无 | macro, earnings_notice | macro: akshare |

注册表与优先级链由 AI 通过 config/sources.toml + [.agents/skills/data-source-maintenance/SKILL.md](.agents/skills/data-source-maintenance/SKILL.md) 维护。

## 目录结构

```
myfin/
├── Cargo.toml                        # workspace（6 crates）
├── crates/
│   ├── mf-core/                      # 领域模型：bar/financial/symbol/valuation/error
│   ├── mf-datasource/                # 数据源注册表：registry/source/dataset
│   ├── mf-storage/                   # 数据目录、Parquet/DuckDB：layout/parquet/sync
│   ├── mf-screener/                  # 筛选流水线：config/stage
│   ├── mf-report/                    # 报告生成（lib.rs）
│   └── mfctl/                        # CLI 入口：src/main.rs
├── py/
│   ├── pyproject.toml               # uv 管理（pandas/pyarrow/baostock/akshare/mootdx）
│   └── src/myfin_py/                # worker.py（CLI）+ sources/（3 个 Python 适配器）
├── config/
│   ├── sources.toml                 # 数据源注册表（版本 1）
│   └── screen.toml                  # 选股参数（universe/低估/排除/回升）
├── docs/                            # 策略、架构、数据源、审查与 ADR 文档
├── data/                            # 本地数据（gitignored）：market/{daily,adj_factor} financial macro sync reports context
└── .agents/skills/data-source-maintenance/   # AI 维护数据源的 skill（SKILL.md 已就位）
```

## 文档索引

- [docs/philosophy.md](docs/philosophy.md) —— 投资理念与「环境性低估 vs 资产性不良」分辨框架。
- [docs/strategy.md](docs/strategy.md) —— 策略规格：六阶段流水线、信号优先级、退出规则、回测方法论。
- [docs/strategy-review.md](docs/strategy-review.md) —— 量化策略审查：目标、实现偏差、风险问题与整改计划。
- [docs/architecture.md](docs/architecture.md) —— 架构：crate 职责、数据流、IPC 边界、增量同步状态机、质量门。
- [docs/data-sources.md](docs/data-sources.md) —— 数据源手册：统一 schema、注册表、维护流程（AI 维护数据源的核心参考）。
- [docs/adr/](docs/adr/) —— 架构决策记录：零成本数据、不复权+复权因子、回测先行。
- [config/screen.toml](config/screen.toml) —— 选股流水线可调参数。
- [.agents/skills/data-source-maintenance/SKILL.md](.agents/skills/data-source-maintenance/SKILL.md) —— AI 维护数据源的工作流（新增/停用/修复数据源、调整优先级链、故障切换）。

## 运行入口

全市场筛选需要点时股票池快照（`InstrumentSnapshot[]`）和已同步的全量 Parquet：

```bash
mfctl --data-dir data screen --all --as-of 2026-08-05 --universe data/universe/snapshots.json
```

历史回测输入必须为 `HistoricalCandidate[]`，每个输入同时提供 `trading_status`；缺少历史停牌/涨跌停
状态、真实公告日或同步质量通过标记时，`mfctl backtest` 会拒绝运行。

## 路线图

- **M1 脚手架（完成）**：workspace、领域模型、注册表解析、数据目录、CLI 骨架、sources list/doctor、Python worker 与 3 个 SDK 适配器骨架、数据源维护 skill、全套文档。
- **M2 存储层（完成）**：Parquet 数据层 + DuckDB 查询（SQLite 可选）、增量同步状态机。
- **M3 数据源适配器**：Rust HTTP 日线适配器、sources check、sync 故障切换与 verify 已完成。
- **M4 指标 + 筛选 + 回测（完成）**：筛选引擎支持全市场输入编排，估值使用不复权价格与点时股本，回测固定月末信号、下一交易日成交、等权/行业上限、成本后收益、样本外与消融。
- **M5 报告（完成）**：候选清单、风险旗标、行业环境归因、组合风险指标、样本外与消融结果、同步质量门均进入报告/CLI。
- **M6 打磨（完成）**：点时字段契约、财务累计值转换、真实公告日门禁、能力注册校验、manifest 严格解析、行数骤变检查、Rust/Python 格式与文档已收口。

## 已知限制

- Tushare 免费档无估值/财务数据（仅不复权日线），估值分位完全依赖本地自算。
- 北向资金自 2024 年起停止披露持股数据，相关因子不可用。
- 前复权统一由本地复权因子换算（baostock 为主源），不信任第三方前复权数据。
- 免费数据源缺少公告日期时严格流程会阻断；调试模式才允许带 `ann_date_is_approx` 的保守近似值。
- MVP 排除北交所（上游数据源对 BJ 支持不稳定）。

## 免责声明

本仓库仅用于个人学习与研究，不构成任何投资建议。项目可能包含错误；使用免费数据源存在接口失效、数据缺失与延迟风险，请自行核验。
