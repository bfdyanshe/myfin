//! 环境扫描阶段的纯函数实现。

use std::collections::BTreeMap;

use chrono::NaiveDate;
use mf_core::{AdjFactor, DailyBar, EnvironmentSummary, FinancialData, FinancialField};

use crate::EnvironmentCfg;

/// 环境扫描的单标的输入。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EnvironmentMember {
    pub symbol: String,
    pub industry: String,
    pub bars: Vec<DailyBar>,
    pub adj_factors: Vec<AdjFactor>,
    pub financial: Vec<FinancialData>,
}

/// 按行业计算环境归因摘要。
///
/// 所有行情和财务记录均在函数内部按 `as_of` 截断。行业收益和市场收益是成员收益的
/// 等权均值，不依赖外部指数数据；成员不足时返回结构化摘要但不生成标签。
pub fn scan_environment(
    members: &[EnvironmentMember],
    as_of: NaiveDate,
    config: &EnvironmentCfg,
) -> Vec<EnvironmentSummary> {
    let market_returns = members
        .iter()
        .filter_map(|member| member_return(member, as_of, config.return_window_days))
        .collect::<Vec<_>>();
    let market_return = average_if_enough(&market_returns, config.min_members);

    let mut grouped = BTreeMap::<String, Vec<&EnvironmentMember>>::new();
    for member in members {
        let industry = member.industry.trim();
        if !industry.is_empty() {
            grouped
                .entry(industry.to_string())
                .or_default()
                .push(member);
        }
    }

    grouped
        .into_iter()
        .map(|(industry, members)| {
            let returns = members
                .iter()
                .filter_map(|member| member_return(member, as_of, config.return_window_days))
                .collect::<Vec<_>>();
            let industry_return = average_if_enough(&returns, config.min_members);
            let relative_return = industry_return
                .zip(market_return)
                .map(|(industry, market)| industry - market);

            let profit_trends = members
                .iter()
                .filter_map(|member| profit_trend(member, as_of, config.profit_trend_quarters))
                .collect::<Vec<_>>();
            let profit_trend_share = average_if_enough(
                &profit_trends
                    .iter()
                    .map(|improving| if *improving { 1.0 } else { 0.0 })
                    .collect::<Vec<_>>(),
                config.min_members,
            );

            let mut tags = Vec::new();
            if relative_return.is_some_and(|value| value < 0.0) {
                tags.push("industry_relative_underperformed".to_string());
            }
            if profit_trend_share.is_some_and(|value| value >= config.earnings_turning_min_share) {
                tags.push("industry_earnings_turning".to_string());
            }

            EnvironmentSummary {
                industry,
                as_of,
                return_window_days: config.return_window_days,
                member_count: members.len() as u32,
                valid_return_members: returns.len() as u32,
                industry_return,
                market_return,
                relative_return,
                valid_profit_members: profit_trends.len() as u32,
                profit_trend_share,
                tags,
            }
        })
        .collect()
}

fn average_if_enough(values: &[f64], min_members: u32) -> Option<f64> {
    if min_members == 0 || values.len() < min_members as usize {
        return None;
    }
    let total = values.iter().sum::<f64>();
    let average = total / values.len() as f64;
    average.is_finite().then_some(average)
}

fn member_return(member: &EnvironmentMember, as_of: NaiveDate, window_days: u32) -> Option<f64> {
    let mut by_date = BTreeMap::<NaiveDate, &DailyBar>::new();
    for bar in member
        .bars
        .iter()
        .filter(|bar| bar.symbol == member.symbol && bar.trade_date <= as_of)
    {
        by_date
            .entry(bar.trade_date)
            .and_modify(|current| {
                if bar.source < current.source {
                    *current = bar;
                }
            })
            .or_insert(bar);
    }
    let prices = by_date
        .into_values()
        .filter_map(|bar| adjusted_close(bar, &member.adj_factors))
        .collect::<Vec<_>>();
    let end = prices.last().copied()?;
    let start = prices
        .get(prices.len().checked_sub(window_days as usize + 1)?)
        .copied()?;
    (start > 0.0)
        .then_some(end / start - 1.0)
        .filter(|value| value.is_finite())
}

fn adjusted_close(bar: &DailyBar, factors: &[AdjFactor]) -> Option<f64> {
    let factor = factors
        .iter()
        .filter(|factor| factor.symbol == bar.symbol && factor.ex_date <= bar.trade_date)
        .max_by(|left, right| {
            left.ex_date
                .cmp(&right.ex_date)
                .then_with(|| right.source.cmp(&left.source))
        })
        .map(|factor| factor.cum_factor)
        .unwrap_or(1.0);
    let close = bar.close * factor;
    (close.is_finite() && close > 0.0).then_some(close)
}

