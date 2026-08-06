//! 月度截面回测与参数敏感性分析。
//!
//! 回测只接收已经准备好的历史候选输入，筛选仍复用 mf-screener，
//! 因此实盘与回测共用同一套 as-of 过滤规则。

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use mf_core::DailyBar;
use mf_screener::{screen, ScreenInput, ScreenerConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestOptions {
    /// 起始截面；省略时使用输入数据的最早月份。
    pub start: Option<NaiveDate>,
    /// 结束截面；省略时使用输入数据的最晚月份。
    pub end: Option<NaiveDate>,
    /// 固定持有期（月）。
    pub hold_months: u32,
    /// 是否生成敏感性网格。
    pub include_sensitivity: bool,
    /// 信号日后的第几个交易日成交。
    #[serde(default = "default_execution_lag")]
    pub execution_lag_trading_days: u32,
    /// 每个调仓批次最多持仓数。
    #[serde(default = "default_max_positions")]
    pub max_positions: u32,
    /// 单行业权重上限。
    #[serde(default = "default_max_industry_weight")]
    pub max_industry_weight: f64,
    /// 单边交易成本（基点）。
    #[serde(default)]
    pub transaction_cost_bps: f64,
    /// 单边滑点（基点）。
    #[serde(default)]
    pub slippage_bps: f64,
    /// 成交额容量门，0 表示关闭。
    #[serde(default)]
    pub min_entry_amount: f64,
}

/// 一个标的的历史筛选输入。横截面样本按 as-of 日期保存，避免把未来估值
/// 样本带入早期截面。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalCandidate {
    pub input: ScreenInput,
    pub market_pe_samples: BTreeMap<NaiveDate, Vec<f64>>,
    pub market_pb_samples: BTreeMap<NaiveDate, Vec<f64>>,
    pub industry_pe_samples: BTreeMap<NaiveDate, Vec<f64>>,
    pub industry_pb_samples: BTreeMap<NaiveDate, Vec<f64>>,
}

impl Default for BacktestOptions {
    fn default() -> Self {
        Self {
            start: None,
            end: None,
            hold_months: 6,
            include_sensitivity: true,
            execution_lag_trading_days: default_execution_lag(),
            max_positions: default_max_positions(),
            max_industry_weight: default_max_industry_weight(),
            transaction_cost_bps: 5.0,
            slippage_bps: 5.0,
            min_entry_amount: 0.0,
        }
    }
}

fn default_execution_lag() -> u32 {
    1
}

fn default_max_positions() -> u32 {
    20
}

