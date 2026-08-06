//! mf-report: Markdown 报告生成。
//!
//! M1 提供 Markdown 结构渲染器；报告内容（候选清单、数据质量页）在 M4/M5 填充。
//! 报告语法遵循 docs/strategy.md 的报告规范。

use std::fmt::Write;

use mf_backtest::BacktestReport;
use mf_core::EnvironmentSummary;

/// 候选股条目（M4 填充完整字段）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Candidate {
    pub symbol: String,
    pub name: String,
    /// 行业
    pub industry: Option<String>,
    /// 全市场 PE 分位
    pub pe_percentile: Option<f64>,
    /// 全市场 PB 分位
    pub pb_percentile: Option<f64>,
    /// 行业内 PB 分位
    pub pb_industry_percentile: Option<f64>,
    /// 业绩拐点（预告/快报同比%）
    pub earnings_turnaround_yoy: Option<f64>,
    /// 3 个月收益（%）
    pub momentum_3m_pct: Option<f64>,
    /// 环境归因标签（由数据计算，agent 只润色）
    #[serde(default)]
    pub env_tags: Vec<String>,
    /// 环境扫描的结构化结果；用于审计标签来源。
    #[serde(default)]
    pub environment: Option<EnvironmentSummary>,
    /// 风险旗标
    pub risk_flags: Vec<String>,
    /// 入选理由（可读文本）
    pub rationale: String,
}

/// 数据源健康状态（报告数据质量页用）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceHealthLine {
    pub source: String,
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

/// Markdown 渲染器。
pub struct MarkdownReport {
    title: String,
    sections: Vec<String>,
}

impl MarkdownReport {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            sections: Vec::new(),
        }
    }

    pub fn add_heading(&mut self, text: &str, level: u8) {
        let mut s = String::new();
        let _ = write!(s, "{} {}\n\n", "#".repeat(level as usize), text);
        self.sections.push(s);
    }

    pub fn add_text(&mut self, text: &str) {
        self.sections.push(format!("{}\n\n", text.trim_end()));
    }

    /// 渲染表格。`header` 与每行等长。
    pub fn add_table(&mut self, header: &[&str], rows: &[Vec<String>]) {
        let mut s = String::new();
        let _ = write!(s, "| {} |\n", header.join(" | "));
        let _ = write!(
            s,
            "|{}|\n",
            header.iter().map(|_| "---").collect::<Vec<_>>().join("|")
        );
        for row in rows {
            let _ = write!(s, "| {} |\n", row.join(" | "));
        }
        s.push('\n');
        self.sections.push(s);
    }

    pub fn add_code_block(&mut self, lang: &str, code: &str) {
        self.sections.push(format!("```{lang}\n{code}\n```\n\n"));
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# {}", self.title);
        out.push('\n');
        for s in &self.sections {
            out.push_str(s);
        }
        out
    }
}

/// 候选清单表格（与报告规范中的字段一致）。
pub fn candidate_table(candidates: &[Candidate]) -> Vec<Vec<String>> {
    candidates
        .iter()
        .map(|c| {
            let env_tags = if c.env_tags.is_empty() {
                c.environment
                    .as_ref()
                    .map(|environment| environment.tags.clone())
                    .unwrap_or_default()
            } else {
                c.env_tags.clone()
            };
            vec![
                c.symbol.clone(),
                c.name.clone(),
                c.industry.clone().unwrap_or_default(),
                c.pe_percentile
                    .map(|v| format!("{:.1}%", v * 100.0))
                    .unwrap_or_default(),
                c.pb_percentile
                    .map(|v| format!("{:.1}%", v * 100.0))
                    .unwrap_or_default(),
                c.earnings_turnaround_yoy
                    .map(|v| format!("{:.1}%", v))
                    .unwrap_or_default(),
                c.momentum_3m_pct
                    .map(|v| format!("{:.1}%", v))
                    .unwrap_or_default(),
                env_tags.join("; "),
                c.risk_flags.join("; "),
                c.rationale.clone(),
            ]
        })
        .collect()
}

