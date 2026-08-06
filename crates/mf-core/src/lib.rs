//! mf-core: 领域模型与统一 schema。
//!
//! 所有数据源适配器产出的数据都必须规范化到本 crate 定义的模型后
//! 才能写入存储层。字段命名即 canonical schema（详见 docs/data-sources.md）。

pub mod bar;
pub mod error;
pub mod environment;
pub mod financial;
pub mod symbol;
pub mod valuation;

pub use bar::{AdjFactor, DailyBar};
pub use error::{Error, Result};
pub use environment::EnvironmentSummary;
pub use financial::{EarningsNotice, FinancialData, FinancialField};
pub use symbol::{Exchange, Market, Symbol};
pub use valuation::{PriceVal, ValuationPoint};
