use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use walkdir::WalkDir;

pub fn discover_fonts(root: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let walker = if recursive {
        WalkDir::new(root)
    } else {
        WalkDir::new(root).max_depth(1)
    };

    let mut fonts = Vec::new();

    for entry in walker {
        let entry = entry.with_context(|| format!("failed to scan {}", root.display()))?;

        if entry.file_type().is_file() && is_supported_font(entry.path()) {
            fonts.push(entry.path().to_path_buf());
        }
    }

    fonts.sort();
    Ok(fonts)
}

pub fn is_supported_font(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };

    matches!(
        extension.to_ascii_lowercase().as_str(),
        "ttf" | "otf" | "ttc"
    )
}

pub fn discover_subtitle_paths<I, P>(inputs: I) -> Result<Vec<PathBuf>>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut subtitles = BTreeSet::new();

    for input in inputs {
        let input = input.as_ref();

        if !input.exists() {
            eprintln!(
                "Warning: subtitle input does not exist: {}",
                input.display()
            );
            continue;
        }

        if input.is_file() {
            if is_supported_subtitle(input) {
                subtitles.insert(input.to_path_buf());
            } else {
                eprintln!(
                    "Warning: unsupported subtitle input ignored: {}",
                    input.display()
                );
            }
            continue;
        }

        if input.is_dir() {
            discover_subtitles_in_dir(input, &mut subtitles)?;
        } else {
            eprintln!(
                "Warning: subtitle input is not a file or directory: {}",
                input.display()
            );
        }
    }

    Ok(subtitles.into_iter().collect())
}

pub fn is_supported_subtitle(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };

    matches!(extension.to_ascii_lowercase().as_str(), "ass" | "ssa")
}

fn discover_subtitles_in_dir(root: &Path, subtitles: &mut BTreeSet<PathBuf>) -> Result<()> {
    for entry in WalkDir::new(root) {
        let entry = entry.with_context(|| format!("failed to scan {}", root.display()))?;

        if entry.file_type().is_file() && is_supported_subtitle(entry.path()) {
            subtitles.insert(entry.path().to_path_buf());
        }
    }

    Ok(())
}
