//! 财务数据与业绩预告/快报。
//!
//! 财务快照同时保留规范化单季值与来源原始累计值，避免把半年报/三季报
//! 的年初至今数据再次相加。公告日优先使用来源真实 `pubDate`；
//! `ann_date_is_approx` 仅在来源缺少公告日时为 `true`，下游质量门会阻断
//! 需要严格点时数据的流程。

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

/// 财务字段的期间口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinancialPeriodKind {
    /// 已由年初至今值转换得到的单季值，或来源直接给出的单季值。
    SingleQuarter,
    /// 来源原始值为年初至今累计值。
    YearToDate,
    /// 由最近四个单季值构造的滚动值；通常不直接落库。
    Ttm,
}

impl Default for FinancialPeriodKind {
    fn default() -> Self {
        Self::SingleQuarter
    }
}

/// 单一报告期的财务快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinancialData {
    pub symbol: String,
    /// 报告期（如 `2026-03-31`）
    pub report_period: NaiveDate,
    /// 披露时点；若 `ann_date_is_approx` 为真，则这是保守近似值。
    pub ann_date: NaiveDate,
    /// 是否由报告期末加配置偏移推算，而非来源真实公告日。
    #[serde(default = "unknown_ann_date_is_approx")]
    pub ann_date_is_approx: bool,
    /// 来源报告版本或抓取版本标识，用于修订值去重和追溯。
    #[serde(default)]
    pub report_version: Option<String>,
    /// 规范化后用于因子计算的期间口径。
    #[serde(default)]
    pub period_kind: FinancialPeriodKind,
    /// 来源原始字段；半年报、三季报等累计值必须保留。
    #[serde(default)]
    pub raw_fields: Vec<(FinancialField, f64)>,
    pub fields: Vec<(FinancialField, f64)>,
    pub source: String,
}

fn unknown_ann_date_is_approx() -> bool {
    true
}

impl FinancialData {
    pub fn get(&self, field: FinancialField) -> Option<f64> {
        self.fields
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, v)| *v)
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
