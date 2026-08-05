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
use mf_core::{AdjFactor, DailyBar, EarningsNotice, FinancialData, FinancialField, PriceVal};
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
        let source = sql_literal(staging);
        let connection = Connection::open_in_memory()?;
        validate_schema(&connection, dataset, &source)?;
        let relation = format!("read_parquet({source})");
        self.ingest_relation(&connection, dataset, partition, file_stem, &relation)
    }

    /// 按数据集的日期字段拆分 staging 文件，并将每个年份幂等接管到数据目录。
    pub fn ingest_parquet_by_year(
        &self,
        dataset: Dataset,
        file_stem: &str,
        staging: &Path,
    ) -> Result<Vec<PathBuf>> {
        if !staging.is_file() {
            return Err(StorageError::Invalid(format!(
                "staging 文件不存在: {}",
                staging.display()
            )));
        }
        let source = sql_literal(staging);
        let connection = Connection::open_in_memory()?;
        validate_schema(&connection, dataset, &source)?;
        let Some(date_column) = partition_column(dataset) else {
            return Ok(vec![self.ingest_relation(
                &connection,
                dataset,
                "all",
                file_stem,
                &format!("read_parquet({source})"),
            )?]);
        };
        let year_sql = format!(
            "SELECT DISTINCT year({date_column}) FROM read_parquet({source}) \
             WHERE {date_column} IS NOT NULL ORDER BY 1"
        );
        let mut statement = connection.prepare(&year_sql)?;
        let years = statement
            .query_map([], |row| row.get::<_, i32>(0))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let mut paths = Vec::with_capacity(years.len());
        for year in years {
            let relation = format!("read_parquet({source}) WHERE year({date_column}) = {year}");
            let relation = format!("SELECT * FROM {relation}");
            paths.push(self.ingest_relation(
                &connection,
                dataset,
                &year.to_string(),
                file_stem,
                &relation,
            )?);
        }
        Ok(paths)
    }

    fn ingest_relation(
        &self,
        connection: &Connection,
        dataset: Dataset,
        partition: &str,
        file_stem: &str,
        relation: &str,
    ) -> Result<PathBuf> {
        let target = self.target_path(dataset, partition, file_stem)?;
        let query = if target.exists() {
            merge_relation_query(dataset, &target, relation)?
        } else if relation.trim_start().starts_with("SELECT ") {
            relation.to_string()
        } else {
            format!("SELECT * FROM {relation}")
        };
        let temporary = temporary_path(&target);
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

    /// 查询指定标的的股本与价格派生数据；过滤条件为交易日闭区间。
    pub fn query_price_vals(
        &self,
        symbol: &str,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
    ) -> Result<Vec<PriceVal>> {
        let files = self.parquet_files(Dataset::PriceVal)?;
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let source = parquet_list(&files);
        let mut sql = format!(
            "SELECT symbol, trade_date, close, total_shares, float_shares, source
             FROM read_parquet({source}) WHERE symbol = ?"
        );
        append_date_filter(&mut sql, "trade_date", start, end);
        sql.push_str(" ORDER BY trade_date, source");
        let connection = Connection::open_in_memory()?;
        let mut statement = connection.prepare(&sql)?;
        let rows = match (start, end) {
            (Some(start), Some(end)) => {
                statement.query_map(params![symbol, start, end], price_val)?
            }
            (Some(start), None) => statement.query_map(params![symbol, start], price_val)?,
            (None, Some(end)) => statement.query_map(params![symbol, end], price_val)?,
            (None, None) => statement.query_map(params![symbol], price_val)?,
        };
        Ok(rows.collect::<duckdb::Result<Vec<_>>>()?)
    }

    /// 查询指定标的的全部复权因子，按生效日排序。
    pub fn query_adj_factors(
        &self,
        symbol: &str,
        end: Option<NaiveDate>,
    ) -> Result<Vec<AdjFactor>> {
        let files = self.parquet_files(Dataset::AdjFactor)?;
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let mut sql = format!(
            "SELECT symbol, ex_date, cum_factor, source
             FROM read_parquet({}) WHERE symbol = ?",
            parquet_list(&files)
        );
        if end.is_some() {
            sql.push_str(" AND ex_date <= ?");
        }
        sql.push_str(" ORDER BY ex_date, source");
        let connection = Connection::open_in_memory()?;
        let mut statement = connection.prepare(&sql)?;
        let rows = match end {
            Some(end) => statement.query_map(params![symbol, end], adj_factor)?,
            None => statement.query_map(params![symbol], adj_factor)?,
        };
        Ok(rows.collect::<duckdb::Result<Vec<_>>>()?)
    }

    /// 查询指定标的在 as-of 日可见的财务快照。
    pub fn query_financial(&self, symbol: &str, as_of: NaiveDate) -> Result<Vec<FinancialData>> {
        let files = self.parquet_files(Dataset::Financial)?;
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT symbol, report_period, ann_date, to_json(fields), source
             FROM read_parquet({})
             WHERE symbol = ? AND report_period <= ? AND ann_date <= ?
             ORDER BY report_period, ann_date, source",
            parquet_list(&files)
        );
        let connection = Connection::open_in_memory()?;
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params![symbol, as_of, as_of], financial_data)?;
        Ok(rows.collect::<duckdb::Result<Vec<_>>>()?)
    }

    /// 查询指定标的在 as-of 日前已披露的业绩预告/快报。
    pub fn query_earnings_notices(
        &self,
        symbol: &str,
        as_of: NaiveDate,
    ) -> Result<Vec<EarningsNotice>> {
        let files = self.parquet_files(Dataset::EarningsNotice)?;
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT symbol, ann_date, report_period, kind, net_profit,
             net_profit_yoy, source
             FROM read_parquet({})
             WHERE symbol = ? AND ann_date <= ? AND report_period <= ?
             ORDER BY ann_date, report_period, source",
            parquet_list(&files)
        );
        let connection = Connection::open_in_memory()?;
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params![symbol, as_of, as_of], earnings_notice)?;
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

    /// 检查日线的 OHLC 和成交字段，返回总行数与异常行数。
    pub fn daily_quality(&self) -> Result<(u64, u64)> {
        let files = self.parquet_files(Dataset::Daily)?;
        if files.is_empty() {
            return Ok((0, 0));
        }
        let source = parquet_list(&files);
        let connection = Connection::open_in_memory()?;
        let total = connection.query_row(
            &format!("SELECT count(*) FROM read_parquet({source})"),
            [],
            |row| row.get(0),
        )?;
        let invalid = connection.query_row(
            &format!(
                "SELECT count(*) FROM read_parquet({source})
                 WHERE high < open OR high < close OR low > open OR low > close
                    OR volume < 0 OR amount < 0
                    OR NOT isfinite(open) OR NOT isfinite(high)
                    OR NOT isfinite(low) OR NOT isfinite(close)
                    OR NOT isfinite(volume) OR NOT isfinite(amount)"
            ),
            [],
            |row| row.get(0),
        )?;
        Ok((total, invalid))
    }

    /// 返回 staging 文件按数据集日期字段聚合的行数，供落库 manifest 使用。
    pub fn staging_date_counts(
        &self,
        dataset: Dataset,
        staging: &Path,
    ) -> Result<Vec<(NaiveDate, u64)>> {
        if !staging.is_file() {
            return Err(StorageError::Invalid(format!(
                "staging 文件不存在: {}",
                staging.display()
            )));
        }
        let Some(date_column) = manifest_date_column(dataset) else {
            return Ok(Vec::new());
        };
        let source = sql_literal(staging);
        let connection = Connection::open_in_memory()?;
        validate_schema(&connection, dataset, &source)?;
        let sql = format!(
            "SELECT {date_column}, count(*) FROM read_parquet({source}) \
             WHERE {date_column} IS NOT NULL GROUP BY 1 ORDER BY 1"
        );
        let mut statement = connection.prepare(&sql)?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))?
            .collect::<duckdb::Result<Vec<_>>>()?)
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

