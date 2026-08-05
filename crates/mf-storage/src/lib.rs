//! mf-storage: 本地数据层（Parquet + DuckDB 将在 M2 引入）。
//!
//! M1 提供：数据目录布局约定 + 增量同步状态机（manifest）。
//! 布局约定：
//! ```text
//! data/
//! ├── market/daily/       不复权日 K（按年分文件）
//! ├── market/adj_factor/  复权因子
//! ├── financial/          季频财务 + 业绩预告/快报
//! ├── macro/              宏观指标
//! ├── reports/            Markdown 报告
//! ├── context/            环境扫描背景文档（agent 生成）
//! └── sync/               增量同步状态（manifest）
//! ```

pub mod layout;
pub mod sync;

pub use layout::Layout;
pub use sync::{SyncEntry, SyncManifest, SyncStatus};
