//! mf-screener: 选股流水线（策略规格见 docs/strategy.md，参数见 config/screen.toml）。
//!
//! 流水线各阶段（M4 实现）：
//! ① universe   剔除 ST/退市风险/上市<1年/流动性不足（MVP 排除北交所）
//! ② 环境扫描   数据化标签（行业相对收益、行业盈利趋势）+ context 文档
//! ③ 低估筛选   PE/PB 5 年历史分位 < 阈值（全市场 + 行业内双分位，后复权口径）
//! ④ 不良排除   连续亏损/经营现金流为负/负债率超限/市值下限
//! ⑤ 回升确认   业绩拐点(主) → 分位回升 → 3 个月动量 → 站上 MA120 → 量能回升
//! ⑥ 输出       候选清单 + 环境归因标签 + 风险旗标 + 数据质量页

pub mod config;
pub mod stage;

pub use config::{ExclusionCfg, RecoveryCfg, ScreenerConfig, UndervaluedCfg, UniverseCfg};
pub use stage::PipelineStage;
