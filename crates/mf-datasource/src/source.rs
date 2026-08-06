//! 数据源能力抽象。

use async_trait::async_trait;

use mf_core::Result;
use mf_core::{AdjFactor, DailyBar, EarningsNotice, FinancialData, PriceVal};

use crate::dataset::{Dataset, DatasetSpec};

/// 数据源健康报告。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HealthReport {
    pub source: String,
    pub ok: bool,
    /// 探针耗时（ms）
    pub latency_ms: Option<u64>,
    /// 失败原因
    pub error: Option<String>,
}

/// 一个数据源适配器的能力描述。
#[derive(Debug, Clone)]
pub struct SourceCapabilities {
    pub name: String,
    /// 支持的数据集
    pub datasets: Vec<Dataset>,
    /// 各数据集取数规格
    pub specs: Vec<DatasetSpec>,
}

/// 数据源适配器 trait。
///
/// 实现要求：
/// - 所有产出字段必须符合 mf-core 的统一 schema
/// - 行情一律输出**不复权** OHLCV（复权由存储层用 adj_factor 统一换算）
/// - 限流/退避在适配器内部实现（token bucket），不得暴力请求
#[async_trait]
pub trait Source: Send + Sync {
    fn capabilities(&self) -> &SourceCapabilities;

    /// 健康检查：用基准股做最小探针，返回耗时与状态。
    async fn health_check(&self) -> HealthReport;

    /// 拉取不复权日 K。
    async fn fetch_daily(
        &self,
        symbol: &str,
        start: chrono::NaiveDate,
        end: chrono::NaiveDate,
    ) -> Result<Vec<DailyBar>>;

    /// 拉取复权因子。
    async fn fetch_adj_factor(&self, symbol: &str) -> Result<Vec<AdjFactor>>;

    /// 拉取季频财务（含近似披露日）。
    async fn fetch_financial(&self, symbol: &str) -> Result<Vec<FinancialData>>;

    /// 拉取业绩预告/快报。
    async fn fetch_earnings_notice(&self, symbol: &str) -> Result<Vec<EarningsNotice>>;

    /// 拉取行情派生价格数据（股本等，用于自算估值）。
    async fn fetch_price_val(
        &self,
        symbol: &str,
        start: chrono::NaiveDate,
        end: chrono::NaiveDate,
    ) -> Result<Vec<PriceVal>>;
}
