---
name: data-source-maintenance
description: 维护 myfin 的数据源注册表与适配器：新增/停用/修复数据源、调整优先级链、健康检查与故障切换。当用户要求添加数据源、数据源失效、调整数据源、跑 mfctl sources 相关命令时使用。
---

# 数据源维护

本 skill 是「AI 维护数据源」的操作手册：以 `config/sources.toml` 为唯一事实来源，配合 `mfctl` CLI 与 Python worker，完成数据源的登记、校验、健康检查、适配器实现与故障切换。全程无需 LLM API 集成。

## 何时使用

- 用户要求新增一个数据源（新的券商/行情/财务/宏观源）
- 某个数据源失效：数据缺失、超时、限流 429、接口改版、字段改名
- 需要调整优先级链（换主源、加兜底源、调整顺序）
- 需要修改字段映射口径或补充 notes 备注
- 用户运行 `mfctl sources list` / `mfctl sources check` / `mfctl doctor` 相关命令或询问数据源状态
- 需要停用/下线某个数据源

## 数据源生命周期总览

```
登记(改 config/sources.toml) → 校验(mfctl sources list) → 健康检查(mfctl sources check)
→ 适配器实现 → 增量同步(mfctl sync) → 故障切换(调整 chains / 自动 fallback)
```

每一步有明确的完成标准：登记后必须能通过解析；适配器实现后 check 探针必须返回数据；故障切换后对应数据集必须仍有多源兜底。

## 注册表字段速查表

顶层为 `version = 1`（必须是 1，否则校验失败）+ `sources` 列表 + `chains` 映射。

### source 条目

| 字段 | 必填 | 含义 | 取值/约定 |
|---|---|---|---|
| `name` | 是 | 源唯一标识 | 小写字母，如 `baostock`；链引用与适配器命名都依赖它 |
| `description` | 是 | 一句话能力描述 | 说明能提供哪些数据 |
| `kind` | 是 | 适配器类型 | `http`（Rust 原生适配器）/ `python_sdk`（Python worker 适配器） |
| `lang` | 是 | 实现语言 | `rust` / `python` |
| `package` | python_sdk 必填 | Python 适配器模块路径 | 如 `myfin_py.sources.baostock_source`；缺省会校验报错 |
| `auth` | 是 | 鉴权方式 | 仅两种：`type = "none"` 或 `type = "token"` + `env_var`（见下例）；env_var 不可为空 |
| `rate_limit` | 是 | 限流参数 | `min_interval_ms`（两次调用最小间隔）、`max_calls_per_minute`、`max_calls_per_day`（后两者可省略，TOML 无 null）、`backoff_ms`（被限流后退避时间） |
| `datasets` | 是 | 支持的数据集 | 从 6 类 Dataset 中选（见下） |
| `probe` | 否 | 健康检查探针 | `symbol`（基准股，各源自定格式）+ `lookback_days`（回看交易日数） |
| `notes` | 否 | 字段映射/口径备注 | AI 维护时更新的自由文本：字段映射、口径、坑位 |

### auth 两种形式（与 Rust 侧 `Auth` enum 的 serde tag 严格一致）

```toml
# 无需鉴权
auth = { type = "none" }

# token 鉴权：token 放 config/tokens.yaml 或环境变量，禁止硬编码/入库
auth = { type = "token", env_var = "TUSHARE_TOKEN" }
```

### rate_limit 建议值（参考现有源）

| 源类型 | min_interval_ms | 说明 |
|---|---|---|
| 通达信 TCP（mootdx） | 100 | 零鉴权、不封 IP |
| 通用免费库（baostock） | 300 | 匿名登录有限频 |
| 普通 HTTP（腾讯） | 500 | 低封 IP 风险 |
| 高限频免费档（Tushare） | 1200 | 另设 max_calls_per_minute = 50、max_calls_per_day = 8000 |
| 爬虫类/东财系（akshare） | 5000 | 至少 5 秒间隔，避免 403/封 IP |

### Dataset 六类（与 `crates/mf-datasource/src/dataset.rs` 的枚举一致）

| 值 | 含义 |
|---|---|
| `daily` | 不复权日 K（OHLCV） |
| `adj_factor` | 复权因子（后复权累计因子） |
| `financial` | 季频财务快照 |
| `earnings_notice` | 业绩预告/快报 |
| `price_val` | 行情派生（股本/市值计算用） |
| `macro` | 宏观指标（PMI/CPI/利率等） |

### chains 条目

```toml
[chains.daily]
dataset = "daily"                          # 必须与键一致，否则校验报错
order = ["mootdx", "tencent", "tushare"]   # 越靠前越优先；多源用于自动兜底
```

- 键与内部 `dataset` 字段必须一致；`order` 非空；`order` 中的源名必须已在 `sources` 中定义。
- 主源原则：稳定（零鉴权/低封 IP）；爬虫类（东财）仅兜底。

## 新增数据源的分步指令

① **确定数据集能力**：先确定新源能提供哪几类数据，对应 6 类 Dataset 枚举；不能提供的类别不要写进 `datasets`。

② **填 registry 条目**：在 `config/sources.toml` 的 `sources` 列表追加条目。auth 用上文的两种形式之一；rate_limit 按上表建议值给保守值；probe 基准股选**流通性最好的大票**（如贵州茅台 600519），symbol 格式遵循该源约定（baostock: `sh.600519`，tushare: `600519.SH`，mootdx: `600519`）。在 notes 中写明字段映射与口径。