fn default_max_industry_weight() -> f64 {
    0.25
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestTrade {
    pub symbol: String,
    pub as_of: NaiveDate,
    pub entry_date: NaiveDate,
    pub exit_date: Option<NaiveDate>,
    /// 收益率，百分比数值，例如 12.5 表示 12.5%。
    pub entry_price: f64,
    pub exit_price: Option<f64>,
    pub industry: Option<String>,
    /// 批次内目标权重（小数）。
    pub weight: f64,
    /// 未扣成本的收益率。
    pub gross_return_pct: Option<f64>,
    /// 往返成本与滑点。
    pub transaction_cost_pct: f64,
    pub return_pct: Option<f64>,
    pub secondary_signal_count: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReturnStats {
    pub count: u32,
    pub mean_pct: Option<f64>,
    pub median_pct: Option<f64>,
    pub win_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YearSummary {
    pub year: i32,
    pub selected: u32,
    pub completed: ReturnStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensitivityCell {
    pub percentile_max: f64,
    pub momentum_days: u32,
    pub ma_days: u32,
    pub selected: u32,
    pub completed: ReturnStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestReport {
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
    pub hold_months: u32,
    pub evaluated_snapshots: u32,
    pub selected: u32,
    pub completed: ReturnStats,
    pub yearly: Vec<YearSummary>,
    pub sensitivity: Vec<SensitivityCell>,
    pub trades: Vec<BacktestTrade>,
    pub portfolio: PortfolioStats,
    pub out_of_sample: Option<ReturnStats>,
    pub ablations: Vec<AblationResult>,
    pub factor_correlations: Vec<FactorCorrelation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PortfolioStats {
    pub periods: u32,
    pub mean_monthly_return_pct: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub annualized_volatility_pct: Option<f64>,
    pub turnover_pct: f64,
    pub average_holdings: f64,
    pub average_cash_pct: f64,
    pub industry_exposure_pct: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AblationResult {
    pub name: String,
    pub selected: u32,
    pub completed: ReturnStats,
    pub portfolio: PortfolioStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactorCorrelation {
    pub left: String,
    pub right: String,
    pub pearson: Option<f64>,
}

#[derive(Debug, Default)]
struct RunResult {
    evaluated_snapshots: u32,
    selected: u32,
    returns: Vec<f64>,
    yearly_selected: BTreeMap<i32, u32>,
    yearly_returns: BTreeMap<i32, Vec<f64>>,
    trades: Vec<BacktestTrade>,
    portfolio_returns: Vec<f64>,
    turnover_pct: f64,
    holdings: Vec<u32>,
    cash_pct: Vec<f64>,
    industry_weights: BTreeMap<String, Vec<f64>>,
    factor_samples: Vec<[f64; 4]>,
}

/// 根据每个候选的交易日重建每月最后一个交易日。
pub fn month_end_dates(
    candidates: &[ScreenInput],
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
) -> Vec<NaiveDate> {
    let mut dates = BTreeMap::<(i32, u32), NaiveDate>::new();
    for candidate in candidates {
        for bar in &candidate.bars {
            if bar.symbol != candidate.symbol
                || start.is_some_and(|value| bar.trade_date < value)
                || end.is_some_and(|value| bar.trade_date > value)
            {
                continue;
            }
            let key = (bar.trade_date.year(), bar.trade_date.month());
            dates
                .entry(key)
                .and_modify(|current| *current = (*current).max(bar.trade_date))
                .or_insert(bar.trade_date);
        }
    }
    dates.into_values().collect()
}

/// 运行默认参数回测，并按 ADR-0003 生成敏感性网格。
pub fn run(
    candidates: &[ScreenInput],
    config: &ScreenerConfig,
    options: &BacktestOptions,
) -> BacktestReport {
    let candidates = candidates
        .iter()
        .cloned()
        .map(|input| {
            let as_of = input.as_of;
            HistoricalCandidate {
                market_pe_samples: BTreeMap::from([(as_of, input.market_pe_samples.clone())]),
                market_pb_samples: BTreeMap::from([(as_of, input.market_pb_samples.clone())]),
                industry_pe_samples: BTreeMap::from([(as_of, input.industry_pe_samples.clone())]),
                industry_pb_samples: BTreeMap::from([(as_of, input.industry_pb_samples.clone())]),
                input,
            }
        })
        .collect::<Vec<_>>();
    run_historical(&candidates, config, options)
}

/// 运行带历史横截面样本的回测。
pub fn run_historical(
    candidates: &[HistoricalCandidate],
    config: &ScreenerConfig,
    options: &BacktestOptions,
) -> BacktestReport {
    let dates = month_end_dates_historical(candidates, options.start, options.end);
    let baseline = run_once(candidates, config, &dates, options);
    let sensitivity = if options.include_sensitivity {
        sensitivity(candidates, config, &dates, options)
    } else {
        Vec::new()
    };
    let yearly = baseline
        .yearly_selected
        .keys()
        .copied()
        .map(|year| YearSummary {
            year,
            selected: baseline
                .yearly_selected
                .get(&year)
                .copied()
                .unwrap_or_default(),
            completed: stats(
                baseline
                    .yearly_returns
                    .get(&year)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            ),
        })
        .collect();
    let portfolio = portfolio_stats(&baseline);
    let out_of_sample = if dates.len() >= 4 {
        let split = dates.len() / 2;
        let oos = run_once(candidates, config, &dates[split..], options);
        (!oos.returns.is_empty()).then(|| stats(&oos.returns))
    } else {
        None
    };
    let ablations = ablations(candidates, config, &dates, options);
    let factor_correlations = factor_correlations(&baseline.factor_samples);
    BacktestReport {
        start: dates.first().copied(),
        end: dates.last().copied(),
        hold_months: options.hold_months,
        evaluated_snapshots: baseline.evaluated_snapshots,
        selected: baseline.selected,
        completed: stats(&baseline.returns),
        yearly,
        sensitivity,
        trades: baseline.trades,
        portfolio,
        out_of_sample,
        ablations,
        factor_correlations,
    }
}

fn run_once(
    candidates: &[HistoricalCandidate],
    config: &ScreenerConfig,
    dates: &[NaiveDate],
    options: &BacktestOptions,
) -> RunResult {
    let mut result = RunResult::default();
    for &as_of in dates {
        let mut pending = Vec::new();
        for candidate in candidates {
            result.evaluated_snapshots += 1;
            let mut input = candidate.input.clone();
            input.as_of = as_of;
            input.market_pe_samples =
                sample_at(&candidate.market_pe_samples, as_of).unwrap_or_default();
            input.market_pb_samples =
                sample_at(&candidate.market_pb_samples, as_of).unwrap_or_default();
            input.industry_pe_samples =
                sample_at(&candidate.industry_pe_samples, as_of).unwrap_or_default();
            input.industry_pb_samples =
                sample_at(&candidate.industry_pb_samples, as_of).unwrap_or_default();
            let screened = screen(&input, config);
            if !screened.passed {
                continue;
            }
            result.selected += 1;
            *result.yearly_selected.entry(as_of.year()).or_default() += 1;
            let latest_close = input
                .bars
                .iter()
                .filter(|bar| bar.symbol == input.symbol && bar.trade_date <= as_of)
                .max_by_key(|bar| bar.trade_date)
                .map(|bar| bar.close);
            result.factor_samples.push([
                screened.metrics.momentum_3m.unwrap_or(f64::NAN),
                latest_close
                    .zip(screened.metrics.ma120)
                    .map(|(close, ma)| (close / ma - 1.0) * 100.0)
                    .unwrap_or(f64::NAN),
                screened.metrics.volume_ratio.unwrap_or(f64::NAN),
                screened
                    .metrics
                    .pe_percentile
                    .map(|v| v * 100.0)
                    .unwrap_or(f64::NAN),
            ]);
            let Some(entry) = nth_bar_on_or_after(
                &input.bars,
                &input.symbol,
                as_of,
                options.execution_lag_trading_days,
            ) else {
                continue;
            };
            let Some(entry_status) = status_at(&input.trading_status, entry.trade_date) else {
                continue;
            };
            if entry_status.is_suspended || entry_status.is_limit_up {
                continue;
            }
            if options.min_entry_amount > 0.0 && entry.amount < options.min_entry_amount {
                continue;
            }
            if input
                .price_limit_up
                .is_some_and(|limit| entry.close >= limit)
                || input
                    .price_limit_down
                    .is_some_and(|limit| entry.close <= limit)
            {
                continue;
            }
            let entry_price = adjusted_close(entry, &input.adj_factors);
            let target = add_months(as_of, options.hold_months);
            let exit = first_executable_exit(&input, target);
            let (exit_date, exit_price, return_pct) = match exit {
                Some(exit) => {
                    let price = adjusted_close(exit, &input.adj_factors);
                    let return_pct = (entry_price > 0.0 && price.is_finite())
                        .then_some((price / entry_price - 1.0) * 100.0);
                    (Some(exit.trade_date), Some(price), return_pct)
                }
                None => (None, None, None),
            };
            let gross_return_pct = return_pct;
            let transaction_cost_pct =
                2.0 * (options.transaction_cost_bps + options.slippage_bps) / 100.0;
            let net_return_pct = gross_return_pct.map(|value| {
                ((1.0 + value / 100.0) * (1.0 - transaction_cost_pct / 100.0) - 1.0) * 100.0
            });
            pending.push(PendingTrade {
                symbol: input.symbol,
                industry: input.industry,
                as_of,
                entry_date: entry.trade_date,
                exit_date,
                entry_price,
                exit_price,
                gross_return_pct,
                net_return_pct,
                transaction_cost_pct,
                secondary_signal_count: screened.metrics.secondary_signal_count,
            });
        }

        pending.sort_by(|left, right| left.symbol.cmp(&right.symbol));
        let max_positions = options.max_positions.max(1) as usize;
        let max_per_industry =
            ((max_positions as f64 * options.max_industry_weight).floor() as usize).max(1);
        let weight = 1.0 / max_positions as f64;
        let mut industry_counts = BTreeMap::<String, usize>::new();
        let mut batch_return = 0.0;
        let mut batch_completed = false;
        let mut batch_holdings = 0_u32;
        for pending in pending.into_iter() {
            if result
                .trades
                .iter()
                .filter(|trade| trade.as_of == as_of)
                .count()
                >= max_positions
            {
                break;
            }
            let industry = pending
                .industry
                .clone()
                .unwrap_or_else(|| "__unknown__".to_string());
            let count = industry_counts.entry(industry.clone()).or_default();
            if *count >= max_per_industry {
                continue;
            }
            *count += 1;
            batch_holdings += 1;
            result.turnover_pct += weight * 200.0;
            result
                .industry_weights
                .entry(industry)
                .or_default()
                .push(weight);
            if let Some(value) = pending.net_return_pct {
                result.returns.push(value);
                result
                    .yearly_returns
                    .entry(as_of.year())
                    .or_default()
                    .push(value);
                batch_return += value * weight;
                batch_completed = true;
            }
            result.trades.push(BacktestTrade {
                symbol: pending.symbol,
                as_of: pending.as_of,
                entry_date: pending.entry_date,
                exit_date: pending.exit_date,
                entry_price: pending.entry_price,
                exit_price: pending.exit_price,
                return_pct: pending.net_return_pct,
                industry: pending.industry,
                weight,
                gross_return_pct: pending.gross_return_pct,
                transaction_cost_pct: pending.transaction_cost_pct,
                secondary_signal_count: pending.secondary_signal_count,
            });
        }
        let invested = result
            .trades
            .iter()
            .filter(|trade| trade.as_of == as_of)
            .map(|trade| trade.weight)
            .sum::<f64>();
        result.cash_pct.push((1.0 - invested).max(0.0));
        result.holdings.push(batch_holdings);
        if batch_completed {
            result.portfolio_returns.push(batch_return);
        }
    }
    result
}

#[derive(Debug)]
struct PendingTrade {
    symbol: String,
    industry: Option<String>,
    as_of: NaiveDate,
    entry_date: NaiveDate,
    exit_date: Option<NaiveDate>,
    entry_price: f64,
    exit_price: Option<f64>,
    gross_return_pct: Option<f64>,
    net_return_pct: Option<f64>,
    transaction_cost_pct: f64,
    secondary_signal_count: u8,
}

fn sensitivity(
    candidates: &[HistoricalCandidate],
    config: &ScreenerConfig,
    dates: &[NaiveDate],
    options: &BacktestOptions,
) -> Vec<SensitivityCell> {
    let mut cells = Vec::new();
    for percentile_max in [0.20, 0.30, 0.40] {
        for momentum_days in [63, 126] {
            for ma_days in [60, 120, 250] {
                let mut candidate_config = config.clone();
                candidate_config.undervalued.percentile_max = percentile_max;
                candidate_config.recovery.momentum_days = momentum_days;
                candidate_config.recovery.ma_days = ma_days;
                let result = run_once(candidates, &candidate_config, dates, options);
                cells.push(SensitivityCell {
                    percentile_max,
                    momentum_days,
                    ma_days,
                    selected: result.selected,
                    completed: stats(&result.returns),
                });
            }
        }
    }
    cells
}

fn ablations(
    candidates: &[HistoricalCandidate],
    config: &ScreenerConfig,
    dates: &[NaiveDate],
    options: &BacktestOptions,
) -> Vec<AblationResult> {
    let variants: [(&str, fn(&mut ScreenerConfig)); 3] = [
        ("without_quality_exclusion", |cfg: &mut ScreenerConfig| {
            cfg.exclusion.max_consecutive_loss_quarters = u32::MAX;
            cfg.exclusion.max_neg_cashflow_quarters = u32::MAX;
            cfg.exclusion.max_debt_ratio = 1.0;
            cfg.exclusion.exclude_negative_equity = false;
        }),
        ("without_recovery_signals", |cfg: &mut ScreenerConfig| {
            cfg.recovery.require_earnings_turnaround = false;
            cfg.recovery.min_secondary_signals = 0;
        }),
        (
            "without_price_secondary_signals",
            |cfg: &mut ScreenerConfig| {
                cfg.recovery.min_secondary_signals = 0;
            },
        ),
    ];
    variants
        .into_iter()
        .map(|(name, mutate)| {
            let mut variant = config.clone();
            mutate(&mut variant);
            let result = run_once(candidates, &variant, dates, options);
            AblationResult {
                name: name.to_string(),
                selected: result.selected,
                completed: stats(&result.returns),
                portfolio: portfolio_stats(&result),
            }
        })
        .collect()
}

fn portfolio_stats(result: &RunResult) -> PortfolioStats {
    let periods = result.portfolio_returns.len() as u32;
    let mean_monthly_return_pct = (!result.portfolio_returns.is_empty()).then(|| {
        result.portfolio_returns.iter().sum::<f64>() / result.portfolio_returns.len() as f64
    });
    let max_drawdown_pct = if result.portfolio_returns.is_empty() {
        None
    } else {
        let mut equity = 1.0;
        let mut peak: f64 = 1.0;
        let mut max_drawdown: f64 = 0.0;
        for value in &result.portfolio_returns {
            equity *= 1.0 + value / 100.0;
            peak = peak.max(equity);
            max_drawdown = max_drawdown.min((equity / peak - 1.0) * 100.0);
        }
        Some(max_drawdown)
    };
    let annualized_volatility_pct = if result.portfolio_returns.len() < 2 {
        None
    } else {
        let mean =
            result.portfolio_returns.iter().sum::<f64>() / result.portfolio_returns.len() as f64;
        let variance = result
            .portfolio_returns
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (result.portfolio_returns.len() - 1) as f64;
        Some(variance.sqrt() * 12.0_f64.sqrt())
    };
    let denominator = result.holdings.len().max(1) as f64;
    let average_holdings = result.holdings.iter().map(|v| *v as f64).sum::<f64>() / denominator;
    let average_cash_pct =
        result.cash_pct.iter().sum::<f64>() / result.cash_pct.len().max(1) as f64 * 100.0;
    let industry_exposure_pct = result
        .industry_weights
        .iter()
        .map(|(industry, weights)| {
            (
                industry.clone(),
                weights.iter().sum::<f64>() / denominator * 100.0,
            )
        })
        .collect();
    PortfolioStats {
        periods,
        mean_monthly_return_pct,
        max_drawdown_pct,
        annualized_volatility_pct,
        turnover_pct: result.turnover_pct,
        average_holdings,
        average_cash_pct,
        industry_exposure_pct,
    }
}

fn factor_correlations(samples: &[[f64; 4]]) -> Vec<FactorCorrelation> {
    let names = ["momentum_3m", "ma_gap", "volume_ratio", "pe_percentile"];
    let mut output = Vec::new();
    for left in 0..names.len() {
        for right in (left + 1)..names.len() {
            let pairs = samples
                .iter()
                .filter_map(|sample| {
                    let x = sample[left];
                    let y = sample[right];
                    (x.is_finite() && y.is_finite()).then_some((x, y))
                })
                .collect::<Vec<_>>();
            let pearson = if pairs.len() < 3 {
                None
            } else {
                let mean_x = pairs.iter().map(|(x, _)| *x).sum::<f64>() / pairs.len() as f64;
                let mean_y = pairs.iter().map(|(_, y)| *y).sum::<f64>() / pairs.len() as f64;
                let mut covariance = 0.0;
                let mut variance_x = 0.0;
                let mut variance_y = 0.0;
                for (x, y) in pairs {
                    let dx = x - mean_x;
                    let dy = y - mean_y;
                    covariance += dx * dy;
                    variance_x += dx * dx;
                    variance_y += dy * dy;
                }
                (variance_x > 0.0 && variance_y > 0.0)
                    .then_some(covariance / (variance_x * variance_y).sqrt())
            };
            output.push(FactorCorrelation {
                left: names[left].to_string(),
                right: names[right].to_string(),
                pearson,
            });
        }
    }
    output
}

fn month_end_dates_historical(
    candidates: &[HistoricalCandidate],
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
) -> Vec<NaiveDate> {
    let inputs = candidates
        .iter()
        .map(|candidate| candidate.input.clone())
        .collect::<Vec<_>>();
    month_end_dates(&inputs, start, end)
}

fn sample_at(samples: &BTreeMap<NaiveDate, Vec<f64>>, as_of: NaiveDate) -> Option<Vec<f64>> {
    samples
        .range(..=as_of)
        .next_back()
        .map(|(_, values)| values.clone())
}

fn stats(values: &[f64]) -> ReturnStats {
    if values.is_empty() {
        return ReturnStats {
            count: 0,
            mean_pct: None,
            median_pct: None,
            win_rate: None,
        };
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median_pct = if sorted.len() % 2 == 0 {
        let right = sorted.len() / 2;
        Some((sorted[right - 1] + sorted[right]) / 2.0)
    } else {
        Some(sorted[sorted.len() / 2])
    };
    let wins = values.iter().filter(|value| **value > 0.0).count();
    ReturnStats {
        count: values.len() as u32,
        mean_pct: Some(values.iter().sum::<f64>() / values.len() as f64),
        median_pct,
        win_rate: Some(wins as f64 / values.len() as f64),
    }
}

fn nth_bar_on_or_after<'a>(
    bars: &'a [DailyBar],
    symbol: &str,
    date: NaiveDate,
    lag: u32,
) -> Option<&'a DailyBar> {
    let mut dates = bars
        .iter()
        .filter(|bar| bar.symbol == symbol && bar.trade_date > date)
        .collect::<Vec<_>>();
    dates.sort_by_key(|bar| bar.trade_date);
    dates.get(lag.saturating_sub(1) as usize).copied()
}

fn first_executable_exit<'a>(input: &'a ScreenInput, date: NaiveDate) -> Option<&'a DailyBar> {
    input
        .bars
        .iter()
        .filter(|bar| bar.symbol == input.symbol && bar.trade_date >= date)
        .filter(|bar| {
            status_at(&input.trading_status, bar.trade_date)
                .is_some_and(|status| !status.is_suspended && !status.is_limit_down)
        })
        .min_by_key(|bar| bar.trade_date)
}

fn status_at<'a>(
    statuses: &'a [mf_core::TradingStatus],
    date: NaiveDate,
) -> Option<&'a mf_core::TradingStatus> {
    statuses.iter().find(|status| status.trade_date == date)
}

fn adjusted_close(bar: &DailyBar, factors: &[mf_core::AdjFactor]) -> f64 {
    let factor = factors
        .iter()
        .filter(|factor| factor.symbol == bar.symbol && factor.ex_date <= bar.trade_date)
        .max_by_key(|factor| factor.ex_date)
        .map(|factor| factor.cum_factor)
        .unwrap_or(1.0);
    bar.close * factor
}

fn add_months(date: NaiveDate, months: u32) -> NaiveDate {
    let month_index = date.year() * 12 + date.month0() as i32 + months as i32;
    let year = month_index.div_euclid(12);
    let month0 = month_index.rem_euclid(12) as u32;
    let month = month0 + 1;
    let last_day = days_in_month(year, month);
    NaiveDate::from_ymd_opt(year, month, date.day().min(last_day)).unwrap()
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(symbol: &str, date: &str) -> DailyBar {
        DailyBar {
            symbol: symbol.to_string(),
            trade_date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
            amount: 1.0,
            source: "test".to_string(),
        }
    }

    #[test]
    fn rebuilds_month_end_dates() {
        let mut input = ScreenInput {
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
            as_of: NaiveDate::from_ymd_opt(2026, 2, 28).unwrap(),
            bars: vec![
                bar("600519.SH", "2026-01-30"),
                bar("600519.SH", "2026-01-31"),
                bar("600519.SH", "2026-02-27"),
            ],
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
        input.bars.push(bar("000001.SZ", "2026-02-28"));
        assert_eq!(
            month_end_dates(&[input], None, None),
            vec![
                NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
                NaiveDate::from_ymd_opt(2026, 2, 27).unwrap(),
            ]
        );
    }

    #[test]
    fn handles_month_end_and_return_statistics() {
        assert_eq!(
            add_months(NaiveDate::from_ymd_opt(2024, 2, 29).unwrap(), 12),
            NaiveDate::from_ymd_opt(2025, 2, 28).unwrap()
        );
        assert_eq!(
            stats(&[-10.0, 5.0, 15.0]),
            ReturnStats {
                count: 3,
                mean_pct: Some(10.0 / 3.0),
                median_pct: Some(5.0),
                win_rate: Some(2.0 / 3.0),
            }
        );
    }
}
