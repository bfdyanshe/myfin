# myfin 数据源手册

> 状态：v1 · 本文件是 **AI 维护数据源的核心参考**。
> 注册表配置：`config/sources.toml`（本文一切数值以此为唯一事实来源）·
> 维护技能：`.agents/skills/data-source-maintenance/` · 入口命令：`mfctl sources list / check`。
> 代码事实：`crates/mf-datasource/src/*.rs`（注册表结构）、`crates/mf-core/src/*.rs`（统一 schema）。

## 0. 设计原则

1. **全部免费**：项目坚持零成本数据方案（`docs/adr/0001`），不购买任何数据服务。
2. **主源稳定优先**：主源必须零鉴权、低封 IP 风险；爬虫类源（东财系）仅作兜底。
3. **统一 schema**：任何源的数据必须规范化为 `mf-core` 定义的模型后才能落库，
   字段命名即 canonical schema（下文 §3）。
4. **行情一律不复权**：存储不复权 OHLCV + 复权因子，本地换算后复权（`docs/adr/0002`），
   杜绝多源前复权口径不一致。
5. **注册表即事实**：数据源能力、限流、探针、优先级链全部声明在 `config/sources.toml`，
   修改后必须通过 `Registry::validate` 校验（`mfctl sources list` 可验证解析）。
6. **token 不入库**：任何 token 放 `config/tokens.yaml` 或环境变量（如 `TUSHARE_TOKEN`），
   `config/tokens.yaml` 已在 `.gitignore` 中。

## 1. 数据源清单（来自 `config/sources.toml`，5 个源）

| 源 | kind | lang | 鉴权 | min_interval | 上限 | backoff | 数据集 | 探针 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| baostock | python_sdk | python | 无 | 800 ms | 无 | 30 s | daily, adj_factor, financial, earnings_notice, price_val | `sh.600519` / 5 日 |
| tencent | http | rust | 无 | 500 ms | 无 | 15 s | daily | `sh600519` / 5 日 |
| tushare | http | rust | token（`TUSHARE_TOKEN`） | 1200 ms | 50/min、8000/day | 60 s | daily | `600519.SH` / 5 日 |
| akshare | python_sdk | python | 无 | 5000 ms | 无 | 120 s | macro, earnings_notice | 空 / 0 日 |
| mootdx | python_sdk | python | 无 | 100 ms | 无 | 10 s | daily | `600519` / 5 日 |

注：`probe` 的 `symbol` 各源格式不同（`sh.600519` / `sh600519` / `600519.SH` / `600519`），
是各源 SDK 自身约定的输入格式，统一化在适配器层完成（`Symbol::from_code` + `ts_code()`）。

### 1.1 baostock —— 财务/估值主源

- `kind = "python_sdk"`，包 `myfin_py.sources.baostock_source`；匿名登录，单次同步复用一个会话并显式 `bs.logout()`。
- 如上游要求账户登录，可通过环境变量 `BAOSTOCK_USER` / `BAOSTOCK_PASSWORD` 注入凭据；未设置时仍使用匿名登录，凭据不会写入配置或日志。
- 数据集：`daily, adj_factor, financial, earnings_notice, price_val`——**唯一**覆盖
  `adj_factor` 与 `financial` 的源，两条链都是它单源。
- 口径要点（`notes` 字段）：
  - 复权因子接口给出**后复权累计因子**（`adjustflag=3`），作为 `adj_factor` 唯一主源；
  - `daily` 输出不复权（`adjustflag=3`），后复权由存储层本地换算；
  - 财务字段映射以 Baostock 当前字段名为准：`MBRevenue/netProfit/epsTTM/gpMargin/roeAvg/
    liabilityToAsset` 映射到 `FinancialField`；`netProfit`、`MBRevenue` 已是元，`totalShare`、
    `liqaShare` 已是股数，不能再次乘 10000；
  - 当前 `balance` / `cash_flow` 接口主要返回比率字段，`equity`、`oper_cash_flow` 等金额字段可能缺失，
    这会触发严格选股质量门；业绩接口公告日和报告期使用 `profitForcastExpPubDate/StatDate`、
    `performanceExpPubDate/StatDate`；
  - 单连接非线程安全：worker 内串行，查询间隔至少 800 ms，并显式 `bs.logout()`。
