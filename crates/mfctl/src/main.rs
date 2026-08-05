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

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use mf_datasource::{Dataset, Registry, SourceKind, DEFAULT_REGISTRY_PATH};
use mf_storage::{
    Layout, ParquetStore, StagingManifest, SyncEntry, SyncManifest, SyncStatus,
};

#[derive(Parser)]
#[command(
    name = "mfctl",
    version,
    about = "myfin 个人量化选股工具：当前低估且正在回升的标的",
    long_about = "myfin — 个人量化选股。理念：当前低估但正在回升的标的，6 个月持有期，不做择时。\n数据源注册表与优先级链由 AI 通过 config/sources.toml + skills 维护。"
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
    /// 增量同步一个源、数据集和标的
    Sync(SyncArgs),
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
            SourcesAction::Check => cmd_sources_check(&registry, &cli.registry),
        },
        Command::Sync(args) => cmd_sync(&registry, &cli.registry, &layout, args),
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

fn python_executable() -> std::ffi::OsString {
    if let Some(python) = std::env::var_os("MYFIN_PYTHON") {
        return python;
    }
    for candidate in ["py/.venv/Scripts/python.exe", "py/.venv/bin/python"] {
        let path = Path::new(candidate);
        if path.is_file() {
            return path.as_os_str().to_owned();
        }
    }
    std::ffi::OsString::from("python")
}

fn cmd_sources_check(registry: &Registry, registry_path: &Path) -> Result<()> {
    let mut failed = 0;
    for source in &registry.sources {
        match source.kind {
            SourceKind::Http => {
                println!(
                    "FAIL {:<12} Rust HTTP 适配器尚未实现（注册表已登记）",
                    source.name
                );
                failed += 1;
            }
            SourceKind::PythonSdk => {
                let mut command = ProcessCommand::new(python_executable());
                command.args([
                    "-m",
                    "myfin_py.worker",
                    "--registry",
                    &registry_path.to_string_lossy(),
                    "health-check",
                    "--source",
                    &source.name,
                ]);
                add_python_path(&mut command);
                match command.output() {
                    Ok(output) => {
                        print!("{}", String::from_utf8_lossy(&output.stdout));
                        eprint!("{}", String::from_utf8_lossy(&output.stderr));
                        if !output.status.success() {
                            failed += 1;
                        }
                    }
                    Err(error) => {
                        println!(
                            "FAIL {:<12} 无法启动 Python worker: {error}",
                            source.name
                        );
                        failed += 1;
                    }
                }
            }
        }
    }
    if failed > 0 {
        anyhow::bail!("{} 个数据源健康检查未通过", failed);
    }
    Ok(())
}

#[derive(Args)]
struct SyncArgs {
    /// 源名称，必须存在于 config/sources.toml
    #[arg(long)]
    source: String,

    /// 数据集名称，如 daily / financial
    #[arg(long)]
    dataset: String,

    /// 标的代码，建议使用统一格式，如 600519.SH
    #[arg(long)]
    symbol: String,

    /// 日线开始日期；不填时由 worker 使用默认窗口
    #[arg(long)]
    start: Option<String>,

    /// 日线结束日期；不填时由 worker 使用当前日期
    #[arg(long)]
    end: Option<String>,
}

fn add_python_path(command: &mut ProcessCommand) {
    let py_src = Path::new("py/src");
    if !py_src.is_dir() {
        return;
    }
    let mut paths = vec![py_src.to_path_buf()];
    if let Some(existing) = std::env::var_os("PYTHONPATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        command.env("PYTHONPATH", joined);
    }
}