fn merge_relation_query(dataset: Dataset, target: &Path, relation: &str) -> Result<String> {
    let Some(columns) = expected_columns(dataset) else {
        return Ok(format!("SELECT * FROM {relation}"));
    };
    let Some(keys) = dedup_keys(dataset) else {
        return Ok(format!("SELECT * FROM {relation}"));
    };
    let columns = columns.join(", ");
    Ok(format!(
        "SELECT {columns} FROM (
            SELECT {columns}, 0 AS _priority FROM read_parquet({})
            UNION ALL
            SELECT {columns}, 1 AS _priority FROM ({relation}) AS incoming
        ) AS merged
        QUALIFY row_number() OVER (
            PARTITION BY {keys}
            ORDER BY _priority DESC
        ) = 1",
        sql_literal(target)
    ))
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

fn dedup_keys(dataset: Dataset) -> Option<&'static str> {
    match dataset {
        Dataset::Daily => Some("symbol, trade_date, source"),
        Dataset::AdjFactor => Some("symbol, ex_date, source"),
        Dataset::Financial => Some("symbol, report_period, ann_date, source"),
        Dataset::EarningsNotice => Some("symbol, ann_date, report_period, kind, source"),
        Dataset::PriceVal => Some("symbol, trade_date, source"),
        Dataset::Macro => None,
    }
}

