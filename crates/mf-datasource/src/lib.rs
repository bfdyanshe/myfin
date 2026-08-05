//! mf-datasource: 数据源抽象、注册表与优先级链。
//!
//! 注册表定义见 `config/sources.toml`。**AI 通过该文件 + 本 crate 的校验逻辑
//! 维护数据源**（增删源、调整优先级链、补字段映射），修改后运行
//! `mfctl sources check` 验证。规范见 `docs/data-sources.md` 与
//! `.agents/skills/data-source-maintenance/`。

pub mod dataset;
pub mod registry;
pub mod source;

pub use dataset::{Dataset, DatasetSpec};
pub use registry::{Auth, Chain, RateLimit, Registry, RegistryError, SourceConfig, SourceKind};
pub use source::{HealthReport, Source, SourceCapabilities};

/// 默认注册表路径（仓库根相对路径）。
pub const DEFAULT_REGISTRY_PATH: &str = "config/sources.toml";
