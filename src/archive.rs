use std::{
    fs::File,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use compress_tools::{uncompress_archive, Ownership};
use tempfile::{Builder, TempDir};

pub struct ArchiveExtraction {
    root: PathBuf,
    temp_dir: Option<TempDir>,
}

impl ArchiveExtraction {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn keep(&mut self) -> PathBuf {
        let Some(temp_dir) = self.temp_dir.take() else {
            return self.root.clone();
        };

        let path = temp_dir.keep();
        self.root = path.clone();
        path
    }
}

pub fn extract_to_temp(archive: &Path) -> Result<ArchiveExtraction> {
    let temp_dir = Builder::new()
        .prefix("sub-font-loader-")
        .tempdir()
        .context("failed to create temporary extraction directory")?;

    let mut source = File::open(archive)
        .with_context(|| format!("failed to open archive {}", archive.display()))?;

    uncompress_archive(&mut source, temp_dir.path(), Ownership::Ignore)
        .with_context(|| format!("failed to extract archive {}", archive.display()))?;

    Ok(ArchiveExtraction {
        root: temp_dir.path().to_path_buf(),
        temp_dir: Some(temp_dir),
    })
}
