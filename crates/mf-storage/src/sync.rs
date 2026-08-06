//! 增量同步状态机（manifest）。
//!
//! 每个数据集一个 manifest（JSONL 追加式，`data/sync/<dataset>.jsonl`），
//! 记录 (source, trade_date) 级别的同步状态，支持断点续跑与缺口检测。
//! 单人维护项目最容易烂尾的就是增量同步，必须以状态机显式管理。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use mf_datasource::Dataset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    /// 成功
    Done,
    /// 部分成功（如个别股票缺失）
    Partial,
    /// 失败（需重试或人工排查）
    Failed,
    /// 跳过（停牌、非交易日、数据源无此交易日数据）
    Skipped,
}

/// 同步状态条目。键为 (dataset, source, trade_date)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEntry {
    pub dataset: Dataset,
    pub source: String,
    pub trade_date: NaiveDate,
    pub status: SyncStatus,
    /// 写入行数
    pub rows: u64,
    /// 完成时间（UTC RFC3339）
    pub updated_at: String,
    /// 备注（失败原因等）
    #[serde(default)]
    pub note: Option<String>,
}

impl SyncEntry {
    pub fn done(dataset: Dataset, source: &str, trade_date: NaiveDate, rows: u64) -> Self {
        Self {
            dataset,
            source: source.to_string(),
            trade_date,
            status: SyncStatus::Done,
            rows,
            updated_at: chrono::Utc::now().to_rfc3339(),
            note: None,
        }
    }
}

/// 内存中的 manifest 视图：加载后按 (source, trade_date) 索引。
#[derive(Debug, Default, Clone)]
pub struct SyncManifest {
    /// dataset -> source -> date -> status
    index: HashMap<Dataset, HashMap<String, HashMap<NaiveDate, SyncEntry>>>,
}

impl SyncManifest {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let mut m = SyncManifest::default();
        if !path.exists() {
            return Ok(m);
        }
        let file = File::open(path)?;
        for (line_number, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str::<SyncEntry>(&line).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("manifest 第 {} 行无效: {error}", line_number + 1),
                )
            })?;
            m.index
                .entry(entry.dataset)
                .or_default()
                .entry(entry.source.clone())
                .or_default()
                .insert(entry.trade_date, entry);
        }
        Ok(m)
    }

    /// 追加一条状态并同步到磁盘。
    pub fn record(&mut self, path: &Path, entry: SyncEntry) -> std::io::Result<()> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{}", serde_json::to_string(&entry).unwrap())?;
        file.sync_all()?;
        self.index
            .entry(entry.dataset)
            .or_default()
            .entry(entry.source.clone())
            .or_default()
            .insert(entry.trade_date, entry);
        Ok(())
    }

    /// 某源在截止日前缺失的交易日（用于缺口检测与重试）。
    pub fn missing_dates(
        &self,
        dataset: Dataset,
        source: &str,
        expected: &[NaiveDate],
    ) -> Vec<NaiveDate> {
        let Some(by_source) = self.index.get(&dataset) else {
            return expected.to_vec();
        };
        let Some(by_date) = by_source.get(source) else {
            return expected.to_vec();
        };
        expected
            .iter()
            .filter(|d| {
                !by_date
                    .get(d)
                    .is_some_and(|entry| entry.status == SyncStatus::Done)
            })
            .copied()
            .collect()
    }

    pub fn status(&self, dataset: Dataset, source: &str, date: NaiveDate) -> Option<&SyncEntry> {
        self.index.get(&dataset)?.get(source)?.get(&date)
    }

    pub fn failed_entries(&self) -> Vec<&SyncEntry> {
        self.index
            .values()
            .flat_map(|by_source| by_source.values())
            .flat_map(|by_date| by_date.values())
            .filter(|e| matches!(e.status, SyncStatus::Failed | SyncStatus::Partial))
            .collect()
    }

    pub fn blocking_entries(&self) -> Vec<&SyncEntry> {
        self.failed_entries()
    }

    /// 检测同一源连续记录中的极端行数变化，避免半截返回被当成完整数据。
    pub fn row_count_anomalies(&self, ratio: f64) -> Vec<&SyncEntry> {
        let mut anomalies = Vec::new();
        for by_source in self.index.values() {
            for by_date in by_source.values() {
                let mut counts = by_date
                    .values()
                    .filter_map(|entry| {
                        (entry.status == SyncStatus::Done && entry.rows > 0).then_some(entry.rows)
                    })
                    .collect::<Vec<_>>();
                if counts.len() < 5 {
                    continue;
                }
                counts.sort_unstable();
                let median = counts[counts.len() / 2] as f64;
                for entry in by_date.values() {
                    let value = entry.rows as f64;
                    if entry.status == SyncStatus::Done
                        && value > 0.0
                        && (value > median * ratio || value * ratio < median)
                    {
                        anomalies.push(entry);
                    }
                }
            }
        }
        anomalies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_missing() {
        let tmp = std::env::temp_dir().join(format!("mf-sync-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("daily.jsonl");
        let mut m = SyncManifest::load(&path).unwrap();
        let d1 = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2026, 8, 4).unwrap();
        m.record(&path, SyncEntry::done(Dataset::Daily, "baostock", d1, 5400))
            .unwrap();
        let expected = vec![d1, d2];
        let missing = m.missing_dates(Dataset::Daily, "baostock", &expected);
        assert_eq!(missing, vec![d2]);

        let reloaded = SyncManifest::load(&path).unwrap();
        assert!(reloaded.status(Dataset::Daily, "baostock", d1).is_some());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn rejects_invalid_manifest_lines() {
        let tmp = std::env::temp_dir().join(format!("mf-sync-invalid-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("daily.jsonl");
        std::fs::write(&path, "{\"dataset\":\"daily\"\n").unwrap();
        assert!(SyncManifest::load(&path).is_err());
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