fn profit_trend(member: &EnvironmentMember, as_of: NaiveDate, quarters: u32) -> Option<bool> {
    if quarters == 0 {
        return None;
    }
    let mut by_period = BTreeMap::<NaiveDate, &FinancialData>::new();
    for record in member.financial.iter().filter(|record| {
        record.symbol == member.symbol && record.ann_date <= as_of && record.report_period <= as_of
    }) {
        by_period
            .entry(record.report_period)
            .and_modify(|current| {
                if record.ann_date > current.ann_date {
                    *current = record;
                }
            })
            .or_insert(record);
    }
    let records = by_period.into_values().collect::<Vec<_>>();
    let count = quarters as usize;
    if records.len() < count.saturating_mul(2) {
        return None;
    }
    let current = sum_profit(&records[records.len() - count..])?;
    let previous = sum_profit(&records[records.len() - count * 2..records.len() - count])?;
    Some(current > previous)
}

fn sum_profit(records: &[&FinancialData]) -> Option<f64> {
    let total = records.iter().try_fold(0.0, |total, record| {
        let value = record.get(FinancialField::NetProfit)?;
        value.is_finite().then_some(total + value)
    })?;
    total.is_finite().then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EnvironmentCfg;

    fn bar(symbol: &str, date: &str, close: f64) -> DailyBar {
        DailyBar {
            symbol: symbol.to_string(),
            trade_date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
            amount: close,
            source: "test".to_string(),
        }
    }

    fn financial(symbol: &str, year: i32, quarter: u32, profit: f64) -> FinancialData {
        let month = quarter * 3;
        FinancialData {
            symbol: symbol.to_string(),
            report_period: NaiveDate::from_ymd_opt(year, month, 1).unwrap(),
            ann_date: NaiveDate::from_ymd_opt(year, month, 15).unwrap(),
            ann_date_is_approx: false,
            report_version: None,
            period_kind: mf_core::FinancialPeriodKind::SingleQuarter,
            raw_fields: vec![(FinancialField::NetProfit, profit)],
            fields: vec![(FinancialField::NetProfit, profit)],
            source: "test".to_string(),
        }
    }

    fn member(symbol: &str, industry: &str, prices: &[f64], profit: f64) -> EnvironmentMember {
        let dates = ["2026-01-01", "2026-01-02", "2026-01-03"];
        EnvironmentMember {
            symbol: symbol.to_string(),
            industry: industry.to_string(),
            bars: dates
                .into_iter()
                .zip(prices)
                .map(|(date, close)| bar(symbol, date, *close))
                .collect(),
            adj_factors: Vec::new(),
            financial: (1..=2)
                .flat_map(|year| {
                    let value = if year == 1 { profit / 2.0 } else { profit };
                    (1..=4).map(move |quarter| financial(symbol, 2023 + year, quarter, value))
                })
                .collect(),
        }
    }

    #[test]
    fn computes_relative_underperformance_and_profit_turning_tags() {
        let members = vec![
            member("A", "steel", &[100.0, 90.0, 80.0], 5.0),
            member("B", "steel", &[100.0, 110.0, 120.0], 4.0),
            member("C", "tech", &[100.0, 130.0, 160.0], 4.0),
        ];
        let config = EnvironmentCfg {
            return_window_days: 2,
            profit_trend_quarters: 4,
            min_members: 2,
            earnings_turning_min_share: 0.5,
        };
        let result = scan_environment(
            &members,
            NaiveDate::from_ymd_opt(2026, 1, 3).unwrap(),
            &config,
        );
        let steel = result
            .iter()
            .find(|summary| summary.industry == "steel")
            .unwrap();
        assert_eq!(steel.valid_return_members, 2);
        assert!(steel.relative_return.unwrap() < 0.0);
        assert_eq!(steel.profit_trend_share, Some(1.0));
        assert_eq!(
            steel.tags,
            vec![
                "industry_relative_underperformed".to_string(),
                "industry_earnings_turning".to_string()
            ]
        );
    }

    #[test]
    fn excludes_future_rows_from_as_of_snapshot() {
        let mut member = member("A", "steel", &[100.0, 100.0, 200.0], 5.0);
        member.bars[2].trade_date = NaiveDate::from_ymd_opt(2026, 1, 4).unwrap();
        let config = EnvironmentCfg {
            return_window_days: 1,
            profit_trend_quarters: 4,
            min_members: 1,
            earnings_turning_min_share: 0.5,
        };
        let result = scan_environment(
            &[member],
            NaiveDate::from_ymd_opt(2026, 1, 3).unwrap(),
            &config,
        );
        assert_eq!(result[0].industry_return, Some(0.0));
    }
}
