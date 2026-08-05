//! 财务数据与业绩预告/快报。
//!
//! 免费数据源（Baostock 等）无公告日期字段，as-of 回测按「报告期末 + 约 2 个月」
//! 近似披露时点（Q1~4-30、H1~8-31、Q3~10-31、年报~次年 4-30），
//! 精度损失在策略文档中声明（docs/strategy.md）。

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// 统一财务字段（季频快照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinancialField {
    /// 营业收入（元）
    Revenue,
    /// 归母净利润（元）
    NetProfit,
    /// 归母股东权益（元）
    Equity,
    /// 总资产（元）
    TotalAssets,
    /// 总负债（元）
    TotalLiabilities,
    /// 经营现金流净额（元）
    OperCashFlow,
    /// 基本每股收益（元）
    Eps,
    /// 每股净资产（元）
    Bps,
    /// 毛利率
    GrossMargin,
    /// 净资产收益率
    Roe,
    /// 资产负债率
    DebtRatio,
}

/// 单一报告期的财务快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinancialData {
    pub symbol: String,
    /// 报告期（如 `2026-03-31`）
    pub report_period: NaiveDate,
    /// 披露时点（免费源为近似值，见模块注释）
    pub ann_date: NaiveDate,
    pub fields: Vec<(FinancialField, f64)>,
    pub source: String,
}

impl FinancialData {
    pub fn get(&self, field: FinancialField) -> Option<f64> {
        self.fields.iter().find(|(f, _)| *f == field).map(|(_, v)| *v)
    }
}

/// 业绩预告/快报：A 股 6 个月收益最强的先行信号之一，披露远早于正式财报。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarningsNotice {
    pub symbol: String,
    /// 预告/快报披露日
    pub ann_date: NaiveDate,
    /// 对应报告期
    pub report_period: NaiveDate,
    /// 类型：预告(forecast) / 快报(express)
    pub kind: NoticeKind,
    /// 归母净利润（元），预告为区间取中值；可选
    pub net_profit: Option<f64>,
    /// 归母净利润同比（%），可选
    pub net_profit_yoy: Option<f64>,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeKind {
    Forecast,
    Express,
}
