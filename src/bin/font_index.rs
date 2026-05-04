use std::{collections::BTreeSet, fs::File, path::PathBuf};

use anyhow::{Context, Result};
use argh::FromArgs;
use sub_font_loader::{
    discover,
    font::index::{FontIndex, FontMatch, ResolveReport, ScanSummary},
    subtitle,
};

/// build and query a local subtitle font index
#[derive(Debug, FromArgs)]
struct Cli {
    /// command to run
    #[argh(subcommand)]
    command: Command,
}

#[derive(Debug, FromArgs)]
#[argh(subcommand)]
enum Command {
    Scan(ScanCommand),
    Query(QueryCommand),
    ResolveSubtitles(ResolveSubtitlesCommand),
    ExportCsv(ExportCsvCommand),
}

/// scan a font directory into the index database
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "scan")]
struct ScanCommand {
    /// font directory to scan
    #[argh(positional)]
    font_dir: PathBuf,

    /// SQLite database path; defaults to font_index.sqlite
    #[argh(option)]
    db: Option<PathBuf>,
}

/// query the index for one font name
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "query")]
struct QueryCommand {
    /// font name or alias to query
    #[argh(positional)]
    font_name: String,

    /// SQLite database path; defaults to font_index.sqlite
    #[argh(option)]
    db: Option<PathBuf>,
}

/// resolve fonts required by ASS/SSA subtitles
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "resolve-subtitles")]
struct ResolveSubtitlesCommand {
    /// subtitle file or directory to scan
    #[argh(positional)]
    subtitle_dir: PathBuf,

    /// SQLite database path; defaults to font_index.sqlite
    #[argh(option)]
    db: Option<PathBuf>,
}

/// export indexed aliases to a CSV file
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "export-csv")]
struct ExportCsvCommand {
    /// CSV path to write
    #[argh(positional)]
    csv_path: PathBuf,

    /// SQLite database path; defaults to font_index.sqlite
    #[argh(option)]
    db: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli: Cli = argh::from_env();

    match cli.command {
        Command::Scan(command) => scan(command),
        Command::Query(command) => query(command),
        Command::ResolveSubtitles(command) => resolve_subtitles(command),
        Command::ExportCsv(command) => export_csv(command),
    }
}

fn scan(command: ScanCommand) -> Result<()> {
    let db_path = db_path(command.db);
    let mut index = FontIndex::open(&db_path)?;
    let summary = index.scan_root(&command.font_dir)?;

    print_scan_summary(&summary);
    Ok(())
}

fn query(command: QueryCommand) -> Result<()> {
    let db_path = db_path(command.db);
    let index = FontIndex::open(&db_path)?;
    let matches = index.query_alias(&command.font_name)?;

    if matches.is_empty() {
        println!("No matches for: {}", command.font_name);
        return Ok(());
    }

    print_query_matches(&matches);
    Ok(())
}

fn resolve_subtitles(command: ResolveSubtitlesCommand) -> Result<()> {
    let subtitles =
        discover::discover_subtitle_paths([&command.subtitle_dir]).with_context(|| {
            format!(
                "failed to discover subtitles in {}",
                command.subtitle_dir.display()
            )
        })?;

    if subtitles.is_empty() {
        eprintln!(
            "No supported subtitle files (.ass, .ssa) found in {}.",
            command.subtitle_dir.display()
        );
        return Ok(());
    }

    let report = subtitle::analyze_subtitle_font_report(&subtitles)?;
    let index = FontIndex::open(&db_path(command.db))?;
    let resolved = index.resolve_required_fonts(&report.required_fonts)?;

    println!("Found {} subtitle file(s).", subtitles.len());
    print_set("Required fonts", &report.required_fonts);
    print_resolve_report(&resolved);
    Ok(())
}

fn export_csv(command: ExportCsvCommand) -> Result<()> {
    let db_path = db_path(command.db);
    let index = FontIndex::open(&db_path)?;
    let file = File::create(&command.csv_path)
        .with_context(|| format!("failed to create CSV file {}", command.csv_path.display()))?;

    index
        .export_aliases_csv(file)
        .with_context(|| format!("failed to export CSV file {}", command.csv_path.display()))?;
    println!("CSV written to: {}", command.csv_path.display());
    Ok(())
}

fn db_path(value: Option<PathBuf>) -> PathBuf {
    value.unwrap_or_else(|| PathBuf::from("font_index.sqlite"))
}

fn print_scan_summary(summary: &ScanSummary) {
    println!("Indexed root: {}", summary.root.display());
    println!("Scanned files: {}", summary.scanned_files);
    println!("Indexed files: {}", summary.indexed_files);
    println!(
        "Skipped unchanged files: {}",
        summary.skipped_unchanged_files
    );
    println!("Failed files: {}", summary.failed_files);
    println!("Marked unavailable: {}", summary.unavailable_files);
}

fn print_query_matches(matches: &[FontMatch]) {
    for font_match in matches {
        println!("Requested: {}", font_match.requested_name);
        println!("Matched alias: {}", font_match.matched_alias);
        println!("Alias kind: {}", font_match.alias_kind);
        println!("Face index: {}", font_match.face_index);
        println!("Path: {}", font_match.font_path.display());
        println!();
    }
}

fn print_resolve_report(report: &ResolveReport) {
    println!();
    println!("Matched fonts:");
    if report.matched.is_empty() {
        println!("  (none)");
    } else {
        for resolved in &report.matched {
            println!("  {}", resolved.requested_name);
            for font_match in &resolved.matches {
                println!(
                    "    {} [{} face #{}] {}",
                    font_match.matched_alias,
                    font_match.alias_kind,
                    font_match.face_index,
                    font_match.font_path.display()
                );
            }
        }
    }

    println!();
    println!("Missing fonts:");
    if report.missing.is_empty() {
        println!("  (none)");
    } else {
        for font_name in &report.missing {
            println!("  {font_name}");
        }
    }

    println!();
    println!("Unique font paths to load:");
    if report.unique_font_paths.is_empty() {
        println!("  (none)");
    } else {
        for path in &report.unique_font_paths {
            println!("  {}", path.display());
        }
    }
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
