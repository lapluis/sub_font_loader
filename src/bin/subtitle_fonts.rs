use std::{collections::BTreeSet, path::PathBuf};

use anyhow::{Context, Result};
use argh::FromArgs;
use sub_font_loader::{discover, subtitle};

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
    let subtitles = if cli.no_recursive && input.is_dir() {
        discover_top_level_subtitles(&input)
            .with_context(|| format!("failed to discover subtitle files in {}", input.display()))?
    } else {
        discover::discover_subtitle_paths([&input])
            .with_context(|| format!("failed to discover subtitle files in {}", input.display()))?
    };

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
    print_set("Declared but unused", &report.declared_but_unused_fonts);

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

fn discover_top_level_subtitles(root: &PathBuf) -> Result<Vec<PathBuf>> {
    let mut subtitles = Vec::new();

    for entry in
        std::fs::read_dir(root).with_context(|| format!("failed to scan {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("failed to scan {}", root.display()))?;
        let path = entry.path();

        if path.is_file() && discover::is_supported_subtitle(&path) {
            subtitles.push(path);
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
