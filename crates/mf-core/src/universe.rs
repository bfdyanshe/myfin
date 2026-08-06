//! 点时股票池与交易状态快照。

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// 在 `effective_date` 起生效的股票池快照。
///
/// 同一标的可以有多条记录，编排层只能选择 `effective_date <= as_of` 的最新记录。
/// 这样历史 ST、退市、行业变更和停牌状态不会被当前快照覆盖。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentSnapshot {
    pub symbol: String,
    pub effective_date: NaiveDate,
    pub name: Option<String>,
    pub industry: Option<String>,
    pub is_st: bool,
    pub listed_date: NaiveDate,
    pub delisted_date: Option<NaiveDate>,
    pub is_suspended: bool,
    pub price_limit_up: Option<f64>,
    pub price_limit_down: Option<f64>,
    pub source: String,
}

impl InstrumentSnapshot {
    pub fn applies_to(&self, as_of: NaiveDate) -> bool {
        self.effective_date <= as_of
            && self
                .delisted_date
                .is_none_or(|delisted_date| delisted_date > as_of)
    }
}

/// 历史交易状态；用于回测成交可行性，不能用当前状态回填历史。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradingStatus {
    pub symbol: String,
    pub trade_date: NaiveDate,
    pub is_suspended: bool,
    pub is_limit_up: bool,
    pub is_limit_down: bool,
    pub limit_up: Option<f64>,
    pub limit_down: Option<f64>,
    pub source: String,
}
