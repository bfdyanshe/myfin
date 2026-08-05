//! 数据源注册表：加载 `config/sources.yaml` 并提供校验与优先级链解析。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use mf_core::Error;

use crate::dataset::Dataset;

/// 注册表文件（相对于仓库根目录）。
pub const DEFAULT_REGISTRY_PATH: &str = "config/sources.yaml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub sources: Vec<SourceConfig>,
    /// 数据集 -> 源优先级链（越靠前越优先；同一数据集配置多个源用于自动兜底）
    pub chains: HashMap<Dataset, Chain>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub name: String,
    pub description: String,
    /// `http` = Rust 原生适配器；`python_sdk` = Python worker 适配器
    pub kind: SourceKind,
    /// 实现语言：`rust` / `python`
    pub lang: String,
    /// Python 适配器模块路径（kind=python_sdk 时必填），如 `myfin_py.sources.baostock_source`
    pub package: Option<String>,
    pub auth: Auth,
    pub rate_limit: RateLimit,
    /// 支持的数据集
    pub datasets: Vec<Dataset>,
    /// 健康检查探针（基准股）
    pub probe: Option<Probe>,
    /// 自由格式的字段映射/口径备注（AI 维护时更新）
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Http,
    PythonSdk,
}

impl SourceKind {
    pub fn label(&self) -> &'static str {
        match self {
            SourceKind::Http => "http",
            SourceKind::PythonSdk => "python",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    /// 无需鉴权
    None,
    /// token 从 `config/tokens.yaml` 或环境变量读取（不得硬编码入库）
    Token { env_var: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    /// 两次调用最小间隔（ms）
    pub min_interval_ms: u64,
    pub max_calls_per_minute: Option<u64>,
    pub max_calls_per_day: Option<u64>,
    /// 被限流后的退避时间（ms）
    pub backoff_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    /// 基准股（各源各自约定格式，如 `sh.600519` / `600519.SH` / `600519`）
    pub symbol: String,
    /// 回看窗口（交易日）
    pub lookback_days: u32,
}

/// 某数据集的优先级链。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chain {
    pub dataset: Dataset,
    /// 源名，按优先级降序（第一个为主源）
    pub order: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("注册表加载失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("注册表解析失败: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("注册表校验失败: {0}")]
    Validation(String),
}

impl Registry {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RegistryError> {
        let raw = fs::read_to_string(path)?;
        let reg: Registry = serde_yaml::from_str(&raw)?;
        reg.validate()?;
        Ok(reg)
    }

    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.version != 1 {
            return Err(RegistryError::Validation(format!(
                "不支持的注册表版本: {}（当前支持 1）",
                self.version
            )));
        }
        let names: HashMap<&str, &SourceConfig> = self.sources.iter().map(|s| (s.name.as_str(), s)).collect();
        for s in &self.sources {
            if s.name.is_empty() {
                return Err(RegistryError::Validation("存在空名字的数据源".into()));
            }
            if s.kind == SourceKind::PythonSdk && s.package.is_none() {
                return Err(RegistryError::Validation(format!(
                    "python_sdk 数据源 {} 缺少 package 字段",
                    s.name
                )));
            }
            if let Auth::Token { env_var } = &s.auth {
                if env_var.is_empty() {
                    return Err(RegistryError::Validation(format!(
                        "数据源 {} 的 token env_var 为空",
                        s.name
                    )));
                }
            }
        }
        for (dataset, chain) in &self.chains {
            if chain.dataset != *dataset {
                return Err(RegistryError::Validation(format!(
                    "chain 键 {} 与内部 dataset 字段不一致（{}）",
                    dataset, chain.dataset
                )));
            }
            if chain.order.is_empty() {
                return Err(RegistryError::Validation(format!("数据集 {dataset} 的优先级链为空")));
            }
            for name in &chain.order {
                if !names.contains_key(name.as_str()) {
                    return Err(RegistryError::Validation(format!(
                        "数据集 {dataset} 优先级链引用了未定义的数据源: {name}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// 取某数据集的优先级链。注册表不保证每个数据集都有链。
    pub fn chain(&self, dataset: Dataset) -> Option<&Chain> {
        self.chains.get(&dataset)
    }

    pub fn source(&self, name: &str) -> Option<&SourceConfig> {
        self.sources.iter().find(|s| s.name == name)
    }
}

impl From<RegistryError> for Error {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::Io(io) => Error::Io(io),
            other => Error::Config(other.to_string()),
        }
    }
}