- 已知限制：不含北交所；缺少 `pubDate` 的记录会标记 `ann_date_is_approx`，严格点时被阻断。
  - 连接或服务端不可用时，worker 返回对应 schema 的空结果并将任务记为 `skipped`；
    `sources check` 仍报告失败，便于恢复后复测。参数错误、字段错误等非连接问题仍会失败。
  - 登录错误码 `10001011` 表示服务端将当前访问来源列入黑名单；适配器不会循环重试，需通过
    [Baostock 登录页](https://baostock.com/login) 联系上游管理员处理。

### 1.2 tencent —— 行情备源（HTTP 零鉴权）

- `kind = "http"`（Rust 原生适配器）。`web.ifzq.gtimg.cn` 日 K（单次最多 640 根，需分页），
  `qt.gtimg.cn` 实时快照（含 PE/PB/市值，**GBK 编码**需 iconv 解码）。无财务。
- 口径要点：适配器使用 `day` 不复权段，分页上限按 640 根处理；长期分位以 baostock
  复权因子为准。腾讯该日线接口当前返回成交量但不返回成交额，统一 schema 中
  `amount` 暂置为 `0`，不能将其用于成交额分析。

### 1.3 tushare —— 校准/兜底源（免费档 120 积分）

- `kind = "http"`（Rust 原生适配器），token 鉴权，环境变量 `TUSHARE_TOKEN`。
- **免费档仅可用不复权日线 `daily` + 交易日历**；`daily_basic`/财务/复权均需 2000 积分
  （约 200 元/年），本仓库零成本方案不依赖（`docs/adr/0001`）。
- 硬限流：`min_interval_ms = 1200`（实际限流 50 次/分钟、8000 次/天，注册表如实声明）。
- 股票列表/交易日历等接口免费档不可用时，fallback 用 baostock/akshare。

### 1.4 akshare —— 宏观/新闻/辅助源

- `kind = "python_sdk"`，包 `myfin_py.sources.akshare_source`。
- 数据集：`macro, earnings_notice`（辅助）。
- 必须锁定 akshare 版本（pyproject 固定），接口改名频繁；东财系接口 5 秒间隔起
  （注册表 `min_interval_ms = 5000`）避免 403/封 IP；宏观源优先国家统计局/央行。

### 1.5 mootdx —— 行情主源（通达信 TCP）

- `kind = "python_sdk"`，包 `myfin_py.sources.mootdx_source`。
- 数据集：`daily`。版本由 `pyproject.toml` 与 `uv.lock` 固定；北交所仍在策略配置中排除。
- 上游公共服务器轮换/失联时按链 fallback 到 tencent/tushare；服务器列表需维护。
- 复权需本地 xdxr 除权除息数据，暂不作 adj 主源。

## 2. 优先级链（`config/sources.toml` 的 `chains`）

| 数据集 | 链（越靠前越优先） | 说明 |
| --- | --- | --- |
| daily | mootdx → tencent → tushare | 主源零鉴权，tushare 兜底 |
| adj_factor | baostock | 单源，后复权累计因子 |
| financial | baostock | 单源，季频财务 |
| earnings_notice | baostock → akshare | akshare 辅助补缺 |
| price_val | baostock | 历史股本与不复权收盘价（估值自算用） |
| macro | akshare | 单源，仅辅助 |

链被 `mf-datasource` 的 `Registry::validate` 校验（链键与内部 `dataset` 字段一致、

## 3. 统一 schema（来自 `crates/mf-core/src/*.rs`）

所有源产出的记录必须规范化到以下模型。字段名与类型以代码为准。

### 3.1 DailyBar（`bar.rs`）—— 不复权日 K

| 字段 | 类型 | 口径 |
| --- | --- | --- |
| `symbol` | String | 统一格式 `600519.SH`（`Symbol::ts_code`，代码 + 交易所后缀） |
| `trade_date` | NaiveDate | 交易日 |
| `open` / `high` / `low` / `close` | f64 | **不复权**价格 |
| `volume` | f64 | 成交量（手） |
| `amount` | f64 | 成交额（元） |
| `source` | String | 来源标识，如 `baostock` / `tencent` / `tushare` |

**口径约定（重要）**：存储层一律存不复权 OHLCV，复权因子单独成表；
所有分位/收益计算在本地用复权因子换算为后复权序列。禁止混用不同源的前复权序列。

### 3.2 AdjFactor（`bar.rs`）—— 复权因子

| 字段 | 类型 | 口径 |
| --- | --- | --- |
| `symbol` | String | `600519.SH` |
| `ex_date` | NaiveDate | 除权除息日（当日开始生效） |
| `cum_factor` | f64 | **后复权累计因子**：后复权价格 = 不复权价格 × `cum_factor` |
| `source` | String | 来源标识 |

### 3.3 FinancialData（`financial.rs`）—— 季频财务快照

| 字段 | 类型 | 口径 |
| --- | --- | --- |
| `symbol` | String | `600519.SH` |
| `report_period` | NaiveDate | 报告期（如 `2026-03-31`） |
| `ann_date` | NaiveDate | 优先使用来源 `pubDate`；只有 `ann_date_is_approx=true` 时才是报告期末 + 配置偏移的保守近似 |
| `ann_date_is_approx` | bool | 是否缺少真实公告日；严格点时流程会阻断 |
| `report_version` | Option\<String\> | 来源报告版本/统计期标识，用于修订值追溯 |
| `period_kind` | FinancialPeriodKind | 落库用于因子计算的值统一为 `single_quarter` |
| `raw_fields` | Vec\<(FinancialField, f64)\> | 来源原始值；Baostock 半年报/三季报的累计值在此保留 |
| `fields` | Vec\<(FinancialField, f64)\> | 转换后的单季财务字段键值对 |
| `source` | String | 来源标识 |

`FinancialField` 枚举字段（均为元口径，来源为 baostock 映射）：

| 枚举值 | 含义 |
| --- | --- |
| `Revenue` | 营业收入（元） |
| `NetProfit` | 归母净利润（元） |
| `Equity` | 归母股东权益（元） |
| `TotalAssets` | 总资产（元） |
| `TotalLiabilities` | 总负债（元） |
| `OperCashFlow` | 经营现金流净额（元） |
| `Eps` | 基本每股收益（元） |
| `Bps` | 每股净资产（元） |
| `GrossMargin` | 毛利率 |
| `Roe` | 净资产收益率 |
| `DebtRatio` | 资产负债率 |

### 3.4 EarningsNotice（`financial.rs`）—— 业绩预告/快报

| 字段 | 类型 | 口径 |
| --- | --- | --- |
| `symbol` | String | `600519.SH` |
| `ann_date` | NaiveDate | 预告/快报披露日 |
| `report_period` | NaiveDate | 对应报告期 |
| `kind` | NoticeKind | `Forecast`（预告）/ `Express`（快报） |
| `net_profit` | Option\<f64\> | 归母净利润（元），预告为区间取中值；可选 |
| `net_profit_yoy` | Option\<f64\> | 归母净利润同比（%）；可选 |
| `source` | String | 来源标识 |

### 3.5 PriceVal（`valuation.rs`）—— 行情派生价格数据（估值自算用）

| 字段 | 类型 | 口径 |
| --- | --- | --- |
| `symbol` | String | `600519.SH` |
| `trade_date` | NaiveDate | 交易日 |
| `close` | f64 | 不复权收盘价 |
| `total_shares` | f64 | 总股本（股） |
| `float_shares` | f64 | 流通股本（股） |
| `source` | String | 来源标识 |

### 3.6 派生：ValuationPoint（`valuation.rs`）

估值点在本地计算（非源产出）：`market_cap = close × total_shares`，
`pe_ttm = market_cap / TTM 归母净利`，`pb = market_cap / 归母股东权益`（均为 `Option<f64>`），
财务按 as-of 规则对齐（`ann_date` 过滤）。口径详见 `docs/strategy.md` §3.1。

### 3.7 Symbol 与市场分类（`symbol.rs`）

- `Symbol { code, exchange }`，`exchange ∈ {Sse, Szse, Bse}`，输出形如 `600519.SH`；
- 代码推断：`60/68/9` 开头 → 上交所（`688` 科创板）；`00/002/003/30` → 深交所（`300/301` 创业板）；
  `43/83/87/92` → 北交所；非 6 位代码返回 `None`。

### 3.8 点时股票池与交易状态（`universe.rs`）

`InstrumentSnapshot` 按 `effective_date` 保存名称、行业、ST、上市日、退市日、停牌和涨跌停信息；
同一标的允许多版本，`mfctl screen --all` 只选择 `effective_date <= as_of` 的最新记录。
`TradingStatus` 按交易日保存历史停牌和涨跌停状态，回测输入缺失该序列时由配置门禁拒绝运行，
禁止用当前状态回填历史。

## 4. 各源免费额度与限制表

| 源 | 免费额度 | 硬限制 | 主要失效风险 |
| --- | --- | --- | --- |
| baostock | 无收费档，匿名可用 | 有限频（注册表 800 ms 间隔） | 服务偶发不可用；财务无公告日期 |
| mootdx | 无（公共 TCP 服务器） | 上游服务器轮换/失联 | 服务器不可达；北交所 920 段旧版本错映射 |
| tencent | 无 | 单次 640 根需分页 | 字段格式变更；GBK 解码 |
| tushare | 120 积分免费档 | **50 次/分、8000 次/天**；仅不复权 daily | 曾连续宕机 5 天（见 §6）；积分权限收紧 |
| akshare | 无 | 无官方额度 | 接口改名频繁；东财系反爬升级 |

## 5. 数据源维护流程（AI 或人工）

1. **修改注册表**：编辑 `config/sources.toml`——新增源 / 调整优先级链 / 补字段映射（`notes`）；
   token 类鉴权声明 `env_var`，不写明文。
2. **`mfctl sources list`**：确认 YAML 解析通过、`Registry::validate` 校验通过
   （版本必须为 1；python_sdk 必须有 `package`；链引用的源必须已定义）。
3. **`mfctl sources check`**：按注册表逐源跑基准股探针（`probe.symbol` + `lookback_days`），
   输出健康报告（`HealthReport { source, ok, latency_ms, error }`）。Python 源通过
   worker 接入，腾讯/Tushare 通过 Rust HTTP 适配器接入；缺 token 或接口失败时明确报告，
   不会静默通过。
4. **更新适配器与本文档**：按新源/新字段同步适配器（python worker 或 Rust 原生），
   并更新 `docs/data-sources.md` 与本文件一致。

`mfctl sync --source auto` 会按对应数据集的 `chains` 顺序逐源尝试；
当前一个源失败时，失败信息保留在终端和 staging 目录，随后自动切换到下一个源。

维护者还须遵守：字段名以 `mf-core` 为准（§3）；行情一律不复权；
改链后跑 `sources list` 验证；新增源必须给出探针与限流配置。

## 6. 故障处理

- **某源失效时如何改链**：以 tushare 失效为例——
  1. 先确认失效范围：运行 `mfctl sources check` 看探针是否通过；
  2. 若为长期失效，编辑 `config/sources.toml` 将对应数据集链中该源下移或移除
     （如 `daily: [tencent, tushare]` → `[tencent]`），保留配置以利恢复后改回；
  3. 不改适配器代码的情况下优先利用优先级链自动兜底；确实需要换源补数据时再改适配器；
  4. 每次改链后运行 `mfctl sources list` 与 `sources check` 验证。
- **教训：Tushare 曾连续宕机约 5 天**：免费档服务不稳定是常态。
  应对：daily 链以 mootdx/tencent 为主（零鉴权），tushare 仅作校准/兜底；
  宕机期间缺口由 manifest 的 `failed` 状态记录，恢复后按 `missing_dates` 补拉。
- **东财系反爬现状（2025 年起升级）**：akshare 中东财系接口间歇性 403/封 IP，
  故 akshare 只承担 macro 与 earnings_notice 辅助，不作主源；
  调用间隔 5 秒起步（注册表 `min_interval_ms = 5000`），锁定版本防接口改名。
- **行情主源 mootdx 上游轮换**：通达信公共服务器列表需维护；
  连接失败按链 fallback 到 tencent，服务器恢复后 `sources check` 复测再回归。
- **Baostock 返回 10001011 或连接错误**：这是登录已到达服务端或传输层不可用，不是股票代码
  或本地字段错误。worker 会将本次 Baostock 数据记为 `skipped` 并保留空缺；`sources check`
  仍会失败。不要用代理轮换或并发重试规避封禁，恢复后先复测再补拉缺失数据。当前
  `earnings_notice` 有 AkShare 兜底，但 `adj_factor`、`financial`、`price_val` 仍是 Baostock
  单源，缺失期间质量门必须保持阻断，不能用不等价数据伪造补齐。

## 7. 已核实的数据可得性事实（写入项目决策）

1. Tushare 免费 120 积分仅可用不复权日线；`daily_basic`/财务/复权均需 2000 积分，
   本仓库不购买 → 估值分位用 Baostock 季频财务 + 市值自算（`docs/adr/0001`）。
2. 北向资金日频数据 2024-08-19 起停止披露，不得作为信号（`docs/strategy.md` §9）。
3. 乐咕 PE/PB 历史分位接口已停更，分位一律自算。
4. 各源前复权口径互相不一致 → 统一不复权 + 复权因子（`docs/adr/0002`）。
5. Baostock 财务接口若返回 `pubDate` 则直接采用；缺失时才按 `config/screen.toml` 的
   `as_of.ann_date_approx_days` 推算并设置 `ann_date_is_approx`，严格点时数据质量门会阻断。
6. Baostock 的半年报/三季报收入和净利润通常是年初至今累计值；适配器先保存原始值，
   再减去上一报告期累计值生成单季值，TTM 只由四个单季值构造。

## 8. 全市场截面构建器

`py/scripts/build_full_market_snapshot.py` 用于生成一个可复跑的全市场点时截面，
不改变 `mfctl sync` 的默认优先级链。它按当前总市值不低于 50 亿元、排除北交所的
股票建立股票池，并将缓存和摘要写入 `data/universe/YYYY-MM-DD/`：

```bash
PYTHONPATH=py/src py/.venv/bin/python \
  py/scripts/build_full_market_snapshot.py \
  --data-dir data --as-of 2026-08-06
```

批量口径如下：股票基础信息优先使用 Baostock；不复权日线使用 Baostock，按注册表
约定至少间隔 800 ms；新浪公开财务报告接口提供带真实公告日的季度财务快照；AKShare
批量接口提供业绩预告；申万历史分类文件提供行业代码。每只股票的原始结果保存在
`data/universe/YYYY-MM-DD/cache/`，因此网络中断后可重复执行并复用已完成数据。

该构建器遵循“单标的失败只留空数据”的约定，`summary.json` 会记录失败股票和警告。
批量构建暂不拉取全市场复权因子，报告必须明确标注技术指标使用不复权价格；行业分类
文件不可用时行业设为 `UNKNOWN`，此时行业分位只能解释为同一缺省组的截面分位，不能
当作真实行业比较。