fn cmd_sync(
    registry: &Registry,
    registry_path: &Path,
    layout: &Layout,
    args: SyncArgs,
) -> Result<()> {
    let source = registry
        .source(&args.source)
        .with_context(|| format!("注册表中不存在数据源: {}", args.source))?;
    if source.kind != SourceKind::PythonSdk {
        anyhow::bail!(
            "数据源 {} 的 Rust HTTP 适配器尚未实现，当前 sync 只支持 Python SDK 源",
            args.source
        );
    }
    let dataset = Dataset::from_str(&args.dataset)
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .with_context(|| format!("未知数据集: {}", args.dataset))?;
    if !source.datasets.contains(&dataset) {
        anyhow::bail!(
            "数据源 {} 未声明支持数据集 {}",
            args.source,
            dataset
        );
    }
    if !matches!(
        dataset,
        Dataset::Daily
            | Dataset::AdjFactor
            | Dataset::Financial
            | Dataset::EarningsNotice
    ) {
        anyhow::bail!(
            "Python worker 当前不支持数据集 {} 的同步接管",
            dataset
        );
    }

    let staging_dir = layout
        .root
        .join("staging")
        .join(format!("sync-{}", unique_suffix()));
    std::fs::create_dir_all(&staging_dir)?;
    let mut command = ProcessCommand::new(python_executable());
    command.args([
        "-m",
        "myfin_py.worker",
        "--registry",
        &registry_path.to_string_lossy(),
        &format!("fetch-{}", dataset),
        "--source",
        &args.source,
        "--symbol",
        &args.symbol,
        "--out",
        &staging_dir.to_string_lossy(),
    ]);
    if dataset == Dataset::Daily {
        if let Some(start) = &args.start {
            command.args(["--start", start]);
        }
        if let Some(end) = &args.end {
            command.args(["--end", end]);
        }
    } else if args.start.is_some() || args.end.is_some() {
        anyhow::bail!("只有 daily 支持 --start/--end");
    }
    add_python_path(&mut command);

    let output = command
        .output()
        .with_context(|| format!("启动 Python worker 失败（源 {}）", args.source))?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        anyhow::bail!("Python worker 执行失败，staging 保留在 {}", staging_dir.display());
    }

    let manifest_path = staging_dir.join("manifest.jsonl");
    let manifest = StagingManifest::load(&manifest_path)
        .with_context(|| format!("读取 staging manifest 失败: {}", manifest_path.display()))?;
    let matched = manifest.entries().iter().find(|entry| {
        entry.dataset == dataset
            && entry.source == args.source
            && entry.symbol == args.symbol
    });
    let Some(entry) = matched else {
        anyhow::bail!(
            "staging manifest 没有匹配记录，数据目录未接管；staging 保留在 {}",
            staging_dir.display()
        );
    };
    if entry.status != SyncStatus::Done {
        anyhow::bail!(
            "staging 状态为 {:?}，数据目录未接管；staging 保留在 {}",
            entry.status,
            staging_dir.display()
        );
    }

    let parquet_dir = staging_dir.join(dataset.as_str());
    let parquet_file = single_parquet_file(&parquet_dir)?;
    let file_stem = format!("{}-{}", safe_component(&args.source), safe_component(&args.symbol));
    let store = ParquetStore::new(layout.clone());
    let paths = store.ingest_parquet_by_year(dataset, &file_stem, &parquet_file)?;
    let date_counts = store.staging_date_counts(dataset, &parquet_file)?;
    let manifest_path = layout.manifest_path(dataset);
    let mut sync_manifest = SyncManifest::load(&manifest_path)
        .with_context(|| format!("读取同步 manifest 失败: {}", manifest_path.display()))?;
    for (date, rows) in date_counts {
        sync_manifest.record(
            &manifest_path,
            SyncEntry::done(dataset, &args.source, date, rows),
        )?;
    }
    println!(
        "已接管 {} 条记录，生成 {} 个分区文件；staging: {}",
        entry.rows,
        paths.len(),
        staging_dir.display()
    );
    for path in paths {
        println!("  {}", path.display());
    }
    Ok(())
}

fn single_parquet_file(dir: &Path) -> Result<PathBuf> {
    let mut files = std::fs::read_dir(dir)
        .with_context(|| format!("staging 数据目录不存在: {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "parquet"));
    let Some(file) = files.next() else {
        anyhow::bail!("staging 没有 Parquet 文件: {}", dir.display());
    };
    if files.next().is_some() {
        anyhow::bail!("staging 数据目录包含多个 Parquet 文件: {}", dir.display());
    }
    Ok(file)
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || matches!(char, '_' | '-') {
                char
            } else {
                '_'
            }
        })
        .collect()
}

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
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
