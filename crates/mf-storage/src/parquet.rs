//! Parquet 文件存储与 DuckDB 查询。
//!
//! Python worker 负责生成符合统一 schema 的 Parquet staging 文件；本模块负责
//! 将 staging 文件接管到数据目录，并为筛选、回测提供统一的 DuckDB 查询入口。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};
use duckdb::{params, Connection};
use mf_core::financial::NoticeKind;
use mf_core::{AdjFactor, DailyBar, EarningsNotice, PriceVal};
use mf_datasource::Dataset;
use thiserror::Error;

use crate::Layout;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("存储 IO 失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("DuckDB 操作失败: {0}")]
    DuckDb(#[from] duckdb::Error),
    #[error("无效存储参数: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// 按统一目录布局写入和查询 Parquet 数据。
#[derive(Debug, Clone)]
pub struct ParquetStore {
    layout: Layout,
}

impl ParquetStore {
    pub fn new(layout: Layout) -> Self {
        Self { layout }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// 执行一个 DuckDB 查询。查询中可使用 `read_parquet(...)` 读取本地文件。
    pub fn with_connection<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> duckdb::Result<T>,
    {
        let connection = Connection::open_in_memory()?;
        Ok(f(&connection)?)
    }

    /// 将 Python staging Parquet 重新编码并原子接管到数据目录。
    ///
    /// `partition` 使用四位年份；不分年的数据集传入 `all`。`file_stem` 只能
    /// 是单个文件名组件，避免把外部输入写到数据根目录之外。
    pub fn ingest_parquet(
        &self,
        dataset: Dataset,
        partition: &str,
        file_stem: &str,
        staging: &Path,
    ) -> Result<PathBuf> {
        if !staging.is_file() {
            return Err(StorageError::Invalid(format!(
                "staging 文件不存在: {}",
                staging.display()
            )));
        }
        let dir = self.partition_dir(dataset, partition)?;
        fs::create_dir_all(&dir)?;
        let file_stem = safe_component(file_stem)?;
        let target = dir.join(format!("{file_stem}.parquet"));
        let temporary = temporary_path(&target);
        let source = sql_literal(staging);
        let connection = Connection::open_in_memory()?;
        validate_schema(&connection, dataset, &source)?;
        let query = format!("SELECT * FROM read_parquet({source})");
        if let Err(error) = copy_query_to_file(&connection, &query, &temporary) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        replace_atomically(&temporary, &target)?;
        Ok(target)
    }

    pub fn write_daily_bars(&self, rows: &[DailyBar]) -> Result<Vec<PathBuf>> {
        let mut groups: BTreeMap<(i32, String), Vec<&DailyBar>> = BTreeMap::new();
        for row in rows {
            groups
                .entry((row.trade_date.year(), row.source.clone()))
                .or_default()
                .push(row);
        }
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TEMP TABLE rows (
                symbol VARCHAR NOT NULL,
                trade_date DATE NOT NULL,
                open DOUBLE NOT NULL,
                high DOUBLE NOT NULL,
                low DOUBLE NOT NULL,
                close DOUBLE NOT NULL,
                volume DOUBLE NOT NULL,
                amount DOUBLE NOT NULL,
                source VARCHAR NOT NULL
            )",
        )?;
        let mut paths = Vec::new();
        for ((year, source), group) in groups {
            connection.execute_batch("DELETE FROM rows")?;
            for row in group {
                connection.execute(
                    "INSERT INTO rows VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        row.symbol,
                        row.trade_date,
                        row.open,
                        row.high,
                        row.low,
                        row.close,
                        row.volume,
                        row.amount,
                        row.source,
                    ],
                )?;
            }
            let target = self.target_path(Dataset::Daily, &year.to_string(), &source)?;
            write_group(
                &connection,
                &target,
                "SELECT symbol, trade_date, open, high, low, close, volume, amount, source
                 FROM (
                    SELECT *, 0 AS _priority FROM read_parquet({old})
                    UNION ALL
                    SELECT *, 1 AS _priority FROM rows
                 )
                 QUALIFY row_number() OVER (PARTITION BY symbol, trade_date ORDER BY _priority DESC) = 1
                 ORDER BY symbol, trade_date",
                "SELECT symbol, trade_date, open, high, low, close, volume, amount, source
                 FROM rows ORDER BY symbol, trade_date",
            )?;
            paths.push(target);
        }
        Ok(paths)
    }

    pub fn write_adj_factors(&self, rows: &[AdjFactor]) -> Result<Vec<PathBuf>> {
        let mut groups: BTreeMap<String, Vec<&AdjFactor>> = BTreeMap::new();
        for row in rows {
            groups.entry(row.source.clone()).or_default().push(row);
        }
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TEMP TABLE rows (
                symbol VARCHAR NOT NULL,
                ex_date DATE NOT NULL,
                cum_factor DOUBLE NOT NULL,
                source VARCHAR NOT NULL
            )",
        )?;
        let mut paths = Vec::new();
        for (source, group) in groups {
            connection.execute_batch("DELETE FROM rows")?;
            for row in group {
                connection.execute(
                    "INSERT INTO rows VALUES (?, ?, ?, ?)",
                    params![row.symbol, row.ex_date, row.cum_factor, row.source],
                )?;
            }
            let target = self.target_path(Dataset::AdjFactor, "all", &source)?;
            write_group(
                &connection,
                &target,
                "SELECT symbol, ex_date, cum_factor, source
                 FROM (
                    SELECT *, 0 AS _priority FROM read_parquet({old})
                    UNION ALL
                    SELECT *, 1 AS _priority FROM rows
                 )
                 QUALIFY row_number() OVER (PARTITION BY symbol, ex_date ORDER BY _priority DESC) = 1
                 ORDER BY symbol, ex_date",
                "SELECT symbol, ex_date, cum_factor, source FROM rows ORDER BY symbol, ex_date",
            )?;
            paths.push(target);
        }
        Ok(paths)
    }

    pub fn write_price_vals(&self, rows: &[PriceVal]) -> Result<Vec<PathBuf>> {
        let mut groups: BTreeMap<(i32, String), Vec<&PriceVal>> = BTreeMap::new();
        for row in rows {
            groups
                .entry((row.trade_date.year(), row.source.clone()))
                .or_default()
                .push(row);
        }
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TEMP TABLE rows (
                symbol VARCHAR NOT NULL,
                trade_date DATE NOT NULL,
                close DOUBLE NOT NULL,
                total_shares DOUBLE NOT NULL,
                float_shares DOUBLE NOT NULL,
                source VARCHAR NOT NULL
            )",
        )?;
        let mut paths = Vec::new();
        for ((year, source), group) in groups {
            connection.execute_batch("DELETE FROM rows")?;
            for row in group {
                connection.execute(
                    "INSERT INTO rows VALUES (?, ?, ?, ?, ?, ?)",
                    params![
                        row.symbol,
                        row.trade_date,
                        row.close,
                        row.total_shares,
                        row.float_shares,
                        row.source,
                    ],
                )?;
            }
            let target = self.target_path(Dataset::PriceVal, &year.to_string(), &source)?;
            write_group(
                &connection,
                &target,
                "SELECT symbol, trade_date, close, total_shares, float_shares, source
                 FROM (
                    SELECT *, 0 AS _priority FROM read_parquet({old})
                    UNION ALL
                    SELECT *, 1 AS _priority FROM rows
                 )
                 QUALIFY row_number() OVER (PARTITION BY symbol, trade_date ORDER BY _priority DESC) = 1
                 ORDER BY symbol, trade_date",
                "SELECT symbol, trade_date, close, total_shares, float_shares, source
                 FROM rows ORDER BY symbol, trade_date",
            )?;
            paths.push(target);
        }
        Ok(paths)
    }

    pub fn write_earnings_notices(&self, rows: &[EarningsNotice]) -> Result<Vec<PathBuf>> {
        let mut groups: BTreeMap<(i32, String), Vec<&EarningsNotice>> = BTreeMap::new();
        for row in rows {
            groups
                .entry((row.report_period.year(), row.source.clone()))
                .or_default()
                .push(row);
        }
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TEMP TABLE rows (
                symbol VARCHAR NOT NULL,
                ann_date DATE NOT NULL,
                report_period DATE NOT NULL,
                kind VARCHAR NOT NULL,
                net_profit DOUBLE,
                net_profit_yoy DOUBLE,
                source VARCHAR NOT NULL
            )",
        )?;
        let mut paths = Vec::new();
        for ((year, source), group) in groups {
            connection.execute_batch("DELETE FROM rows")?;
            for row in group {
                connection.execute(
                    "INSERT INTO rows VALUES (?, ?, ?, ?, ?, ?, ?)",
                    params![
                        row.symbol,
                        row.ann_date,
                        row.report_period,
                        notice_kind(row.kind),
                        row.net_profit,
                        row.net_profit_yoy,
                        row.source,
                    ],
                )?;
            }
            let target = self.target_path(Dataset::EarningsNotice, &year.to_string(), &source)?;
            write_group(
                &connection,
                &target,
                "SELECT symbol, ann_date, report_period, kind, net_profit, net_profit_yoy, source
                 FROM (
                    SELECT *, 0 AS _priority FROM read_parquet({old})
                    UNION ALL
                    SELECT *, 1 AS _priority FROM rows
                 )
                 QUALIFY row_number() OVER (
                    PARTITION BY symbol, ann_date, report_period, kind
                    ORDER BY _priority DESC
                 ) = 1
                 ORDER BY symbol, ann_date, report_period",
                "SELECT symbol, ann_date, report_period, kind, net_profit, net_profit_yoy, source
                 FROM rows ORDER BY symbol, ann_date, report_period",
            )?;
            paths.push(target);
        }
        Ok(paths)
    }

    /// 查询指定标的的不复权日线；过滤条件均为交易日闭区间。
    pub fn query_daily_bars(
        &self,
        symbol: &str,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
    ) -> Result<Vec<DailyBar>> {
        let files = self.parquet_files(Dataset::Daily)?;
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let source = parquet_list(&files);
        let mut sql = format!(
            "SELECT symbol, trade_date, open, high, low, close, volume, amount, source
             FROM read_parquet({source}) WHERE symbol = ?"
        );
        match (start, end) {
            (Some(_), Some(_)) => sql.push_str(" AND trade_date >= ? AND trade_date <= ?"),
            (Some(_), None) => sql.push_str(" AND trade_date >= ?"),
            (None, Some(_)) => sql.push_str(" AND trade_date <= ?"),
            (None, None) => {}
        }
        sql.push_str(" ORDER BY trade_date, source");
        let connection = Connection::open_in_memory()?;
        let mut statement = connection.prepare(&sql)?;
        let rows = match (start, end) {
            (Some(start), Some(end)) => {
                statement.query_map(params![symbol, start, end], daily_bar)?
            }
            (Some(start), None) => statement.query_map(params![symbol, start], daily_bar)?,
            (None, Some(end)) => statement.query_map(params![symbol, end], daily_bar)?,
            (None, None) => statement.query_map(params![symbol], daily_bar)?,
        };
        Ok(rows.collect::<duckdb::Result<Vec<_>>>()?)
    }

    pub fn row_count(&self, dataset: Dataset) -> Result<u64> {
        let files = self.parquet_files(dataset)?;
        if files.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            "SELECT count(*) FROM read_parquet({})",
            parquet_list(&files)
        );
        let connection = Connection::open_in_memory()?;
        Ok(connection.query_row(&sql, [], |row| row.get(0))?)
    }

    fn target_path(&self, dataset: Dataset, partition: &str, source: &str) -> Result<PathBuf> {
        let dir = self.partition_dir(dataset, partition)?;
        fs::create_dir_all(&dir)?;
        Ok(dir.join(format!("{}.parquet", safe_component(source)?)))
    }

    fn partition_dir(&self, dataset: Dataset, partition: &str) -> Result<PathBuf> {
        if partition == "all" {
            return Ok(self.layout.dataset_dir(dataset));
        }
        if partition.len() != 4 || partition.parse::<i32>().is_err() {
            return Err(StorageError::Invalid(format!(
                "分区必须是四位年份或 all: {partition}"
            )));
        }
        Ok(self
            .layout
            .dataset_year_dir(dataset, partition.parse().unwrap()))
    }

    fn parquet_files(&self, dataset: Dataset) -> Result<Vec<PathBuf>> {
        let root = self.layout.dataset_dir(dataset);
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        collect_parquet_files(&root, &mut files)?;
        files.sort();
        Ok(files)
    }
}

