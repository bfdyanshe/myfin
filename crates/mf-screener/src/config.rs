//! 选股参数配置（`config/screen.toml`）。

use std::collections::BTreeMap;
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
    #[serde(default)]
    pub portfolio: PortfolioCfg,
}

/// 月频组合与成交规则。固定规则比运行时自由选择更容易复现。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioCfg {
    /// 信号日为每月最后一个交易日。
    pub signal_at_month_end: bool,
    /// 信号日后的第几个可交易日成交；默认 1 表示下一交易日。
    pub execution_lag_trading_days: u32,
    /// 单个调仓批次最多持仓数，超出按标的代码稳定排序截断。
    pub max_positions: u32,
    /// 单行业权重上限；超出的名额进入现金。
    pub max_industry_weight: f64,
    /// 单边交易成本（基点）。
    pub transaction_cost_bps: f64,
    /// 单边滑点（基点）。
    pub slippage_bps: f64,
    /// 成交额低于该值的标的不成交；0 表示不启用容量门。
    pub min_entry_amount: f64,
    /// 是否允许没有候选时持有现金。
    pub allow_cash: bool,
    /// v1 回测固定持有期，不在组合引擎中混入提前退出。
    #[serde(default)]
    pub exit_on_signal_loss: bool,
    /// 是否要求回测输入提供逐交易日停牌/涨跌停状态。
    #[serde(default)]
    pub require_trade_status: bool,
}

impl Default for PortfolioCfg {
    fn default() -> Self {
        Self {
            signal_at_month_end: true,
            execution_lag_trading_days: 1,
            max_positions: 20,
            max_industry_weight: 0.25,
            transaction_cost_bps: 5.0,
            slippage_bps: 5.0,
            min_entry_amount: 0.0,
            allow_cash: true,
            exit_on_signal_loss: false,
            require_trade_status: true,
        }
    }
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
    /// 缺少真实公告日时使用的保守偏移天数；严格流程默认拒绝近似值。
    pub ann_date_approx_days: i64,
    /// 严格回测/全市场流程是否拒绝没有真实公告日的财务快照。
    #[serde(default)]
    pub require_real_ann_date: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniverseCfg {
    /// 是否要求输入带有历史股票池快照。自动全市场流程必须开启。
    pub require_point_in_time: bool,
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
    /// 是否要求最近四个报告期均有经营现金流字段；缺失即阻断。
    #[serde(default)]
    pub require_oper_cash_flow: bool,
    /// 资产负债率上限
    pub max_debt_ratio: f64,
    /// 净资产 < 0 直接出池
    pub exclude_negative_equity: bool,
    /// 行业化负债率上限；未列出的行业使用 `max_debt_ratio`。
    #[serde(default)]
    pub industry_debt_ratio: BTreeMap<String, f64>,
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
    /// 通过主信号后至少满足的辅信号数，冻结“回升”定义。
    #[serde(default)]
    pub min_secondary_signals: u8,
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
        if self.as_of.ann_date_approx_days < 0 {
            return Err(Error::Config(
                "as_of.ann_date_approx_days 不能为负数".into(),
            ));
        }
        if self.environment.return_window_days == 0 {
            return Err(Error::Config(
                "environment.return_window_days 必须大于 0".into(),
            ));
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
            return Err(Error::Config(
                "undervalued.percentile_max 必须在 (0,1)".into(),
            ));
        }
        if !(0.0..1.0).contains(&self.recovery.percent_3m_ago_max) {
            return Err(Error::Config(
                "recovery.percent_3m_ago_max 必须在 (0,1)".into(),
            ));
        }
        if self.exclusion.max_debt_ratio <= 0.0 || self.exclusion.max_debt_ratio > 1.0 {
            return Err(Error::Config(
                "exclusion.max_debt_ratio 必须在 (0,1]".into(),
            ));
        }
        if self
            .exclusion
            .industry_debt_ratio
            .values()
            .any(|value| !value.is_finite() || *value <= 0.0 || *value > 1.0)
        {
            return Err(Error::Config(
                "exclusion.industry_debt_ratio 必须全部在 (0,1]".into(),
            ));
        }
        if self.universe.min_list_years == 0 {
            return Err(Error::Config("universe.min_list_years 必须大于 0".into()));
        }
        if !self.universe.min_market_cap.is_finite() || self.universe.min_market_cap < 0.0 {
            return Err(Error::Config(
                "universe.min_market_cap 必须是非负有限数".into(),
            ));
        }
        if !self.universe.min_avg_amount.is_finite() || self.universe.min_avg_amount < 0.0 {
            return Err(Error::Config(
                "universe.min_avg_amount 必须是非负有限数".into(),
            ));
        }
        if self.undervalued.percentile_window_days == 0 {
            return Err(Error::Config(
                "undervalued.percentile_window_days 必须大于 0".into(),
            ));
        }
        if self.recovery.momentum_days == 0 || self.recovery.ma_days == 0 {
            return Err(Error::Config(
                "recovery.momentum_days 与 ma_days 必须大于 0".into(),
            ));
        }
        if !self.recovery.volume_ratio_min.is_finite() || self.recovery.volume_ratio_min < 0.0 {
            return Err(Error::Config(
                "recovery.volume_ratio_min 必须是非负有限数".into(),
            ));
        }
        if self.recovery.min_secondary_signals > 4 {
            return Err(Error::Config(
                "recovery.min_secondary_signals 不能大于 4".into(),
            ));
        }
        if !self.portfolio.signal_at_month_end {
            return Err(Error::Config(
                "portfolio.signal_at_month_end 必须为 true".into(),
            ));
        }
        if self.portfolio.exit_on_signal_loss {
            return Err(Error::Config(
                "portfolio.exit_on_signal_loss 当前必须为 false，避免与固定持有期冲突".into(),
            ));
        }
        if self.portfolio.execution_lag_trading_days == 0 {
            return Err(Error::Config(
                "portfolio.execution_lag_trading_days 必须大于 0".into(),
            ));
        }
        if self.portfolio.max_positions == 0 {
            return Err(Error::Config("portfolio.max_positions 必须大于 0".into()));
        }
        if !(0.0..=1.0).contains(&self.portfolio.max_industry_weight)
            || self.portfolio.max_industry_weight == 0.0
        {
            return Err(Error::Config(
                "portfolio.max_industry_weight 必须在 (0,1]".into(),
            ));
        }
        for (name, value) in [
            (
                "portfolio.transaction_cost_bps",
                self.portfolio.transaction_cost_bps,
            ),
            ("portfolio.slippage_bps", self.portfolio.slippage_bps),
            (
                "portfolio.min_entry_amount",
                self.portfolio.min_entry_amount,
            ),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(Error::Config(format!("{name} 必须是非负有限数")));
            }
        }
        Ok(())
    }
}
