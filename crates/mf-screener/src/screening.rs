//! 六阶段选股规则的纯函数实现。

use std::collections::BTreeMap;

use chrono::{Duration, NaiveDate};
use mf_core::{
    AdjFactor, DailyBar, EarningsNotice, EnvironmentSummary, FinancialData, FinancialField,
    PriceVal, TradingStatus,
};
use serde::{Deserialize, Serialize};

use crate::metrics::{
    momentum, percentile_rank, simple_moving_average, trailing_twelve_months, volume_ratio,
};
use crate::ScreenerConfig;

/// 单标的筛选输入。跨标的样本由编排层准备，筛选器本身不访问文件或网络。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenInput {
    pub symbol: String,
    pub name: Option<String>,
    pub industry: Option<String>,
    pub is_st: bool,
    /// 输入是否来自带生效区间的历史股票池快照。
    #[serde(default)]
    pub point_in_time_complete: bool,
    /// 上市日，用于 as-of 股票池重建。
    #[serde(default)]
    pub listed_date: Option<NaiveDate>,
    /// 退市日（退市后不再进入股票池）。
    #[serde(default)]
    pub delisted_date: Option<NaiveDate>,
    /// as-of 日是否停牌。
    #[serde(default)]
    pub is_suspended: bool,
    /// 当日涨停价；用于回测成交阻断。
    #[serde(default)]
    pub price_limit_up: Option<f64>,
    /// 当日跌停价；用于回测成交阻断。
    #[serde(default)]
    pub price_limit_down: Option<f64>,
    /// 历史停牌/涨跌停状态；回测严格使用对应交易日的记录。
    #[serde(default)]
    pub trading_status: Vec<TradingStatus>,
    pub as_of: NaiveDate,
    pub bars: Vec<DailyBar>,
    pub price_vals: Vec<PriceVal>,
    pub adj_factors: Vec<AdjFactor>,
    pub financial: Vec<FinancialData>,
    pub earnings: Vec<EarningsNotice>,
    pub market_pe_samples: Vec<f64>,
    pub market_pb_samples: Vec<f64>,
    pub industry_pe_samples: Vec<f64>,
    pub industry_pb_samples: Vec<f64>,
    /// 可选的环境扫描结果；标签只用于输出和解释，不参与筛选开关。
    #[serde(default)]
    pub environment: Option<EnvironmentSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScreenMetrics {
    pub market_cap: Option<f64>,
    pub avg_amount: Option<f64>,
    pub pe_ttm: Option<f64>,
    pub pb: Option<f64>,
    pub pe_percentile: Option<f64>,
    pub pb_percentile: Option<f64>,
    pub pe_industry_percentile: Option<f64>,
    pub pb_industry_percentile: Option<f64>,
    pub earnings_turnaround_yoy: Option<f64>,
    pub momentum_3m: Option<f64>,
    pub ma120: Option<f64>,
    pub volume_ratio: Option<f64>,
    pub secondary_signal_count: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScreenResult {
    pub symbol: String,
    pub as_of: NaiveDate,
    pub passed: bool,
    pub stage: String,
    pub reason: Option<String>,
    pub metrics: ScreenMetrics,
    pub environment: Option<EnvironmentSummary>,
    pub risk_flags: Vec<String>,
}

#[derive(Debug, Clone)]
struct ValuationSample {
    date: NaiveDate,
    pe_ttm: Option<f64>,
    pb: Option<f64>,
}

/// 按策略阶段执行单标的筛选。
pub fn screen(input: &ScreenInput, config: &ScreenerConfig) -> ScreenResult {
    let mut result = screen_inner(input, config);
    result.symbol = input.symbol.clone();
    result.as_of = input.as_of;
    result.environment = input.environment.clone();
    result
}

/// 对同一 as-of 截面的全市场输入运行筛选。
///
/// 函数先从每个标的自身历史数据构造当日 PE/PB，再统一生成全市场和行业样本，
/// 最后调用单标的筛选器。这样调用方无需手工拼接分位样本，也不会把未来截面混入当前截面。
pub fn screen_universe(inputs: &[ScreenInput], config: &ScreenerConfig) -> Vec<ScreenResult> {
    let mut market_pe_samples = Vec::new();
    let mut market_pb_samples = Vec::new();
    let mut industry_samples = BTreeMap::<String, (Vec<f64>, Vec<f64>)>::new();
    for input in inputs {
        let current = valuation_samples(input)
            .into_iter()
            .rev()
            .find(|sample| sample.date <= input.as_of);
        if let Some(sample) = &current {
            if let Some(value) = sample.pe_ttm {
                market_pe_samples.push(value);
            }
            if let Some(value) = sample.pb {
                market_pb_samples.push(value);
            }
            if let Some(industry) = input.industry.as_deref().filter(|value| !value.is_empty()) {
                let entry = industry_samples.entry(industry.to_string()).or_default();
                if let Some(value) = sample.pe_ttm {
                    entry.0.push(value);
                }
                if let Some(value) = sample.pb {
                    entry.1.push(value);
                }
            }
        }
    }

    inputs
        .iter()
        .map(|input| {
            let mut input = input.clone();
            input.market_pe_samples = market_pe_samples.clone();
            input.market_pb_samples = market_pb_samples.clone();
            if let Some(industry) = input.industry.as_deref() {
                if let Some((pe, pb)) = industry_samples.get(industry) {
                    input.industry_pe_samples = pe.clone();
                    input.industry_pb_samples = pb.clone();
                }
            }
            screen(&input, config)
        })
        .collect()
}

fn screen_inner(input: &ScreenInput, config: &ScreenerConfig) -> ScreenResult {
    let empty_metrics = ScreenMetrics {
        market_cap: None,
        avg_amount: None,
        pe_ttm: None,
        pb: None,
        pe_percentile: None,
        pb_percentile: None,
        pe_industry_percentile: None,
        pb_industry_percentile: None,
        earnings_turnaround_yoy: None,
        momentum_3m: None,
        ma120: None,
        volume_ratio: None,
        secondary_signal_count: 0,
    };
    let mut bars_by_date = BTreeMap::<NaiveDate, &DailyBar>::new();
    for bar in input
        .bars
        .iter()
        .filter(|bar| bar.symbol == input.symbol && bar.trade_date <= input.as_of)
    {
        bars_by_date
            .entry(bar.trade_date)
            .and_modify(|current| {
                if source_priority(&bar.source) < source_priority(&current.source) {
                    *current = bar;
                }
            })
            .or_insert(bar);
    }
    let bars = bars_by_date.into_values().collect::<Vec<_>>();
    let Some(latest_bar) = bars.last().copied() else {
        return rejected("universe", "缺少 as-of 日线数据", empty_metrics);
    };
    if config.universe.require_point_in_time && !input.point_in_time_complete {
        return rejected(
            "universe",
            "缺少点时股票池快照，不能用于全市场或历史筛选",
            empty_metrics,
        );
    }
    if input
        .listed_date
        .is_some_and(|listed_date| listed_date > input.as_of)
        || input
            .delisted_date
            .is_some_and(|delisted_date| delisted_date <= input.as_of)
    {
        return rejected("universe", "标的未上市或已在 as-of 日前退市", empty_metrics);
    }
    if input.is_suspended {
        return rejected(
            "universe",
            "as-of 日停牌，不能作为新买入标的",
            empty_metrics,
        );
    }
    if config.universe.exclude_bse && input.symbol.ends_with(".BJ") {
        return rejected("universe", "北交所标的被配置为排除", empty_metrics);
    }
    if config.universe.exclude_st && input.is_st {
        return rejected("universe", "ST 标的被配置为排除", empty_metrics);
    }

    let amounts = bars.iter().map(|bar| bar.amount).collect::<Vec<_>>();
    let Some(avg_amount) = simple_moving_average(&amounts, 20) else {
        return rejected(
            "universe",
            "日线不足 20 个交易日，无法检查流动性",
            empty_metrics,
        );
    };
    if avg_amount < config.universe.min_avg_amount {
        return rejected(
            "universe",
            &format!(
                "近 20 日均成交额 {:.2} 低于 {:.2}",
                avg_amount, config.universe.min_avg_amount
            ),
            ScreenMetrics {
                avg_amount: Some(avg_amount),
                ..empty_metrics
            },
        );
    }

    let Some(price) = latest_price_val(input, input.as_of) else {
        return rejected("universe", "缺少 as-of 股本与收盘价数据", empty_metrics);
    };
    let market_cap = price.close * price.total_shares;
    if !market_cap.is_finite() || market_cap < config.universe.min_market_cap {
        return rejected(
            "universe",
            &format!(
                "市值 {:.2} 低于 {:.2}",
                market_cap, config.universe.min_market_cap
            ),
            ScreenMetrics {
                market_cap: Some(market_cap),
                avg_amount: Some(avg_amount),
                ..empty_metrics
            },
        );
    }
    if !has_minimum_history(&bars, input.as_of, config.universe.min_list_years) {
        return rejected(
            "universe",
            "历史数据不足配置的最短上市年限",
            ScreenMetrics {
                market_cap: Some(market_cap),
                avg_amount: Some(avg_amount),
                ..empty_metrics
            },
        );
    }

    let valuations = valuation_samples(input);
    let Some(current_index) = valuations
        .iter()
        .rposition(|sample| sample.date <= input.as_of)
    else {
        return rejected(
            "undervalued",
            "缺少可用的财务与股本数据，无法计算估值",
            ScreenMetrics {
                market_cap: Some(market_cap),
                avg_amount: Some(avg_amount),
                ..empty_metrics
            },
        );
    };
    let current = &valuations[current_index];
    let window = config.undervalued.percentile_window_days as usize;
    let history_pe_percentile =
        percentile_at(&valuations, current_index, window, |sample| sample.pe_ttm);
    let history_pb_percentile =
        percentile_at(&valuations, current_index, window, |sample| sample.pb);
    if current.pe_ttm.is_none() && current.pb.is_none() {
        return rejected(
            "undervalued",
            "当前 PE 与 PB 均不可用",
            ScreenMetrics {
                market_cap: Some(market_cap),
                avg_amount: Some(avg_amount),
                ..empty_metrics
            },
        );
    }
    if history_pe_percentile.is_none() && history_pb_percentile.is_none() {
        return rejected(
            "undervalued",
            "缺少历史估值数据，无法计算当前分位",
            ScreenMetrics {
                market_cap: Some(market_cap),
                avg_amount: Some(avg_amount),
                pe_ttm: current.pe_ttm,
                pb: current.pb,
                ..empty_metrics
            },
        );
    };
    let pe_percentile = current
        .pe_ttm
        .and_then(|value| percentile_rank(value, &input.market_pe_samples));
    let pb_percentile = current
        .pb
        .and_then(|value| percentile_rank(value, &input.market_pb_samples));
    let pe_industry_percentile = current
        .pe_ttm
        .and_then(|value| percentile_rank(value, &input.industry_pe_samples));
    let pb_industry_percentile = current
        .pb
        .and_then(|value| percentile_rank(value, &input.industry_pb_samples));
    let absolute_undervalued = [pe_percentile, pb_percentile]
        .into_iter()
        .flatten()
        .any(|value| value < config.undervalued.percentile_max);
    let industry_undervalued = if config.undervalued.use_industry_percentile {
        [pe_industry_percentile, pb_industry_percentile]
            .into_iter()
            .flatten()
            .any(|value| value < config.undervalued.percentile_max)
    } else {
        true
    };
    if !absolute_undervalued {
        return rejected(
            "undervalued",
            "PE/PB 全市场分位均未低于阈值",
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    }
    if !industry_undervalued {
        return rejected(
            "undervalued",
            "PE/PB 行业内分位均未低于阈值或行业样本缺失",
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    }

    let periods = financial_periods(&input.financial, input.as_of);
    let Some(latest_financial) = periods.last().copied() else {
        return rejected(
            "exclude_bad",
            "缺少 as-of 财务快照",
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    };
    if config.as_of.require_real_ann_date && periods.iter().any(|record| record.ann_date_is_approx)
    {
        return rejected(
            "exclude_bad",
            "可见财务快照缺少真实公告日，严格点时流程已阻断",
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    }
    let loss_streak = consecutive_negative(&periods, FinancialField::NetProfit);
    if loss_streak > config.exclusion.max_consecutive_loss_quarters {
        return rejected(
            "exclude_bad",
            &format!("连续亏损 {} 个报告期", loss_streak),
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    }
    let negative_cashflow = periods
        .iter()
        .rev()
        .take(4)
        .filter(|record| {
            record
                .get(FinancialField::OperCashFlow)
                .is_some_and(|v| v < 0.0)
        })
        .count() as u32;
    if config.exclusion.require_oper_cash_flow
        && periods
            .iter()
            .rev()
            .take(4)
            .any(|record| record.get(FinancialField::OperCashFlow).is_none())
    {
        return rejected(
            "exclude_bad",
            "最近 4 个报告期经营现金流字段不完整，质量门阻断",
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    }
    if negative_cashflow > config.exclusion.max_neg_cashflow_quarters {
        return rejected(
            "exclude_bad",
            &format!("最近 4 个报告期有 {} 个经营现金流为负", negative_cashflow),
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    }
    let equity = latest_financial.get(FinancialField::Equity);
    if config.exclusion.exclude_negative_equity && equity.is_some_and(|value| value < 0.0) {
        return rejected(
            "exclude_bad",
            "最新归母权益为负",
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    }
    let debt_limit = input
        .industry
        .as_deref()
        .and_then(|industry| config.exclusion.industry_debt_ratio.get(industry))
        .copied()
        .unwrap_or(config.exclusion.max_debt_ratio);
    if latest_financial
        .get(FinancialField::DebtRatio)
        .is_some_and(|value| value > debt_limit)
    {
        return rejected(
            "exclude_bad",
            &format!("资产负债率超过行业阈值 {:.1}%", debt_limit * 100.0),
            metrics(
                market_cap,
                avg_amount,
                current,
                pe_percentile,
                pb_percentile,
                pe_industry_percentile,
                pb_industry_percentile,
                None,
                &bars,
                input,
                0,
                config,
            ),
        );
    }

    let earnings_turnaround_yoy = earnings_turnaround(&input.earnings, input.as_of);
    let mut result_metrics = metrics(
        market_cap,
        avg_amount,
        current,
        pe_percentile,
        pb_percentile,
        pe_industry_percentile,
        pb_industry_percentile,
        earnings_turnaround_yoy,
        &bars,
        input,
        0,
        config,
    );
    let momentum_days = config.recovery.momentum_days as usize;
    let previous_percentile_ok = current_index >= momentum_days
        && [
            percentile_at(
                &valuations,
                current_index - momentum_days,
                window,
                |sample| sample.pe_ttm,
            ),
            percentile_at(
                &valuations,
                current_index - momentum_days,
                window,
                |sample| sample.pb,
            ),
        ]
        .into_iter()
        .flatten()
        .any(|value| value < config.recovery.percent_3m_ago_max);
    let current_percentile_ok = [pe_percentile, pb_percentile]
        .into_iter()
        .flatten()
        .any(|value| value < config.undervalued.percentile_max);
    let momentum_ok = result_metrics.momentum_3m.is_some_and(|value| value > 0.0);
    let ma_ok = result_metrics
        .ma120
        .is_some_and(|value| latest_bar.close > value);
    let volume_ok = result_metrics
        .volume_ratio
        .is_some_and(|value| value >= config.recovery.volume_ratio_min);
    result_metrics.secondary_signal_count = [
        previous_percentile_ok && current_percentile_ok,
        momentum_ok,
        ma_ok,
        volume_ok,
    ]
    .into_iter()
    .filter(|passed| *passed)
    .count() as u8;
    if config.recovery.require_earnings_turnaround && earnings_turnaround_yoy.is_none() {
        return rejected(
            "recovery",
            "缺少业绩同比由负转正的预告/快报信号",
            result_metrics,
        );
    }
    if result_metrics.secondary_signal_count < config.recovery.min_secondary_signals {
        return rejected(
            "recovery",
            &format!(
                "辅信号仅 {} 个，低于要求的 {} 个",
                result_metrics.secondary_signal_count, config.recovery.min_secondary_signals
            ),
            result_metrics,
        );
    }
    ScreenResult {
        symbol: input.symbol.clone(),
        as_of: input.as_of,
        passed: true,
        stage: "output".to_string(),
        reason: None,
        metrics: result_metrics,
        environment: None,
        risk_flags: risk_flags(market_cap, latest_financial, latest_bar.close),
    }
}

fn metrics(
    market_cap: f64,
    avg_amount: f64,
    current: &ValuationSample,
    pe_percentile: Option<f64>,
    pb_percentile: Option<f64>,
    pe_industry_percentile: Option<f64>,
    pb_industry_percentile: Option<f64>,
    earnings_turnaround_yoy: Option<f64>,
    bars: &[&DailyBar],
    input: &ScreenInput,
    secondary_signal_count: u8,
    config: &ScreenerConfig,
) -> ScreenMetrics {
    let closes = bars
        .iter()
        .map(|bar| adjusted_close(bar, &input.adj_factors))
        .collect::<Vec<_>>();
    let amounts = bars.iter().map(|bar| bar.amount).collect::<Vec<_>>();
    ScreenMetrics {
        market_cap: Some(market_cap),
        avg_amount: Some(avg_amount),
        pe_ttm: current.pe_ttm,
        pb: current.pb,
        pe_percentile,
        pb_percentile,
        pe_industry_percentile,
        pb_industry_percentile,
        earnings_turnaround_yoy,
        momentum_3m: momentum(&closes, config.recovery.momentum_days as usize)
            .map(|value| value * 100.0),
        ma120: simple_moving_average(&closes, config.recovery.ma_days as usize),
        volume_ratio: volume_ratio(&amounts, 20, 60),
        secondary_signal_count,
    }
}

fn rejected(stage: &str, reason: &str, metrics: ScreenMetrics) -> ScreenResult {
    ScreenResult {
        symbol: String::new(),
        as_of: NaiveDate::MIN,
        passed: false,
        stage: stage.to_string(),
        reason: Some(reason.to_string()),
        metrics,
        environment: None,
        risk_flags: Vec::new(),
    }
}

fn risk_flags(market_cap: f64, financial: &FinancialData, close: f64) -> Vec<String> {
    let mut flags = Vec::new();
    let equity = financial.get(FinancialField::Equity);
    let revenue = financial.get(FinancialField::Revenue);
    let net_profit = financial.get(FinancialField::NetProfit);
    if equity.is_some_and(|value| value < 0.0) {
        flags.push("negative_equity".to_string());
    }
    if revenue.is_some_and(|value| value < 3.0e8) && net_profit.is_some_and(|value| value < 0.0) {
        flags.push("delisting_combined_rule".to_string());
    }
    if market_cap < 5.0e8 {
        flags.push("market_cap_under_500m".to_string());
    }
    if close < 1.0 {
        flags.push("price_under_1".to_string());
    }
    flags
}

fn latest_price_val(input: &ScreenInput, as_of: NaiveDate) -> Option<&PriceVal> {
    input
        .price_vals
        .iter()
        .filter(|price| price.symbol == input.symbol && price.trade_date <= as_of)
        .max_by(|left, right| {
            left.trade_date
                .cmp(&right.trade_date)
                .then_with(|| source_priority(&right.source).cmp(&source_priority(&left.source)))
        })
}

fn has_minimum_history(bars: &[&DailyBar], as_of: NaiveDate, min_years: u32) -> bool {
    let Some(first) = bars.first() else {
        return false;
    };
    first.trade_date + Duration::days(i64::from(min_years) * 365) <= as_of
}

fn valuation_samples(input: &ScreenInput) -> Vec<ValuationSample> {
    let mut prices_by_date = BTreeMap::<NaiveDate, &PriceVal>::new();
    for price in input
        .price_vals
        .iter()
        .filter(|price| price.symbol == input.symbol && price.trade_date <= input.as_of)
    {
        prices_by_date
            .entry(price.trade_date)
            .and_modify(|current| {
                if source_priority(&price.source) < source_priority(&current.source) {
                    *current = price;
                }
            })
            .or_insert(price);
    }
    prices_by_date
        .into_values()
        .filter_map(|price| {
            let periods = financial_periods(&input.financial, price.trade_date);
            let net_profits = periods
                .iter()
                .filter_map(|record| record.get(FinancialField::NetProfit))
                .collect::<Vec<_>>();
            let ttm = trailing_twelve_months(&net_profits);
            let equity = periods
                .last()
                .and_then(|record| record.get(FinancialField::Equity));
            // 历史估值使用当日不复权收盘价；复权价格只用于收益与技术指标。
            let market_cap = price.close * price.total_shares;
            Some(ValuationSample {
                date: price.trade_date,
                pe_ttm: ttm
                    .filter(|value| *value > 0.0)
                    .map(|value| market_cap / value),
                pb: equity
                    .filter(|value| *value > 0.0)
                    .map(|value| market_cap / value),
            })
        })
        .collect()
}

fn financial_periods<'a>(
    financial: &'a [FinancialData],
    as_of: NaiveDate,
) -> Vec<&'a FinancialData> {
    let mut by_period = BTreeMap::new();
    for record in financial
        .iter()
        .filter(|record| record.ann_date <= as_of && record.report_period <= as_of)
    {
        by_period
            .entry(record.report_period)
            .and_modify(|current: &mut &FinancialData| {
                if record.ann_date > current.ann_date
                    || (record.ann_date == current.ann_date
                        && source_priority(&record.source) < source_priority(&current.source))
                {
                    *current = record;
                }
            })
            .or_insert(record);
    }
    by_period.into_values().collect()
}

fn consecutive_negative(records: &[&FinancialData], field: FinancialField) -> u32 {
    records
        .iter()
        .rev()
        .take_while(|record| record.get(field).is_some_and(|value| value < 0.0))
        .count() as u32
}

fn earnings_turnaround(notices: &[EarningsNotice], as_of: NaiveDate) -> Option<f64> {
    let mut notices = notices
        .iter()
        .filter(|notice| notice.ann_date <= as_of)
        .collect::<Vec<_>>();
    notices.sort_by_key(|notice| notice.ann_date);
    let latest = notices.last()?;
    let previous = notices
        .iter()
        .rev()
        .skip(1)
        .find_map(|notice| notice.net_profit_yoy)?;
    let latest_yoy = latest.net_profit_yoy?;
    (previous < 0.0 && latest_yoy > 0.0).then_some(latest_yoy)
}

fn percentile_at<F>(
    samples: &[ValuationSample],
    index: usize,
    window: usize,
    value: F,
) -> Option<f64>
where
    F: Fn(&ValuationSample) -> Option<f64>,
{
    let current = value(samples.get(index)?)?;
    let start = index.saturating_add(1).saturating_sub(window.max(1));
    let history = samples[start..=index]
        .iter()
        .filter_map(value)
        .collect::<Vec<_>>();
    percentile_rank(current, &history)
}

fn adjusted_close(bar: &DailyBar, factors: &[AdjFactor]) -> f64 {
    let factor = factors
        .iter()
        .filter(|factor| factor.symbol == bar.symbol && factor.ex_date <= bar.trade_date)
        .max_by_key(|factor| factor.ex_date)
        .map(|factor| factor.cum_factor)
        .unwrap_or(1.0);
    bar.close * factor
}

fn source_priority(source: &str) -> u8 {
    match source {
        "mootdx" => 0,
        "tencent" => 1,
        "tushare" => 2,
        "baostock" => 3,
        "akshare" => 4,
        _ => 100,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_daily_data_without_losing_identity() {
        let input = ScreenInput {
            symbol: "600519.SH".to_string(),
            name: None,
            industry: None,
            is_st: false,
            point_in_time_complete: true,
            listed_date: None,
            delisted_date: None,
            is_suspended: false,
            price_limit_up: None,
            price_limit_down: None,
            trading_status: Vec::new(),
            as_of: NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
            bars: Vec::new(),
            price_vals: Vec::new(),
            adj_factors: Vec::new(),
            financial: Vec::new(),
            earnings: Vec::new(),
            market_pe_samples: Vec::new(),
            market_pb_samples: Vec::new(),
            industry_pe_samples: Vec::new(),
            industry_pb_samples: Vec::new(),
            environment: None,
        };
        let config_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/screen.toml");
        let config = ScreenerConfig::load(config_path).unwrap();
        let result = screen(&input, &config);
        assert!(!result.passed);
        assert_eq!(result.symbol, "600519.SH");
        assert_eq!(result.stage, "universe");
    }
}