fn write_group(
    connection: &Connection,
    target: &Path,
    merge_query: &str,
    fresh_query: &str,
) -> Result<()> {
    let temporary = temporary_path(target);
    let query = if target.exists() {
        merge_query.replace("{old}", &sql_literal(target))
    } else {
        fresh_query.to_string()
    };
    if let Err(error) = copy_query_to_file(connection, &query, &temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    replace_atomically(&temporary, target)
}

fn copy_query_to_file(connection: &Connection, query: &str, destination: &Path) -> Result<()> {
    let sql = format!(
        "COPY ({query}) TO {} (FORMAT PARQUET, COMPRESSION ZSTD)",
        sql_literal(destination)
    );
    connection.execute_batch(&sql)?;
    Ok(())
}

fn validate_schema(connection: &Connection, dataset: Dataset, source: &str) -> Result<()> {
    let Some(expected) = expected_columns(dataset) else {
        return Ok(());
    };
    let sql = format!("DESCRIBE SELECT * FROM read_parquet({source})");
    let mut statement = connection.prepare(&sql)?;
    let actual = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let missing = expected
        .iter()
        .filter(|column| !actual.iter().any(|name| name == **column))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(StorageError::Invalid(format!(
            "{} Parquet 缺少字段: {}",
            dataset,
            missing.join(", ")
        )));
    }
    Ok(())
}

