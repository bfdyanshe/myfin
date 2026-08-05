//! 估值数据（本地自算路径）。
//!
//! 零成本口径：PE = 每日市值 / TTM 归母净利；PB = 每日市值 / 归母股东权益。
//! 市值 = 不复权收盘价 × 总股本。财务数据按 as-of 规则对齐（ann_date 过滤）。

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// 单日估值点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValuationPoint {
    pub symbol: String,
    pub trade_date: NaiveDate,
    pub close: f64,
    pub market_cap: f64,
    pub pe_ttm: Option<f64>,
    pub pb: Option<f64>,
}

/// 行情派生价格数据（用于自算估值：股本信息）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceVal {
    pub symbol: String,
    pub trade_date: NaiveDate,
    pub close: f64,
    /// 总股本（股）
    pub total_shares: f64,
    /// 流通股本（股）
    pub float_shares: f64,
    pub source: String,
}