fn partition_column(dataset: Dataset) -> Option<&'static str> {
    match dataset {
        Dataset::Daily | Dataset::PriceVal => Some("trade_date"),
        Dataset::Financial | Dataset::EarningsNotice => Some("report_period"),
        Dataset::AdjFactor | Dataset::Macro => None,
    }
}

fn manifest_date_column(dataset: Dataset) -> Option<&'static str> {
    match dataset {
        Dataset::Daily | Dataset::PriceVal => Some("trade_date"),
        Dataset::AdjFactor => Some("ex_date"),
        Dataset::Financial | Dataset::EarningsNotice => Some("report_period"),
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

fn append_date_filter(
    sql: &mut String,
    column: &str,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
) {
    match (start, end) {
        (Some(_), Some(_)) => sql.push_str(&format!(" AND {column} >= ? AND {column} <= ?")),
        (Some(_), None) => sql.push_str(&format!(" AND {column} >= ?")),
        (None, Some(_)) => sql.push_str(&format!(" AND {column} <= ?")),
        (None, None) => {}
    }
}

fn price_val(row: &duckdb::Row<'_>) -> duckdb::Result<PriceVal> {
    Ok(PriceVal {
        symbol: row.get(0)?,
        trade_date: row.get(1)?,
        close: row.get(2)?,
        total_shares: row.get(3)?,
        float_shares: row.get(4)?,
        source: row.get(5)?,
    })
}

fn adj_factor(row: &duckdb::Row<'_>) -> duckdb::Result<AdjFactor> {
    Ok(AdjFactor {
        symbol: row.get(0)?,
        ex_date: row.get(1)?,
        cum_factor: row.get(2)?,
        source: row.get(3)?,
    })
}

fn financial_data(row: &duckdb::Row<'_>) -> duckdb::Result<FinancialData> {
    let fields_json: String = row.get(3)?;
    let fields = parse_financial_fields(&fields_json)
        .map_err(|error| duckdb::Error::ToSqlConversionFailure(Box::new(error)))?;
    Ok(FinancialData {
        symbol: row.get(0)?,
        report_period: row.get(1)?,
        ann_date: row.get(2)?,
        fields,
        source: row.get(4)?,
    })
}

fn earnings_notice(row: &duckdb::Row<'_>) -> duckdb::Result<EarningsNotice> {
    let kind: String = row.get(3)?;
    let kind = match kind.as_str() {
        "forecast" => NoticeKind::Forecast,
        "express" => NoticeKind::Express,
        other => {
            return Err(duckdb::Error::ToSqlConversionFailure(Box::new(
                StorageError::Invalid(format!("未知业绩公告类型: {other}")),
            )))
        }
    };
    Ok(EarningsNotice {
        symbol: row.get(0)?,
        ann_date: row.get(1)?,
        report_period: row.get(2)?,
        kind,
        net_profit: row.get(4)?,
        net_profit_yoy: row.get(5)?,
        source: row.get(6)?,
    })
}

fn parse_financial_fields(
    raw: &str,
) -> std::result::Result<Vec<(FinancialField, f64)>, StorageError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| StorageError::Invalid(format!("财务 fields JSON 无效: {error}")))?;
    let Some(items) = value.as_array() else {
        return Err(StorageError::Invalid("财务 fields 必须是数组".into()));
    };
    items
        .iter()
        .map(|item| {
            let item = item.get("item").unwrap_or(item);
            let name = item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| StorageError::Invalid("财务字段缺少 name".into()))?;
            let number = item
                .get("value")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| StorageError::Invalid(format!("财务字段 {name} 缺少 value")))?;
            let field = financial_field(name)
                .ok_or_else(|| StorageError::Invalid(format!("未知财务字段: {name}")))?;
            Ok((field, number))
        })
        .collect()
}

