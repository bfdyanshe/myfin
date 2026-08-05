//! Python worker staging manifest。
//!
//! staging manifest 按 dataset/source/symbol 记录一次拉取运行；
//! 它与落库后按交易日索引的 SyncManifest 是两层不同协议。

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};

use mf_datasource::Dataset;

use crate::SyncStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagingEntry {
    pub dataset: Dataset,
    pub source: String,
    pub symbol: String,
    pub rows: u64,
    pub status: SyncStatus,
    pub updated_at: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct StagingManifest {
    entries: Vec<StagingEntry>,
}

impl StagingManifest {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let mut manifest = Self::default();
        if !path.exists() {
            return Ok(manifest);
        }
        for (line_number, line) in BufReader::new(File::open(path)?).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str(&line).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("staging manifest 第 {} 行无效: {error}", line_number + 1),
                )
            })?;
            manifest.entries.push(entry);
        }
        Ok(manifest)
    }

    pub fn entries(&self) -> &[StagingEntry] {
        &self.entries
    }

    pub fn done_entries(&self) -> impl Iterator<Item = &StagingEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.status == SyncStatus::Done)
    }

    pub fn failed_entries(&self) -> impl Iterator<Item = &StagingEntry> {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.status, SyncStatus::Failed | SyncStatus::Partial))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_staging_entries_and_rejects_invalid_json() {
        let root =
            std::env::temp_dir().join(format!("mf-staging-manifest-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("manifest.jsonl");
        std::fs::write(
            &path,
            "{\"dataset\":\"daily\",\"source\":\"test\",\"symbol\":\"600519.SH\",\"rows\":1,\"status\":\"done\",\"updated_at\":\"2026-08-05T00:00:00Z\"}\n",
        )
        .unwrap();
        let manifest = StagingManifest::load(&path).unwrap();
        assert_eq!(manifest.entries().len(), 1);
        assert_eq!(manifest.done_entries().count(), 1);

        std::fs::write(&path, "{\"dataset\":\"daily\"\n").unwrap();
        assert!(StagingManifest::load(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
