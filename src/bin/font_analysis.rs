use std::{
    collections::HashSet,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use argh::FromArgs;
use sub_font_loader::{
    discover,
    font::{self, FontFaceAnalysis, FontFileAnalysis},
};

/// analyze font name aliases and optionally export them as CSV
#[derive(Debug, FromArgs)]
struct Cli {
    /// font file or directory to scan; defaults to the current directory
    #[argh(positional)]
    input: Option<PathBuf>,

    /// scan only the top level of the input directory
    #[argh(switch)]
    no_recursive: bool,

    /// write alias CSV to this path
    #[argh(option, short = 'o')]
    csv: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli: Cli = argh::from_env();
    let input = cli.input.unwrap_or_else(|| PathBuf::from("."));
    let font_paths = discover_font_inputs(&input, !cli.no_recursive)?;

    let mut analyses = Vec::new();
    let mut failed = 0usize;

    for path in &font_paths {
        match font::analyze_font_file(path) {
            Ok(analysis) => analyses.push(analysis),
            Err(error) => {
                failed += 1;
                eprintln!("Warning: failed to analyze {}: {error:#}", path.display());
            }
        }
    }

    print_summary(
        font_paths.len(),
        analyses.len(),
        failed,
        unique_alias_count(&analyses),
    );

    if let Some(csv_path) = cli.csv {
        let file = File::create(&csv_path)
            .with_context(|| format!("failed to create CSV file {}", csv_path.display()))?;
        write_font_alias_csv(file, &analyses)
            .with_context(|| format!("failed to write CSV file {}", csv_path.display()))?;
        println!("CSV written to: {}", csv_path.display());
    } else {
        print_aliases(&analyses);
    }

    Ok(())
}

fn discover_font_inputs(input: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    if !input.exists() {
        bail!("input does not exist: {}", input.display());
    }

    if input.is_file() {
        if discover::is_supported_font(input) {
            return Ok(vec![input.to_path_buf()]);
        }

        bail!(
            "unsupported input file {}; expected .ttf, .otf, .ttc, or a directory",
            input.display()
        );
    }

    if input.is_dir() {
        return discover::discover_fonts(input, recursive)
            .with_context(|| format!("failed to discover fonts in {}", input.display()));
    }

    bail!("input is not a file or directory: {}", input.display());
}

fn print_summary(found: usize, analyzed: usize, failed: usize, aliases: usize) {
    println!("Found {found} font file(s).");
    println!("Analyzed {analyzed} font file(s).");
    println!("Failed {failed} font file(s).");
    println!("Found {aliases} unique alias(es).");
}

fn print_aliases(analyses: &[FontFileAnalysis]) {
    for analysis in analyses {
        println!();
        println!("{}", analysis.path.display());

        for face in &analysis.faces {
            println!("  face #{}", face.face_index);

            let aliases = sorted_unique_aliases(face);
            if aliases.is_empty() {
                println!("    (none)");
                continue;
            }

            for alias in aliases {
                println!("    {alias}");
            }
        }
    }
}

fn sorted_unique_aliases(face: &FontFaceAnalysis) -> Vec<&str> {
    let mut seen = HashSet::new();
    let mut aliases = Vec::new();

    for alias in &face.aliases {
        if seen.insert(alias.value.to_lowercase()) {
            aliases.push(alias.value.as_str());
        }
    }

    aliases.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    aliases
}

fn unique_alias_count(analyses: &[FontFileAnalysis]) -> usize {
    let mut aliases = HashSet::new();

    for analysis in analyses {
        for face in &analysis.faces {
            for alias in &face.aliases {
                aliases.insert(alias.value.to_lowercase());
            }
        }
    }

    aliases.len()
}

fn write_font_alias_csv<W: Write>(writer: W, analyses: &[FontFileAnalysis]) -> Result<()> {
    let mut rows = collect_csv_rows(analyses);
    rows.sort_by(|left, right| {
        left.font_path
            .cmp(&right.font_path)
            .then_with(|| left.face_index.cmp(&right.face_index))
            .then_with(|| left.alias.to_lowercase().cmp(&right.alias.to_lowercase()))
            .then_with(|| left.name_id.cmp(&right.name_id))
            .then_with(|| left.alias.cmp(&right.alias))
    });

    let mut writer = csv::Writer::from_writer(writer);
    writer
        .write_record([
            "font_path",
            "face_index",
            "name_id",
            "platform_id",
            "encoding_id",
            "language_id",
            "alias",
        ])
        .context("failed to write CSV header")?;

    for row in rows {
        writer
            .write_record([
                row.font_path,
                row.face_index.to_string(),
                row.name_id.to_string(),
                row.platform_id,
                row.encoding_id.to_string(),
                row.language_id.to_string(),
                row.alias,
            ])
            .context("failed to write CSV row")?;
    }

    writer.flush().context("failed to flush CSV writer")?;
    Ok(())
}

#[derive(Debug)]
struct CsvAliasRow {
    font_path: String,
    face_index: u32,
    name_id: u16,
    platform_id: String,
    encoding_id: u16,
    language_id: u16,
    alias: String,
}

fn collect_csv_rows(analyses: &[FontFileAnalysis]) -> Vec<CsvAliasRow> {
    let mut seen = HashSet::new();
    let mut rows = Vec::new();

    for analysis in analyses {
        let font_path = analysis.path.display().to_string();

        for face in &analysis.faces {
            for alias in &face.aliases {
                let alias_key = alias.value.to_lowercase();
                if !seen.insert((font_path.clone(), face.face_index, alias.name_id, alias_key)) {
                    continue;
                }

                rows.push(CsvAliasRow {
                    font_path: font_path.clone(),
                    face_index: face.face_index,
                    name_id: alias.name_id,
                    platform_id: alias.platform_id.clone(),
                    encoding_id: alias.encoding_id,
                    language_id: alias.language_id,
                    alias: alias.value.clone(),
                });
            }
        }
    }

    rows
}
