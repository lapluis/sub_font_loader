use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::archive::{self, ArchiveExtraction};

pub struct PreparedInput {
    scan_root: PathBuf,
    _extraction: Option<ArchiveExtraction>,
}

impl PreparedInput {
    pub fn scan_root(&self) -> &Path {
        &self.scan_root
    }
}

pub fn prepare_input(path: &Path, keep_extracted: bool) -> Result<PreparedInput> {
    if path.is_dir() {
        return Ok(PreparedInput {
            scan_root: path.to_path_buf(),
            _extraction: None,
        });
    }

    if path.is_file() {
        let mut extraction = archive::extract_to_temp(path)?;

        if keep_extracted {
            extraction.keep();
        }

        return Ok(PreparedInput {
            scan_root: extraction.root().to_path_buf(),
            _extraction: Some(extraction),
        });
    }

    bail!(
        "input must be a directory or archive file: {}",
        path.display()
    );
}
