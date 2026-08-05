//! 数据集类型：注册表与适配器之间约定的数据能力单元。

use serde::{Deserialize, Serialize};
use std::str::FromStr;

use mf_core::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dataset {
    /// 不复权日 K（OHLCV）
    Daily,
    /// 复权因子（后复权累计因子）
    AdjFactor,
    /// 季频财务快照
    Financial,
    /// 业绩预告/快报
    EarningsNotice,
    /// 行情派生价格数据（股本/市值计算用）
    PriceVal,
    /// 宏观指标（PMI/CPI/利率等）
    Macro,
}

impl Dataset {
    pub const ALL: [Dataset; 6] = [
        Dataset::Daily,
        Dataset::AdjFactor,
        Dataset::Financial,
        Dataset::EarningsNotice,
        Dataset::PriceVal,
        Dataset::Macro,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Dataset::Daily => "daily",
            Dataset::AdjFactor => "adj_factor",
            Dataset::Financial => "financial",
            Dataset::EarningsNotice => "earnings_notice",
            Dataset::PriceVal => "price_val",
            Dataset::Macro => "macro",
        }
    }

    /// 该数据集在数据目录中的子路径（存储层约定）。
    pub fn dir(&self) -> &'static str {
        match self {
            Dataset::Daily => "market/daily",
            Dataset::AdjFactor => "market/adj_factor",
            Dataset::Financial => "financial",
            Dataset::EarningsNotice => "financial",
            Dataset::PriceVal => "market/daily",
            Dataset::Macro => "macro",
        }
    }
}

impl FromStr for Dataset {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Dataset::ALL
            .iter()
            .find(|d| d.as_str() == s)
            .copied()
            .ok_or_else(|| Error::Config(format!("未知数据集: {s}")))
    }
}

impl std::fmt::Display for Dataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 数据集的取数规格（供适配器实现与同步器消费）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetSpec {
    pub dataset: Dataset,
    /// 是否支持增量同步（按交易日增量拉取）
    pub incremental: bool,
    /// 健康检查基准（symbol + 数据获取窗口天数）
    pub probe: Option<DatasetProbe>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetProbe {
    pub symbol: String,
    pub lookback_days: u32,
}