③ **加/改 chains 优先级链**：为该源支持的数据集加入 `chains`；作为主源放在 `order` 最前，作为兜底放在靠后。同一数据集尽量配置多源，避免单点依赖。

④ **跑 `mfctl sources list` 确认解析通过**：注册表解析失败时检查常见错误：
   - **auth tag 格式错误**：必须精确写成 `auth = { type = "token", env_var = "XXX" }` 或 `auth = { type = "none" }`，多/少字段都会导致反序列化失败；
   - **chain 引用未定义源**：`优先级链引用了未定义的数据源: X`——补上 sources 条目或从 order 移除；
   - **python_sdk 缺 package**：`python_sdk 数据源 X 缺少 package 字段`；
   - **版本号错误**：`version` 必须是 1；
   - **chain 键与 dataset 不一致**：`chain 键 X 与内部 dataset 字段不一致`。

⑤ **实现适配器**：`python_sdk` 源在 `py/src/myfin_py/sources/` 下新建 `xxx_source.py`，实现 `fetch_*`（各数据集对应方法）与 `health_check()`（按 probe 拉基准股数据），模块路径填进 `package`。参考现有 baostock/akshare/mootdx 适配器的模式。`http` 源的 Rust 原生适配器属于后续里程碑，本轮只登记不实现。

⑥ **更新 `docs/data-sources.md`**：补充新源的说明、字段映射与维护注意事项。

⑦ **字段对齐统一 schema**：写入适配器前确认输出字段与 mf-core 统一 schema（FinancialField 等）一致；不一致时在 notes 中记录映射口径。

## 数据源故障处理流程

按症状对号入座：

- **数据缺失/返回空**：先用 `mfctl sources check` 看该源探针是否通过 → 通不过则检查上游接口是否改版 → 通得过则检查增量同步逻辑与数据目录（`mfctl doctor`）。
- **超时/连接失败**：检查网络与服务器可用性；mootdx 这类公共服务器轮换/失联是常态，按链 fallback 到下一源。
- **限流 429/403**：遵守该源 rate_limit；拉大 `min_interval_ms`、压低 `max_calls_per_*`；被限后按 `backoff_ms` 退避。东财系反爬严重，只能兜底，不能作主源。
- **接口改版/字段改名**：akshare 接口改名频繁——先锁版本（pyproject 固定），再按新接口改适配器，同时更新 notes 与 docs；mootdx 依赖活跃 fork，跟进 upstream 修复。

**调整优先级链做自动兜底**：把失效源从 `order` 前面挪后或补入新源，保证每个数据集至少两个可用源（有现实可行源的条件下）。**Tushare 宕机历史教训（2025-08 停运 5 天）→ 任何单源不得作为唯一依赖**；adj_factor/financial 目前只有 baostock 单源，是已知风险点，新增源时应优先补这两类。

## 字段映射铁律

- 行情必须输出**不复权** OHLCV；复权一律用 `adj_factor` 表本地换算（baostock 后复权累计因子 adjustflag=3 为唯一主源），禁止直接采用源自带的前复权数据（如腾讯 fqkline 前复权段）。
- 财务必须带 `ann_date`（公告日期）；免费源无公告日期时近似为**报告期末 + 60 天**，并在 notes 注明。
- 禁止使用北向资金信号（2024 年起已停止披露）。
- 东财系（akshare 相关接口）间隔 ≥ 5 秒。
- token 只允许放 `config/tokens.yaml` 或环境变量，禁止硬编码/入库。

## 反爬与限流纪律

- 每个源必须遵守自己的 `rate_limit`；新源一律从保守值起步。
- 失败采用指数退避（base 为 `backoff_ms`，重试 2-3 次后放弃，交给链上下一源）。
- 禁止暴力请求（并发、低间隔轮询）；不遵守纪律的源会被上游封禁，导致整个链路受损。

## 常用命令速查表

```bash
# 解析注册表并列出所有源与优先级链（改完 yaml 必跑）
./target/debug/mfctl sources list

# 健康检查：按 probe 探针拉取各源基准股数据（M3 落地后生效，当前提示未实现）
./target/debug/mfctl sources check

# 数据目录健康审计（数据缺失/未对齐检测）
./target/debug/mfctl doctor

# 增量同步数据（M3 实现）——后续里程碑
./target/debug/mfctl sync

# 运行选股流水线（M4 实现）——后续里程碑
./target/debug/mfctl screen

# 生成 Markdown 报告（M4/M5 实现）——后续里程碑
./target/debug/mfctl report

# 跨源抽查对账（M3/M4 实现）——后续里程碑
./target/debug/mfctl verify

# 历史月度截面重建回测（M4 实现）——后续里程碑
./target/debug/mfctl backtest

# Python worker（python_sdk 源的实际执行环境；worker 入口落地后补充）
# 占位：python -m myfin_py.sources.xxx_source ...
```

## 验证清单（新增/修改源后逐项确认）

- [ ] `mfctl sources list` 解析通过，且输出中新源与链顺序正确
- [ ] `mfctl sources check` 运行无报错（M3 落地后要求探针返回真实数据）
- [ ] probe 基准股为流通性最好的大票，symbol 格式符合该源约定
- [ ] 适配器 `fetch_*` 与 `health_check` 已实现并遵循统一 schema
- [ ] `docs/data-sources.md` 已更新
- [ ] 无硬编码 token；auth 正确声明 `type = "none"` 或 `type = "token"` + `env_var`
- [ ] 任何数据集不依赖单一源（有现实可行源时至少双源兜底）
