//! 环境扫描结果。

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// 单个行业在某个 as-of 截面的可解释环境摘要。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentSummary {
    pub industry: String,
    pub as_of: NaiveDate,
    /// 行情收益使用的交易日观察窗口。
    pub return_window_days: u32,
    /// 行业内输入标的数量。
    pub member_count: u32,
    /// 有效行情收益的行业成员数量。
    pub valid_return_members: u32,
    /// 有效行业收益（小数，例如 0.1 表示 10%）。
    pub industry_return: Option<f64>,
    /// 有效市场收益（小数）。
    pub market_return: Option<f64>,
    /// 行业收益减市场收益（小数）。
    pub relative_return: Option<f64>,
    /// 有效盈利趋势判断的行业成员数量。
    pub valid_profit_members: u32,
    /// TTM 净利润较前一组 TTM 改善的成员占比。
    pub profit_trend_share: Option<f64>,
    /// 由上述数值确定的机器标签；不包含人工判断。
    pub tags: Vec<String>,
}