fn financial_field(name: &str) -> Option<FinancialField> {
    match name {
        "revenue" => Some(FinancialField::Revenue),
        "net_profit" => Some(FinancialField::NetProfit),
        "equity" => Some(FinancialField::Equity),
        "total_assets" => Some(FinancialField::TotalAssets),
        "total_liabilities" => Some(FinancialField::TotalLiabilities),
        "oper_cash_flow" => Some(FinancialField::OperCashFlow),
        "eps" => Some(FinancialField::Eps),
        "bps" => Some(FinancialField::Bps),
        "gross_margin" => Some(FinancialField::GrossMargin),
        "roe" => Some(FinancialField::Roe),
        "debt_ratio" => Some(FinancialField::DebtRatio),
        _ => None,
    }
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
        assert_eq!(store.daily_quality().unwrap(), (2, 0));
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

    fn write_daily_staging(path: &std::path::Path, rows: &[(NaiveDate, f64)]) {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE rows (
                    symbol VARCHAR, trade_date DATE, open DOUBLE, high DOUBLE,
                    low DOUBLE, close DOUBLE, volume DOUBLE, amount DOUBLE, source VARCHAR
                )",
            )
            .unwrap();
        for (trade_date, close) in rows {
            connection
                .execute(
                    "INSERT INTO rows VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        "600519.SH",
                        trade_date,
                        close - 1.0,
                        close + 1.0,
                        close - 2.0,
                        close,
                        100.0,
                        1000.0,
                        "test",
                    ],
                )
                .unwrap();
        }
        let sql = format!("COPY rows TO {} (FORMAT PARQUET)", sql_literal(path));
        connection.execute_batch(&sql).unwrap();
    }

    #[test]
    fn partitions_and_merges_staging_by_year() {
        let layout = temp_layout();
        let store = ParquetStore::new(layout.clone());
        std::fs::create_dir_all(&layout.root).unwrap();
        let first = layout.root.join("first.parquet");
        write_daily_staging(
            &first,
            &[(date("2025-12-31"), 10.0), (date("2026-01-02"), 11.0)],
        );
        assert_eq!(
            store
                .ingest_parquet_by_year(Dataset::Daily, "test", &first)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(store.row_count(Dataset::Daily).unwrap(), 2);

        let second = layout.root.join("second.parquet");
        write_daily_staging(
            &second,
            &[(date("2026-01-02"), 12.0), (date("2026-01-03"), 13.0)],
        );
        store
            .ingest_parquet_by_year(Dataset::Daily, "test", &second)
            .unwrap();
        let rows = store
            .query_daily_bars("600519.SH", Some(date("2026-01-02")), None)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].close, 12.0);
        assert_eq!(store.row_count(Dataset::Daily).unwrap(), 3);
        std::fs::remove_dir_all(layout.root).unwrap();
    }

    #[test]
    fn parses_financial_fields_from_arrow_json() {
        let fields = parse_financial_fields(
            r#"[{"name":"net_profit","value":100.0},{"name":"debt_ratio","value":0.4}]"#,
        )
        .unwrap();
        assert_eq!(
            fields,
            vec![
                (FinancialField::NetProfit, 100.0),
                (FinancialField::DebtRatio, 0.4)
            ]
        );

        let wrapped = parse_financial_fields(r#"[{"item":{"name":"equity","value":200.0}}]"#)
            .unwrap();
        assert_eq!(wrapped, vec![(FinancialField::Equity, 200.0)]);
    }
}
