//! 流水线阶段定义（M4 填充实现）。

/// 各阶段输入输出（M4 定义具体类型）：
/// - universe: Vec<Symbol> → 候选池
/// - environment: 行业标签 + context 文档引用
/// - undervalued: 估值分位（全市场 + 行业内）
/// - exclude_bad: 财务健康旗标
/// - recovery: 回升信号评分
/// - output: 候选清单

use std::fmt;

/// 流水线阶段序号。
pub enum PipelineStage {
    Universe,
    Environment,
    Undervalued,
    ExcludeBad,
    Recovery,
    Output,
}

impl PipelineStage {
    pub fn label(&self) -> &'static str {
        match self {
            PipelineStage::Universe => "universe",
            PipelineStage::Environment => "environment",
            PipelineStage::Undervalued => "undervalued",
            PipelineStage::ExcludeBad => "exclude_bad",
            PipelineStage::Recovery => "recovery",
            PipelineStage::Output => "output",
        }
    }
}

impl fmt::Display for PipelineStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}
