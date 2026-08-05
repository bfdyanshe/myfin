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
        }
    }
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
}

#[derive(Debug, Default)]
struct RunResult {
    evaluated_snapshots: u32,
    selected: u32,
    returns: Vec<f64>,
    yearly_selected: BTreeMap<i32, u32>,
    yearly_returns: BTreeMap<i32, Vec<f64>>,
    trades: Vec<BacktestTrade>,
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
            let Some(entry) = bar_on_or_before(&input.bars, &input.symbol, as_of) else {
                continue;
            };
            let entry_price = adjusted_close(entry, &input.adj_factors);
            let target = add_months(as_of, options.hold_months);
            let exit = first_bar_on_or_after(&input.bars, &input.symbol, target);
            let (exit_date, exit_price, return_pct) = match exit {
                Some(exit) => {
                    let price = adjusted_close(exit, &input.adj_factors);
                    let return_pct = (entry_price > 0.0 && price.is_finite())
                        .then_some((price / entry_price - 1.0) * 100.0);
                    (Some(exit.trade_date), Some(price), return_pct)
                }
                None => (None, None, None),
            };
            if let Some(value) = return_pct {
                result.returns.push(value);
                result
                    .yearly_returns
                    .entry(as_of.year())
                    .or_default()
                    .push(value);
            }
            result.trades.push(BacktestTrade {
                symbol: input.symbol,
                as_of,
                entry_date: entry.trade_date,
                exit_date,
                entry_price,
                exit_price,
                return_pct,
                secondary_signal_count: screened.metrics.secondary_signal_count,
            });
        }
    }
    result
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

fn bar_on_or_before<'a>(
    bars: &'a [DailyBar],
    symbol: &str,
    date: NaiveDate,
) -> Option<&'a DailyBar> {
    bars.iter()
        .filter(|bar| bar.symbol == symbol && bar.trade_date <= date)
        .max_by_key(|bar| bar.trade_date)
}

fn first_bar_on_or_after<'a>(
    bars: &'a [DailyBar],
    symbol: &str,
    date: NaiveDate,
) -> Option<&'a DailyBar> {
    bars.iter()
        .filter(|bar| bar.symbol == symbol && bar.trade_date >= date)
        .min_by_key(|bar| bar.trade_date)
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