fn expected_columns(dataset: Dataset) -> Option<&'static [&'static str]> {
    match dataset {
        Dataset::Daily => Some(&[
            "symbol",
            "trade_date",
            "open",
            "high",
            "low",
            "close",
            "volume",
            "amount",
            "source",
        ]),
        Dataset::AdjFactor => Some(&["symbol", "ex_date", "cum_factor", "source"]),
        Dataset::Financial => Some(&["symbol", "report_period", "ann_date", "fields", "source"]),
        Dataset::EarningsNotice => Some(&[
            "symbol",
            "ann_date",
            "report_period",
            "kind",
            "net_profit",
            "net_profit_yoy",
            "source",
        ]),
        Dataset::PriceVal => Some(&[
            "symbol",
            "trade_date",
            "close",
            "total_shares",
            "float_shares",
            "source",
        ]),
        Dataset::Macro => None,
    }
}

fn replace_atomically(temporary: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        fs::rename(temporary, target)?;
        return Ok(());
    }
    let backup = target.with_extension(format!("parquet.backup-{}", unique_suffix()));
    fs::rename(target, &backup)?;
    if let Err(error) = fs::rename(temporary, target) {
        let _ = fs::rename(&backup, target);
        return Err(error.into());
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

fn collect_parquet_files(root: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_parquet_files(&path, output)?;
        } else if path.extension().is_some_and(|ext| ext == "parquet") {
            output.push(path);
        }
    }
    Ok(())
}

