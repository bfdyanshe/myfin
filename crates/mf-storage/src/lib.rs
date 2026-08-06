//! mf-storage: 本地 Parquet 数据层与 DuckDB 查询引擎。
//!
//! M2 提供：按数据集分区的 Parquet 写入/接管、DuckDB 查询和增量同步状态机。
//! 布局约定：
//! ```text
//! data/
//! ├── market/daily/       不复权日 K（按年分文件）
//! ├── market/adj_factor/  复权因子
//! ├── financial/          季频财务 + 业绩预告/快报
//! ├── macro/              宏观指标
//! ├── reports/            Markdown 报告
//! ├── context/            环境扫描结构化结果与背景文档
//! └── sync/               增量同步状态（manifest）
//! ```

pub mod layout;
pub mod parquet;
pub mod staging;
pub mod sync;

pub use layout::Layout;
pub use parquet::{ParquetStore, StorageError};
pub use staging::{StagingEntry, StagingManifest};
pub use sync::{SyncEntry, SyncManifest, SyncStatus};
