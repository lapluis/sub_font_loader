use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use argh::FromArgs;
use sub_font_loader::subtitle;
use walkdir::WalkDir;

/// analyze subtitle font requirements without loading fonts
#[derive(Debug, FromArgs)]
struct Cli {
    /// subtitle file or directory to scan; defaults to the current directory
    #[argh(positional)]
    input: Option<PathBuf>,

    /// scan only the top level of the input directory
    #[argh(switch)]
    no_recursive: bool,
}

fn main() -> Result<()> {
    let cli: Cli = argh::from_env();
    let input = cli.input.unwrap_or_else(|| PathBuf::from("."));
    let subtitles = discover_subtitles(&input, !cli.no_recursive)
        .with_context(|| format!("failed to discover subtitle files in {}", input.display()))?;

    if subtitles.is_empty() {
        eprintln!(
            "No supported subtitle files (.ass, .ssa) found in {}.",
            input.display()
        );
        return Ok(());
    }

    let report = subtitle::analyze_subtitle_font_report(&subtitles)?;

    println!("Found {} subtitle file(s).", subtitles.len());
    print_set("Required fonts", &report.required_fonts);

    if !report.declared_fonts.is_empty() {
        print_set("Declared fonts", &report.declared_fonts);
    }

    if !report.inline_fonts.is_empty() {
        print_set("Inline fonts", &report.inline_fonts);
    }

    if !report.missing_styles.is_empty() {
        eprintln!();
        eprintln!("Missing styles:");
        for style in &report.missing_styles {
            eprintln!("  {style}");
        }
    }

    Ok(())
}

fn is_supported_subtitle(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };

    matches!(extension.to_ascii_lowercase().as_str(), "ass" | "ssa")
}

fn discover_subtitles(root: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(if is_supported_subtitle(root) {
            vec![root.to_path_buf()]
        } else {
            Vec::new()
        });
    }

    if !root.is_dir() {
        return Ok(Vec::new());
    }

    let walker = if recursive {
        WalkDir::new(root)
    } else {
        WalkDir::new(root).max_depth(1)
    };

    let mut subtitles = Vec::new();

    for entry in walker {
        let entry = entry.with_context(|| format!("failed to scan {}", root.display()))?;

        if entry.file_type().is_file() && is_supported_subtitle(entry.path()) {
            subtitles.push(entry.path().to_path_buf());
        }
    }

    subtitles.sort();
    Ok(subtitles)
}

fn print_set(title: &str, values: &BTreeSet<String>) {
    println!();
    println!("{title}:");

    if values.is_empty() {
        println!("  (none)");
        return;
    }

    for value in values {
        println!("  {value}");
    }
}
