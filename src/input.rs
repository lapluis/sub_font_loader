use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::archive::{self, ArchiveExtraction};

#[derive(Debug, Clone, Copy)]
pub enum InputSource {
    Directory,
    Archive,
}

impl InputSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::Archive => "archive",
        }
    }
}

pub struct PreparedInput {
    original_path: PathBuf,
    scan_root: PathBuf,
    source: InputSource,
    _extraction: Option<ArchiveExtraction>,
}

impl PreparedInput {
    pub fn original_path(&self) -> &Path {
        &self.original_path
    }

    pub fn scan_root(&self) -> &Path {
        &self.scan_root
    }

    pub fn source(&self) -> InputSource {
        self.source
    }

    pub fn extracted_to(&self) -> Option<&Path> {
        match self.source {
            InputSource::Directory => None,
            InputSource::Archive => Some(&self.scan_root),
        }
    }
}

pub fn prepare_input(path: &Path, keep_extracted: bool) -> Result<PreparedInput> {
    if path.is_dir() {
        return Ok(PreparedInput {
            original_path: path.to_path_buf(),
            scan_root: path.to_path_buf(),
            source: InputSource::Directory,
            _extraction: None,
        });
    }

    if path.is_file() {
        let mut extraction = archive::extract_to_temp(path)?;

        if keep_extracted {
            extraction.keep();
        }

        return Ok(PreparedInput {
            original_path: path.to_path_buf(),
            scan_root: extraction.root().to_path_buf(),
            source: InputSource::Archive,
            _extraction: Some(extraction),
        });
    }

    bail!(
        "input must be a directory or archive file: {}",
        path.display()
    );
}
