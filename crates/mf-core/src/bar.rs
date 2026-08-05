//! 行情数据：不复权 OHLCV + 复权因子。
//!
//! **口径约定（重要）**：存储层一律存**不复权** OHLCV，复权因子单独成表。
//! 所有分位/收益计算在本地用复权因子统一换算为后复权序列，
//! 禁止混用不同数据源的前复权序列（各源除权除息数据不同，历史价格会互相矛盾）。

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// 单个交易日的不复权 OHLCV 柱。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyBar {
    /// `600519.SH`
    pub symbol: String,
    pub trade_date: NaiveDate,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// 成交量（手）
    pub volume: f64,
    /// 成交额（元）
    pub amount: f64,
    /// 数据来源标识，如 `baostock` / `tencent` / `tushare`
    pub source: String,
}

/// 复权因子：后复权累计因子。
///
/// 后复权价格 = 不复权价格 × `cum_factor`。除权除息日为 `ex_date`（当日开始生效）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdjFactor {
    pub symbol: String,
    pub ex_date: NaiveDate,
    pub cum_factor: f64,
    pub source: String,
}
