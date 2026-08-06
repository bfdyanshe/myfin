//! 选股参数配置（`config/screen.toml`）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use mf_core::Error;

pub const DEFAULT_SCREEN_CONFIG: &str = "config/screen.toml";

/// 流水线各阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Universe,
    Environment,
    Undervalued,
    ExcludeBad,
    Recovery,
    Output,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenerConfig {
    pub as_of: AsOfCfg,
    pub universe: UniverseCfg,
    #[serde(default)]
    pub environment: EnvironmentCfg,
    pub undervalued: UndervaluedCfg,
    pub exclusion: ExclusionCfg,
    pub recovery: RecoveryCfg,
}

/// 环境归因参数；标签只用于解释和排序，不作为买卖开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentCfg {
    /// 行情收益使用的交易日观察窗口。
    pub return_window_days: u32,
    /// 盈利趋势比较的连续报告期数量；4 表示 TTM。
    pub profit_trend_quarters: u32,
    /// 生成行业标签所需的最小成员数。
    pub min_members: u32,
    /// 行业内 TTM 净利润改善成员占比达到该值时标记为盈利拐点。
    pub earnings_turning_min_share: f64,
}

impl Default for EnvironmentCfg {
    fn default() -> Self {
        Self {
            return_window_days: 126,
            profit_trend_quarters: 4,
            min_members: 3,
            earnings_turning_min_share: 0.5,
        }
    }
}

/// as-of 日期：所有因子只使用该日及之前可知的数据（防前视偏差）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsOfCfg {
    /// 报告期末 + 约 2 个月作为财务近似披露时点（免费源无公告日期）
    pub ann_date_approx_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniverseCfg {
    /// 排除北交所（免费数据源不支持）
    pub exclude_bse: bool,
    /// 排除 ST/*ST（名称含 ST 或退市风险旗标）
    pub exclude_st: bool,
    /// 上市最短年限
    pub min_list_years: u32,
    /// 市值下限（元）
    pub min_market_cap: f64,
    /// 最小日均成交额（元）
    pub min_avg_amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndervaluedCfg {
    /// 估值分位窗口（交易日），固定 5 年
    pub percentile_window_days: u32,
    /// 分位阈值（< 该值入选）
    pub percentile_max: f64,
    /// 同时计算行业内分位（行业中性化，避免整池集中于单一风格）
    pub use_industry_percentile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExclusionCfg {
    /// 连续亏损季数上限（TTM 净利为负）
    pub max_consecutive_loss_quarters: u32,
    /// 经营现金流为负的最大季数
    pub max_neg_cashflow_quarters: u32,
    /// 资产负债率上限
    pub max_debt_ratio: f64,
    /// 净资产 < 0 直接出池
    pub exclude_negative_equity: bool,
}

/// 回升确认（右则确认）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCfg {
    /// 业绩拐点为主信号：预告/快报净利同比转正
    pub require_earnings_turnaround: bool,
    /// 分位回升：当前分位 < max 且 3 个月前分位 < 前值阈值
    pub percent_3m_ago_max: f64,
    /// 3 个月动量 > 0
    pub momentum_days: u32,
    /// 收盘 > MA 周期
    pub ma_days: u32,
    /// 量能回升：近 20 日均额 / 前 60 日均额下限
    pub volume_ratio_min: f64,
}

impl ScreenerConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path_ref = path.as_ref();
        let raw = std::fs::read_to_string(path_ref)
            .map_err(|e| Error::Config(format!("读取 {} 失败: {e}", path_ref.display())))?;
        let cfg: ScreenerConfig = toml::from_str(&raw)
            .map_err(|e| Error::Config(format!("解析 {} 失败: {e}", path_ref.display())))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.environment.return_window_days == 0 {
            return Err(Error::Config("environment.return_window_days 必须大于 0".into()));
        }
        if self.environment.profit_trend_quarters == 0 {
            return Err(Error::Config(
                "environment.profit_trend_quarters 必须大于 0".into(),
            ));
        }
        if self.environment.min_members == 0 {
            return Err(Error::Config("environment.min_members 必须大于 0".into()));
        }
        if !(0.0..=1.0).contains(&self.environment.earnings_turning_min_share) {
            return Err(Error::Config(
                "environment.earnings_turning_min_share 必须在 [0,1]".into(),
            ));
        }
        if !(0.0..1.0).contains(&self.undervalued.percentile_max) {
            return Err(Error::Config("undervalued.percentile_max 必须在 (0,1)".into()));
        }
        if !(0.0..1.0).contains(&self.recovery.percent_3m_ago_max) {
            return Err(Error::Config("recovery.percent_3m_ago_max 必须在 (0,1)".into()));
        }
        if self.exclusion.max_debt_ratio <= 0.0 || self.exclusion.max_debt_ratio > 1.0 {
            return Err(Error::Config("exclusion.max_debt_ratio 必须在 (0,1]".into()));
        }
        Ok(())
    }
}