fn daily_bar(row: &duckdb::Row<'_>) -> duckdb::Result<DailyBar> {
    Ok(DailyBar {
        symbol: row.get(0)?,
        trade_date: row.get(1)?,
        open: row.get(2)?,
        high: row.get(3)?,
        low: row.get(4)?,
        close: row.get(5)?,
        volume: row.get(6)?,
        amount: row.get(7)?,
        source: row.get(8)?,
    })
}

fn notice_kind(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::Forecast => "forecast",
        NoticeKind::Express => "express",
    }
}

fn safe_component(value: &str) -> Result<&str> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains(':')
    {
        return Err(StorageError::Invalid(format!("非法文件名组件: {value}")));
    }
    Ok(value)
}

fn sql_literal(path: &Path) -> String {
    let value = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "''");
    format!("'{value}'")
}

fn parquet_list(files: &[PathBuf]) -> String {
    let paths = files
        .iter()
        .map(|path| sql_literal(path))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{paths}]")
}

fn temporary_path(target: &Path) -> PathBuf {
    target.with_extension(format!("parquet.tmp-{}", unique_suffix()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_layout() -> Layout {
        let root = std::env::temp_dir().join(format!("mf-parquet-test-{}", unique_suffix()));
        Layout::new(root)
    }

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn writes_merges_and_queries_daily_bars() {
        let layout = temp_layout();
        let store = ParquetStore::new(layout.clone());
        let first = DailyBar {
            symbol: "600519.SH".into(),
            trade_date: date("2026-08-03"),
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            volume: 10.0,
            amount: 20.0,
            source: "test".into(),
        };
        let mut updated = first.clone();
        updated.close = 1.7;
        let next = DailyBar {
            trade_date: date("2026-08-04"),
            ..first.clone()
        };
        store.write_daily_bars(&[first]).unwrap();
        store.write_daily_bars(&[updated, next]).unwrap();

        let rows = store
            .query_daily_bars("600519.SH", Some(date("2026-08-03")), None)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].close, 1.7);
        assert_eq!(store.row_count(Dataset::Daily).unwrap(), 2);
        std::fs::remove_dir_all(layout.root).unwrap();
    }

    #[test]
    fn ingests_staging_parquet() {
        let layout = temp_layout();
        let store = ParquetStore::new(layout.clone());
        let staging = layout.root.join("staging.parquet");
        std::fs::create_dir_all(&layout.root).unwrap();
        store
            .with_connection(|connection| {
                let sql = format!(
                    "CREATE TABLE rows (
                        symbol VARCHAR, trade_date DATE, open DOUBLE, high DOUBLE,
                        low DOUBLE, close DOUBLE, volume DOUBLE, amount DOUBLE, source VARCHAR
                     );
                     INSERT INTO rows VALUES (
                        '000001.SZ', DATE '2026-08-05', 9.0, 11.0, 8.0, 10.0,
                        100.0, 1000.0, 'test'
                     );
                     COPY rows TO {} (FORMAT PARQUET)",
                    sql_literal(&staging)
                );
                connection.execute_batch(&sql)
            })
            .unwrap();
        let target = store
            .ingest_parquet(Dataset::Daily, "2026", "test", &staging)
            .unwrap();
        assert!(target.is_file());
        assert_eq!(store.row_count(Dataset::Daily).unwrap(), 1);
        std::fs::remove_dir_all(layout.root).unwrap();
    }
}
