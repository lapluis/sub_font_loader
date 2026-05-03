use std::path::{Path, PathBuf};

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
