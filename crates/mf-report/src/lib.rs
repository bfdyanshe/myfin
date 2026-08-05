//! mf-report: Markdown 报告生成。
//!
//! M1 提供 Markdown 结构渲染器；报告内容（候选清单、数据质量页）在 M4/M5 填充。
//! 报告语法遵循 docs/strategy.md 的报告规范。

use std::fmt::Write;

/// 候选股条目（M4 填充完整字段）。
#[derive(Debug, Clone, serde::Serialize)]
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
    pub env_tags: Vec<String>,
    /// 风险旗标
    pub risk_flags: Vec<String>,
    /// 入选理由（可读文本）
    pub rationale: String,
}

/// 数据源健康状态（报告数据质量页用）。
#[derive(Debug, Clone, serde::Serialize)]
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
        Self { title: title.into(), sections: Vec::new() }
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
        let _ = write!(s, "|{}|\n", header.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
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
            vec![
                c.symbol.clone(),
                c.name.clone(),
                c.industry.clone().unwrap_or_default(),
                c.pe_percentile.map(|v| format!("{:.1}%", v * 100.0)).unwrap_or_default(),
                c.pb_percentile.map(|v| format!("{:.1}%", v * 100.0)).unwrap_or_default(),
                c.earnings_turnaround_yoy.map(|v| format!("{:.1}%", v)).unwrap_or_default(),
                c.momentum_3m_pct.map(|v| format!("{:.1}%", v)).unwrap_or_default(),
                c.env_tags.join("; "),
                c.rationale.clone(),
            ]
        })
        .collect()
}