/// 渲染候选清单和数据质量页。
pub fn candidate_markdown(input: &ReportInput) -> String {
    let mut markdown = MarkdownReport::new("myfin 选股报告");
    markdown.add_heading("候选清单", 2);
    markdown.add_table(
        &[
            "标的",
            "名称",
            "行业",
            "PE 分位",
            "PB 分位",
            "业绩拐点",
            "3 个月动量",
            "环境标签",
            "风险旗标",
            "入选理由",
        ],
        &candidate_table(&input.candidates),
    );
    markdown.add_heading("数据质量", 2);
    let health_rows = input
        .source_health
        .iter()
        .map(|line| {
            vec![
                line.source.clone(),
                if line.ok { "通过" } else { "失败" }.to_string(),
                line.latency_ms
                    .map(|value| format!("{value} ms"))
                    .unwrap_or_else(|| "—".to_string()),
                line.error.clone().unwrap_or_else(|| "—".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    markdown.add_table(&["数据源", "状态", "延迟", "错误"], &health_rows);
    if input.source_health.is_empty() {
        markdown.add_text("未提供数据源健康检查结果，本报告不将其视为通过。");
    }
    for note in &input.quality_notes {
        markdown.add_text(note);
    }
    markdown.render()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReportInput {
    pub candidates: Vec<Candidate>,
    pub source_health: Vec<SourceHealthLine>,
    pub quality_notes: Vec<String>,
}

/// 渲染月度截面回测报告，保留默认参数、逐年分层和敏感性网格。
pub fn backtest_markdown(report: &BacktestReport) -> String {
    let mut markdown = MarkdownReport::new("myfin 月度截面回测");
    markdown.add_text(&format!(
        "截面区间：{} 至 {}；固定持有 {} 个月；评估快照 {} 个；入选 {} 次。",
        report
            .start
            .map(|date| date.to_string())
            .unwrap_or_else(|| "无".to_string()),
        report
            .end
            .map(|date| date.to_string())
            .unwrap_or_else(|| "无".to_string()),
        report.hold_months,
        report.evaluated_snapshots,
        report.selected
    ));
    markdown.add_heading("默认参数结果", 2);
    markdown.add_table(
        &["样本数", "平均收益", "中位数收益", "胜率"],
        &[vec![
            report.completed.count.to_string(),
            format_stats(report.completed.mean_pct),
            format_stats(report.completed.median_pct),
            format_rate(report.completed.win_rate),
        ]],
    );
    markdown.add_heading("按年份分层", 2);
    let yearly = report
        .yearly
        .iter()
        .map(|summary| {
            vec![
                summary.year.to_string(),
                summary.selected.to_string(),
                summary.completed.count.to_string(),
                format_stats(summary.completed.mean_pct),
                format_stats(summary.completed.median_pct),
                format_rate(summary.completed.win_rate),
            ]
        })
        .collect::<Vec<_>>();
    markdown.add_table(
        &[
            "年份",
            "入选次数",
            "完成数",
            "平均收益",
            "中位数收益",
            "胜率",
        ],
        &yearly,
    );
    markdown.add_heading("敏感性网格", 2);
    let sensitivity = report
        .sensitivity
        .iter()
        .map(|cell| {
            vec![
                format!("{:.0}%", cell.percentile_max * 100.0),
                cell.momentum_days.to_string(),
                cell.ma_days.to_string(),
                cell.selected.to_string(),
                cell.completed.count.to_string(),
                format_stats(cell.completed.mean_pct),
                format_stats(cell.completed.median_pct),
                format_rate(cell.completed.win_rate),
            ]
        })
        .collect::<Vec<_>>();
    markdown.add_table(
        &[
            "估值分位阈值",
            "动量天数",
            "均线天数",
            "入选次数",
            "完成数",
            "平均收益",
            "中位数收益",
            "胜率",
        ],
        &sensitivity,
    );
    markdown.add_heading("组合与样本外结果", 2);
    let portfolio = &report.portfolio;
    markdown.add_table(
        &["指标", "结果"],
        &[
            vec![
                "组合月均收益（成本后）".to_string(),
                format_stats(portfolio.mean_monthly_return_pct),
            ],
            vec![
                "最大回撤".to_string(),
                format_stats(portfolio.max_drawdown_pct),
            ],
            vec![
                "年化波动率".to_string(),
                format_stats(portfolio.annualized_volatility_pct),
            ],
            vec![
                "累计换手".to_string(),
                format!("{:.1}%", portfolio.turnover_pct),
            ],
            vec![
                "平均持仓数".to_string(),
                format!("{:.1}", portfolio.average_holdings),
            ],
            vec![
                "平均现金比例".to_string(),
                format!("{:.1}%", portfolio.average_cash_pct),
            ],
        ],
    );
    let exposures = portfolio
        .industry_exposure_pct
        .iter()
        .map(|(industry, weight)| format!("{industry}: {weight:.1}%"))
        .collect::<Vec<_>>();
    if !exposures.is_empty() {
        markdown.add_text(&format!("平均行业暴露：{}。", exposures.join("；")));
    }
    markdown.add_heading("价格/估值信号相关性", 3);
    let correlations = report
        .factor_correlations
        .iter()
        .map(|item| {
            vec![
                item.left.clone(),
                item.right.clone(),
                item.pearson
                    .map(|value| format!("{value:.3}"))
                    .unwrap_or_else(|| "—".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    markdown.add_table(&["因子 A", "因子 B", "Pearson 相关"], &correlations);
    if let Some(oos) = &report.out_of_sample {
        markdown.add_text(&format!(
            "样本外交易收益：样本数 {}，平均 {}，中位数 {}，胜率 {}。",
            oos.count,
            format_stats(oos.mean_pct),
            format_stats(oos.median_pct),
            format_rate(oos.win_rate)
        ));
    } else {
        markdown.add_text("样本外结果不可用：历史截面少于 4 个月或没有完成交易。");
    }
    markdown.add_heading("关键假设消融", 3);
    let ablations = report
        .ablations
        .iter()
        .map(|item| {
            vec![
                item.name.clone(),
                item.selected.to_string(),
                item.completed.count.to_string(),
                format_stats(item.completed.mean_pct),
                format_stats(item.portfolio.max_drawdown_pct),
            ]
        })
        .collect::<Vec<_>>();
    markdown.add_table(
        &["变体", "入选", "完成", "平均收益", "最大回撤"],
        &ablations,
    );
    markdown.add_heading("数据质量说明", 2);
    markdown.add_text(
        "本报告使用统一 as-of 规则；财务披露日为免费数据源的近似值，\
         退出日不足六个月的数据不计入完成收益。敏感性网格仅用于诊断，\
         不会自动修改 config/screen.toml。",
    );
    markdown.render()
}

fn format_stats(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}%"))
        .unwrap_or_else(|| "—".to_string())
}

fn format_rate(value: Option<f64>) -> String {
    value
        .map(|value| format!("{:.1}%", value * 100.0))
        .unwrap_or_else(|| "—".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_quality_page_without_health_data() {
        let markdown = candidate_markdown(&ReportInput {
            candidates: Vec::new(),
            source_health: Vec::new(),
            quality_notes: vec!["测试说明".to_string()],
        });
        assert!(markdown.contains("## 候选清单"));
        assert!(markdown.contains("## 数据质量"));
        assert!(markdown.contains("不将其视为通过"));
        assert!(markdown.contains("测试说明"));
    }

    #[test]
    fn renders_tags_from_structured_environment_summary() {
        let markdown = candidate_markdown(&ReportInput {
            candidates: vec![Candidate {
                symbol: "600519.SH".to_string(),
                name: "测试标的".to_string(),
                industry: Some("食品饮料".to_string()),
                pe_percentile: None,
                pb_percentile: None,
                pb_industry_percentile: None,
                earnings_turnaround_yoy: None,
                momentum_3m_pct: None,
                env_tags: Vec::new(),
                environment: Some(EnvironmentSummary {
                    industry: "食品饮料".to_string(),
                    as_of: chrono::NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
                    return_window_days: 126,
                    member_count: 3,
                    valid_return_members: 3,
                    industry_return: Some(-0.1),
                    market_return: Some(0.0),
                    relative_return: Some(-0.1),
                    valid_profit_members: 3,
                    profit_trend_share: Some(2.0 / 3.0),
                    tags: vec!["industry_earnings_turning".to_string()],
                }),
                risk_flags: Vec::new(),
                rationale: "测试".to_string(),
            }],
            source_health: Vec::new(),
            quality_notes: Vec::new(),
        });
        assert!(markdown.contains("industry_earnings_turning"));
    }
}
