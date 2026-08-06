# myfin 策略规格（可执行规范）

> 状态：v1 · 参数定义：`config/screen.toml`（本文件唯一参数来源，本文引用的是当前默认值）。
> 理念背景：`docs/philosophy.md`。参数名与阈值以 `config/screen.toml` 为准，本文为解释性规格。

## 0. 总览

流水线六个阶段（与 `crates/mf-screener` 的 `PipelineStage` 枚举一致：
`universe → environment → undervalued → exclude_bad → recovery → output`）：

```
universe ──> environment ──> undervalued ──> exclude_bad ──> recovery ──> output
剔除池内不合格  环境归因标签     低估筛选          不良排除        回升确认       候选清单+报告
```

各阶段职责与顺序不可颠倒：先保证池子干净，再谈便宜，最后谈回升。
除 `output` 外任一阶段淘汰即出池，不进入后续阶段。

## 1. 阶段① universe：候选池构建

目标：剔除「不可能是策略对象」的股票，减少后续计算量并避免噪声。

规则（对应 `config/screen.toml` 的 `universe` 段）：

| 规则 | 参数 | 当前值 | 说明 |
| --- | --- | --- | --- |
| 排除北交所 | `universe.exclude_bse` | `true` | 免费数据源（mootdx/baostock/腾讯公网）缺 BJ 数据，见 `docs/data-sources.md` |
| 排除 ST/*ST | `universe.exclude_st` | `true` | 名称含 ST 或带退市风险旗标 |
| 上市最短年限 | `universe.min_list_years` | `1` | 上市不足 1 年无历史分位可算 |
| 市值下限 | `universe.min_market_cap` | `5.0e9`（50 亿） | 退市新规「市值退」阈值 5 亿，留 10 倍缓冲；同时规避小盘流动性风险 |
| 日均成交额下限 | `universe.min_avg_amount` | `5.0e7`（5000 万） | 剔除流动性不足，防滑点过大 |

## 2. 阶段② environment：环境扫描（数据化标签）

目标：区分「环境性低估 vs 资产性不良」（框架见 `docs/philosophy.md`），并为输出提供归因标签。

规则：

1. **标签必须由数据计算**：每行业计算「行业相对收益」（行业指数近 6–12 个月相对全市场超额收益）
   与「行业盈利趋势」（行业内公司 TTM 净利同比增长为正的公司占比等），生成结构化标签
   （如 `industry_relative_underperformed` / `industry_earnings_turning`）。
2. **agent 只润色不创造**：`data/context/` 下的背景文档可以由 agent 撰写，
   但任何「行业景气判断」「政策影响」表述必须引用于可查证的数据标签或外部事实，
   不得凭空生成数据点；标签的数值永远来自阶段②的计算，不来自 agent 文本。
3. 环境标签进入输出的 `env_tags` 字段（见 `crates/mf-report` 的 `Candidate.env_tags`）。

当前实现：`mfctl environment` 接收 `EnvironmentMember[]` JSON 和显式 `--as-of`，按行业输出
`EnvironmentSummary[]` 到 `data/context/environment-YYYY-MM-DD.json`。行业收益采用成员后复权收益
的等权均值与全体成员均值比较；最近 4 个可见报告期的净利润合计与前 4 期比较，均不足最小样本数时
只输出空指标，不生成标签。扫描结果可作为 `ScreenInput.environment` 或报告候选的
`Candidate.environment` 输入，`env_tags` 仍只用于展示，不改变筛选开关。

## 3. 阶段③ undervalued：低估筛选

目标：找到「当前低估」的标的。估值全部**本地自算**，不依赖任何第三方估值接口
（Tushare 免费档无 `daily_basic`，乐咕接口已停更，见 `docs/adr/0001`）。

### 3.1 估值口径（零成本自算）

| 指标 | 公式 | 说明 |
| --- | --- | --- |
| 市值 | 不复权收盘价 × 总股本 | 总股本来自 `PriceVal.total_shares`（行情派生数据） |
| PE（TTM） | 市值 / TTM 归母净利 | TTM = 最近 4 个报告期滚动；净利为负时 PE 无效，不参与分位 |
| PB | 市值 / 归母股东权益 | 权益为负时 PB 无效，直接落入不良排除（阶段④） |

- 财务数据按 **as-of 规则**对齐：只有 `ann_date <= as_of` 的财务快照参与计算；
  `ann_date` 为近似披露时点（报告期末 + `as_of.ann_date_approx_days` 天，当前 `60`）。
- 价格序列使用**后复权**口径（不复权价 × 复权因子本地换算，见 `docs/adr/0002`），
  杜绝多源前复权不一致带来的伪分位。

### 3.2 分位规则（参数 `undervalued` 段）

| 规则 | 参数 | 当前值 |
| --- | --- | --- |
| 分位窗口 | `undervalued.percentile_window_days` | `1250`（约 5 个交易日年，固定窗口） |
| 全市场分位阈值 | `undervalued.percentile_max` | `0.30`（PE 或 PB 任一 5 年分位 < 30%） |
| 行业内分位 | `undervalued.use_industry_percentile` | `true`（行业内分位也须 < 阈值，行业中性化） |

- **双分位**：全市场分位防止「绝对不便宜」，行业内分位防止整池集中于单一低估风格
  （如只选出被错杀的银行/地产），二者同时满足才入选。
- 输出时附「上市以来」分位对照（`percentile_window_days` 是固定 5 年窗，
  对上市不满 5 年者以实际可算天数覆盖，并标注）。
- 分位是滚动截面值：每个 as-of 日按该日之前 `percentile_window_days` 个交易日的估值序列计算，
  实现上只使用 as-of 日及之前可知的数据。

## 4. 阶段④ exclude_bad：不良排除

目标：把「资产性不良」挡在门外（参数 `exclusion` 段）。

| 规则 | 参数 | 当前值 | 说明 |
| --- | --- | --- | --- |
| 连续亏损季数上限 | `exclusion.max_consecutive_loss_quarters` | `2` | TTM 净利为负最多允许 2 个季度 |
| 经营现金流为负上限 | `exclusion.max_neg_cashflow_quarters` | `3` | 利润可能只是账面利润 |
| 资产负债率上限 | `exclusion.max_debt_ratio` | `0.70` | 高负债 + 低估值常见于风险暴露前夜 |
| 净资产 < 0 直接出池 | `exclusion.exclude_negative_equity` | `true` | 资不抵债 |

补充说明：

- **审计意见**：免费数据源（Baostock）无审计意见字段，`FinancialField` 枚举中也没有对应字段，
  无法机器化。审计意见保留为**人工复核项**，发现「非标意见」即列入风险旗标并提示。
- 阶段④与阶段③的顺序：先估值后排除，保证「便宜但烂」的标的确实被排除而不是被先入为主省略。

## 5. 阶段⑤ recovery：回升确认（右侧确认）

目标：确认「正在回升」。信号按以下**优先级**处理（主信号不足时辅信号不能越级替代）：

> 业绩拐点（主） > 分位回升 > 动量 / 均线 > 量能

| 优先级 | 信号 | 参数 | 当前值 | 规则 |
| --- | --- | --- | --- | --- |
| 主 | 业绩拐点 | `recovery.require_earnings_turnaround` | `true` | 最近一次业绩预告/快报（`EarningsNotice`）净利同比由负转正（`net_profit_yoy` 由负变正） |
| 辅1 | 分位回升 | `recovery.percent_3m_ago_max` | `0.20` | 当前分位 < `undervalued.percentile_max` 且 **3 个月前分位 < 0.20**（确认它确实曾更深低估） |
| 辅2 | 动量 | `recovery.momentum_days` | `63` | 近 63 个交易日（3 个月）收益 > 0 |
| 辅3 | 均线 | `recovery.ma_days` | `120` | 收盘价站上 120 日均线 |
| 辅4 | 量能 | `recovery.volume_ratio_min` | `1.2` | 近 20 日均成交额 / 前 60 日均成交额 ≥ 1.2 |

- 主信号（业绩拐点）**必须成立**：`require_earnings_turnaround=true`。
  依据：A 股业绩预告/快报披露远早于正式财报，且「净利同比转正」是基本面修复最直接的先行证据。
- 辅信号用于强度排序（如 4/4 辅信号全中的标的排序高于 2/4），不用于替代主信号。
- 业绩预告为区间时取区间中值（`EarningsNotice.net_profit` 的口径约定见 `docs/data-sources.md`）。

## 6. 阶段⑥ output：输出

每期输出（`mfctl report`，Markdown 落盘 `data/reports/`）：

1. **候选清单**：字段与 `crates/mf-report` 的 `Candidate` 一致——
   symbol/name/industry、全市场 PE/PB 分位（`pe_percentile`/`pb_percentile`）、
   行业内 PB 分位（`pb_industry_percentile`）、业绩拐点同比（`earnings_turnaround_yoy`）、
   3 个月收益（`momentum_3m_pct`）、环境归因标签（`env_tags`）、风险旗标（`risk_flags`）、入选理由。
2. **环境归因标签**：阶段②产出，说明该标的被低估的环境性原因。
3. **风险旗标**：见下节。
4. **数据质量页**：数据源健康状态（`SourceHealthLine`：ok/latency/error）与缺失、对账结果。

### 退市风险旗标规则（2025 退市新规）

对每个标的**实时计算**以下旗标（独立于阶段①~⑤，只要触发即进 `risk_flags`，供人工复核）：

| 旗标 | 触发条件 | 说明 |
| --- | --- | --- |
| 净资产为负 | 最新报告期归母股东权益 < 0 | 阶段④已出池；对持仓标的则立即标记 |
| 组合退 | 营收 < 3 亿 且 净利润为负 | 2025 退市新规「组合退」标准 |
| 市值退 | 市值 < 5 亿 | 阶段①以 50 亿为下限留 10 倍缓冲，实时监控 |
| 面值退 | 收盘价 < 1 元 | 面值退标准 |

## 7. 退出规则（持仓纪律）

1. **逻辑破坏退出（立即）**：财报证伪恢复预期——如预告/快报转正后的正式财报净利仍为负、
   出现非标审计意见、重大利空事件，恢复逻辑不再成立时立即退出，不等 6 个月。
2. **6 个月时间止损**：持有满 6 个月无条件退出。
3. **恢复信号消失（提前退出）**：回升信号消失——动量转负、跌破 MA120 或分位重新回落
   （具体以回升信号反转为准，参数与阶段⑤一致）。

## 8. 回测方法论

目标：在写任何实盘买入之前，先检验「6 个月收益预期」是否在历史上成立（`docs/adr/0003`）。

### 8.1 月度截面重建

- 自 2019-01 起，每月最后一个交易日的 as-of 点重建完整截面，逐日回放流水线六阶段；
- 每个截面只使用该 as-of 日及之前**已知**的数据（as-of 纪律，见 8.4）；
- 对入选标的按退出规则模拟 6 个月持有，记录区间收益。

### 8.2 分层检验

- **按年份分层**：2019–2026 逐年报告：入选数量、6 个月持有收益的中位数/均值、胜率、
  相对全市场基准的超额。
- 目的：检验策略是否仅靠某一年（如 2019 或 2024 单边行情）贡献收益。

### 8.3 阈值敏感性网格

| 维度 | 网格取值 | 当前默认 |
| --- | --- | --- |
| 分位阈值 | `percentile_max` ∈ {0.20, 0.30, 0.40} | 0.30 |
| 动量窗口 | `momentum_days` ∈ {63, 126}（3/6 个月） | 63 |
| 均线窗口 | `ma_days` ∈ {60, 120, 250} | 120 |

- **参数先验固定，不事后调参**：默认值即为 `config/screen.toml` 现值；
  网格只回答「结果对参数是否敏感」，若最优值偏离默认值不回溯更改默认（防过拟合）。

### 8.4 防前视偏差（as-of 纪律）

- 财务数据无公告日期，`ann_date` 按「报告期末 + 60 天」近似
  （`as_of.ann_date_approx_days = 60`，Q1≈4-30、H1≈8-31、Q3≈10-31、年报≈次年 4-30）；
- 分位、动量、均线等所有因子一律以 as-of 日的可用数据计算；
- 已知的 as-of 精度损失（公告日 ±60 天近似、业绩预告仅覆盖部分公司）须在报告的数据质量页声明。

## 9. 已知精度限制（必须声明）

1. **公告日近似**：财务 `ann_date` 为报告期末 + 60 天的近似，真实披露可能早于或晚于此点，
   回测与实盘均存在 as-of 精度损失；
2. **北向资金缺失**：日频北向数据 2024-08-19 起停止披露，任何资金面判断不得使用北向信号；
3. **乐咕接口停更**：历史 PE/PB 分位接口已停更，全部估值分位自算（`docs/adr/0001`）；
4. **复权口径**：多源前复权互相不一致，统一不复权 + 复权因子本地换算（`docs/adr/0002`）；
5. **审计意见不可得**：免费源无此字段，只能人工复核；
6. **业绩预告覆盖不全**：并非所有公司披露预告，`require_earnings_turnaround` 会系统性偏向
   披露习惯好的公司，回测中统计该偏差。
