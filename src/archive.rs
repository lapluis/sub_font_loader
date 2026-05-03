use std::{
    fs::{self, File},
    io::{self, BufWriter},
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use tempfile::{Builder, TempDir};
use zip::ZipArchive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveType {
    Zip,
    SevenZip,
    Rar,
}

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

pub fn detect_archive_type(path: &Path) -> Result<ArchiveType> {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        bail!("unsupported archive type for {}", path.display());
    };

    match extension.to_ascii_lowercase().as_str() {
        "zip" => Ok(ArchiveType::Zip),
        "7z" => Ok(ArchiveType::SevenZip),
        "rar" => Ok(ArchiveType::Rar),
        _ => bail!(
            "unsupported archive type for {}; only .zip, .7z, and .rar are supported",
            path.display()
        ),
    }
}

pub fn extract_archive_to_temp(archive: &Path) -> Result<TempDir> {
    let archive_type = detect_archive_type(archive)?;
    let temp_dir = Builder::new()
        .prefix("sub-font-loader-")
        .tempdir()
        .context("failed to create temporary extraction directory")?;

    match archive_type {
        ArchiveType::Zip => extract_zip(archive, temp_dir.path())?,
        ArchiveType::SevenZip => extract_7z(archive, temp_dir.path())?,
        ArchiveType::Rar => extract_rar(archive, temp_dir.path())?,
    }

    Ok(temp_dir)
}

pub fn extract_to_temp(archive: &Path) -> Result<ArchiveExtraction> {
    let temp_dir = extract_archive_to_temp(archive)?;

    Ok(ArchiveExtraction {
        root: temp_dir.path().to_path_buf(),
        temp_dir: Some(temp_dir),
    })
}

pub fn extract_zip(archive: &Path, destination: &Path) -> Result<()> {
    let source = File::open(archive)
        .with_context(|| format!("failed to open ZIP archive {}", archive.display()))?;
    let mut archive_reader = ZipArchive::new(source)
        .with_context(|| format!("failed to read ZIP archive {}", archive.display()))?;

    for index in 0..archive_reader.len() {
        let mut entry = archive_reader
            .by_index(index)
            .with_context(|| format!("failed to read ZIP entry #{index}"))?;
        let entry_name = entry.name().to_string();
        let Some(relative_path) = entry.enclosed_name() else {
            bail!("ZIP entry has an unsafe path: {entry_name}");
        };
        let output_path = destination.join(relative_path);

        if entry.is_dir() {
            fs::create_dir_all(&output_path).with_context(|| {
                format!("failed to create ZIP directory {}", output_path.display())
            })?;
            continue;
        }

        create_parent_dirs(&output_path)
            .with_context(|| format!("failed to prepare ZIP entry {}", output_path.display()))?;
        let mut output = File::create(&output_path).with_context(|| {
            format!(
                "failed to create extracted ZIP file {}",
                output_path.display()
            )
        })?;
        io::copy(&mut entry, &mut output).with_context(|| {
            format!(
                "failed to extract ZIP entry {entry_name} to {}",
                output_path.display()
            )
        })?;
    }

    Ok(())
}

pub fn extract_7z(archive: &Path, destination: &Path) -> Result<()> {
    sevenz_rust2::decompress_file_with_extract_fn(archive, destination, |entry, reader, _| {
        let output_path = safe_destination_io(destination, Path::new(entry.name()))?;

        if entry.is_directory() {
            fs::create_dir_all(&output_path).map_err(|error| {
                io_error_with_context(
                    error,
                    format!("failed to create 7z directory {}", output_path.display()),
                )
            })?;
            return Ok(true);
        }

        create_parent_dirs_io(&output_path)?;
        let output = File::create(&output_path).map_err(|error| {
            io_error_with_context(
                error,
                format!(
                    "failed to create extracted 7z file {}",
                    output_path.display()
                ),
            )
        })?;
        let mut output = BufWriter::new(output);
        io::copy(reader, &mut output).map_err(|error| {
            io_error_with_context(
                error,
                format!(
                    "failed to extract 7z entry {} to {}",
                    entry.name(),
                    output_path.display()
                ),
            )
        })?;

        Ok(true)
    })
    .with_context(|| format!("failed to extract 7z archive {}", archive.display()))
}

pub fn extract_rar(archive: &Path, destination: &Path) -> Result<()> {
    let mut cursor = unrar_ng::Archive::new(archive)
        .open_for_processing()
        .with_context(|| format!("failed to open RAR archive {}", archive.display()))?;

    while let Some(entry_cursor) = cursor
        .read_header()
        .with_context(|| format!("failed to read RAR header from {}", archive.display()))?
    {
        let entry_name = entry_cursor.entry().filename.clone();
        let is_directory = entry_cursor.entry().is_directory();
        let output_path = safe_destination(destination, &entry_name)?;

        if is_directory {
            fs::create_dir_all(&output_path).with_context(|| {
                format!("failed to create RAR directory {}", output_path.display())
            })?;
            cursor = entry_cursor.skip().with_context(|| {
                format!(
                    "failed to skip RAR directory entry {}",
                    entry_name.display()
                )
            })?;
            continue;
        }

        create_parent_dirs(&output_path)
            .with_context(|| format!("failed to prepare RAR entry {}", output_path.display()))?;
        let (data, next_cursor) = entry_cursor
            .read()
            .with_context(|| format!("failed to extract RAR entry {}", entry_name.display()))?;
        fs::write(&output_path, data).with_context(|| {
            format!(
                "failed to write extracted RAR file {}",
                output_path.display()
            )
        })?;
        cursor = next_cursor;
    }

    Ok(())
}

fn safe_destination(root: &Path, entry_path: &Path) -> Result<PathBuf> {
    let relative_path = safe_relative_path(entry_path)?;
    let output_path = root.join(relative_path);

    if !output_path.starts_with(root) {
        bail!(
            "archive entry would escape extraction directory: {}",
            entry_path.display()
        );
    }

    Ok(output_path)
}

fn safe_relative_path(path: &Path) -> Result<PathBuf> {
    let mut relative_path = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(value) => relative_path.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                bail!("archive entry has an unsafe path: {}", path.display());
            }
        }
    }

    if relative_path.as_os_str().is_empty() {
        bail!("archive entry has an empty path");
    }

    Ok(relative_path)
}

fn safe_destination_io(root: &Path, entry_path: &Path) -> io::Result<PathBuf> {
    safe_destination(root, entry_path)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{error:#}")))
}

fn create_parent_dirs(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    Ok(())
}

fn create_parent_dirs_io(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            io_error_with_context(
                error,
                format!("failed to create directory {}", parent.display()),
            )
        })?;
    }

    Ok(())
}

fn io_error_with_context(error: io::Error, context: String) -> io::Error {
    io::Error::new(error.kind(), format!("{context}: {error}"))
}
