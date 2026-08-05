//! mfctl: myfin 命令行入口。
//!
//! 子命令：
//! - `sources`   数据源注册表状态 / 健康检查（AI 维护数据源的核心入口）
//! - `sync`      增量同步行情/财务数据（M3）
//! - `screen`    运行选股流水线（M4）
//! - `report`    生成 Markdown 报告（M4/M5）
//! - `doctor`    数据目录健康审计
//! - `verify`    跨源抽查对账（M3/M4）
//! - `backtest`  历史月度截面重建回测（M4）

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use mf_datasource::{Registry, SourceKind, DEFAULT_REGISTRY_PATH};
use mf_storage::Layout;

#[derive(Parser)]
#[command(
    name = "mfctl",
    version,
    about = "myfin 个人量化选股工具：当前低估且正在回升的标的",
    long_about = "myfin — 个人量化选股。理念：当前低估但正在回升的标的，6 个月持有期，不做择时。\n数据源注册表与优先级链由 AI 通过 config/sources.yaml + skills 维护。"
)]
struct Cli {
    /// 数据源注册表路径
    #[arg(long, global = true, default_value = DEFAULT_REGISTRY_PATH)]
    registry: PathBuf,

    /// 数据根目录（默认 $MYFIN_DATA 或 data/）
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 数据源注册表操作
    Sources {
        #[command(subcommand)]
        action: SourcesAction,
    },
    /// 增量同步数据（M3 实现）
    Sync,
    /// 运行选股流水线（M4 实现）
    Screen,
    /// 生成 Markdown 报告（M4/M5 实现）
    Report,
    /// 数据目录健康审计
    Doctor,
    /// 跨源抽查对账（M3/M4 实现）
    Verify,
    /// 历史月度截面重建回测（M4 实现）
    Backtest,
}

#[derive(Subcommand)]
enum SourcesAction {
    /// 列出注册表中的数据源与优先级链
    List,
    /// 对全部数据源做健康检查（基准股探针）
    Check,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let registry = Registry::load(&cli.registry)
        .with_context(|| format!("加载数据源注册表 {} 失败", cli.registry.display()))?;

    let layout = match &cli.data_dir {
        Some(d) => Layout::new(d),
        None => Layout::from_env(),
    };
    layout.ensure().context("初始化数据目录失败")?;

    match cli.command {
        Command::Sources { action } => match action {
            SourcesAction::List => cmd_sources_list(&registry),
            SourcesAction::Check => cmd_sources_check(&registry),
        },
        Command::Sync => cmd_pending("sync", "增量同步数据（M3 实现）"),
        Command::Screen => cmd_pending("screen", "选股流水线（M4 实现）"),
        Command::Report => cmd_pending("report", "Markdown 报告（M4/M5 实现）"),
        Command::Doctor => cmd_doctor(&layout),
        Command::Verify => cmd_pending("verify", "跨源对账（M3/M4 实现）"),
        Command::Backtest => cmd_pending("backtest", "历史截面回测（M4 实现）"),
    }
}

fn cmd_sources_list(registry: &Registry) -> Result<()> {
    println!("数据源注册表 v{} ({} 个源)", registry.version, registry.sources.len());
    println!();
    for s in &registry.sources {
        println!("- {:<12} {:6} {:10} datasets: {}", s.name, kind_label(s.kind), s.lang, s.datasets.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(","));
        if let Some(notes) = &s.notes {
            println!("    notes: {notes}");
        }
    }
    println!();
    println!("优先级链:");
    for (dataset, chain) in &registry.chains {
        println!("  {:<16} {}", dataset.as_str(), chain.order.join(" -> "));
    }
    Ok(())
}

fn kind_label(kind: SourceKind) -> &'static str {
    kind.label()
}

fn cmd_sources_check(_registry: &Registry) -> Result<()> {
    // M3: 实例化全部适配器，逐源跑基准股探针，输出健康报告 + 交叉校验。
    println!("健康检查将在 M3（数据源适配器落地）实现。当前注册表配置有效，共 {} 个源。", _registry.sources.len());
    Ok(())
}

fn cmd_doctor(layout: &Layout) -> Result<()> {
    println!("数据目录: {}", layout.root.display());
    let (dirs, files) = layout_summary(layout)?;
    println!("子目录: {} 个，文件: {} 个", dirs, files);
    println!("报告目录: {}", layout.reports_dir().display());
    println!("环境上下文: {}", layout.context_dir().display());
    Ok(())
}

fn layout_summary(layout: &Layout) -> Result<(usize, usize)> {
    let mut dirs = 0;
    let mut files = 0;
    fn walk(path: &std::path::Path, dirs: &mut usize, files: &mut usize) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.path().is_dir() {
                *dirs += 1;
                walk(&entry.path(), dirs, files)?;
            } else {
                *files += 1;
            }
        }
        Ok(())
    }
    walk(&layout.root, &mut dirs, &mut files)?;
    Ok((dirs, files))
}

fn cmd_pending(name: &str, msg: &str) -> Result<()> {
    println!("`mfctl {name}` 尚未实现 — {msg}");
    Ok(())
}
