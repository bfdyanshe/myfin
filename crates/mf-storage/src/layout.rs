//! 数据目录布局。

use std::path::{Path, PathBuf};

use mf_datasource::Dataset;

/// 默认数据根目录（可用环境变量 `MYFIN_DATA` 覆盖）。
pub const DEFAULT_DATA_DIR: &str = "data";

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new(DEFAULT_DATA_DIR)
    }
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 数据根目录：优先取 `MYFIN_DATA` 环境变量。
    pub fn from_env() -> Self {
        match std::env::var("MYFIN_DATA") {
            Ok(dir) if !dir.is_empty() => Self::new(dir),
            _ => Self::default(),
        }
    }

    /// 确保全部子目录存在。
    pub fn ensure(&self) -> std::io::Result<()> {
        for ds in Dataset::ALL {
            std::fs::create_dir_all(self.dataset_dir(ds))?;
        }
        std::fs::create_dir_all(self.reports_dir())?;
        std::fs::create_dir_all(self.context_dir())?;
        std::fs::create_dir_all(self.sync_dir())?;
        Ok(())
    }

    pub fn dataset_dir(&self, dataset: Dataset) -> PathBuf {
        self.root.join(dataset.dir())
    }

    /// 数据集按年分文件的目录。
    pub fn dataset_year_dir(&self, dataset: Dataset, year: i32) -> PathBuf {
        self.dataset_dir(dataset).join(year.to_string())
    }

    pub fn reports_dir(&self) -> PathBuf {
        self.root.join("reports")
    }

    pub fn context_dir(&self) -> PathBuf {
        self.root.join("context")
    }

    /// 环境扫描结构化结果路径。
    pub fn context_path(&self, name: &str) -> PathBuf {
        self.context_dir().join(name)
    }

    pub fn sync_dir(&self) -> PathBuf {
        self.root.join("sync")
    }

    /// 数据集 manifest 文件路径。
    pub fn manifest_path(&self, dataset: Dataset) -> PathBuf {
        self.sync_dir().join(format!("{}.jsonl", dataset.as_str()))
    }

    /// 报告文件路径。
    pub fn report_path(&self, name: &str) -> PathBuf {
        self.reports_dir().join(name)
    }

    pub fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_creates_all_dirs() {
        let tmp = std::env::temp_dir().join(format!("mf-layout-test-{}", std::process::id()));
        let layout = Layout::new(&tmp);
        layout.ensure().unwrap();
        for ds in Dataset::ALL {
            assert!(layout.dataset_dir(ds).is_dir());
        }
        assert!(layout.reports_dir().is_dir());
        assert!(layout.sync_dir().is_dir());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
