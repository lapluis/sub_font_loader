use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use argh::FromArgs;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;
use sub_font_loader::font::index::{FontFileIndexRecord, MetaRecord};

const SCHEMA_VERSION: u32 = 5;
const META_KEY: &str = "state";

const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const FORWARD_INDEX_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("forward_index");
const REVERSE_INDEX_TABLE: TableDefinition<&str, &str> = TableDefinition::new("reverse_index");

/// export the three redb font index tables beside the database
#[derive(Debug, FromArgs)]
struct Cli {
    /// redb database path to export
    #[argh(positional)]
    db_path: PathBuf,
}

fn main() -> Result<()> {
    let cli: Cli = argh::from_env();
    run(cli)
}

fn run(cli: Cli) -> Result<()> {
    if !cli.db_path.is_file() {
        bail!(
            "redb database path is not a file: {}",
            cli.db_path.display()
        );
    }

    let output_dir = output_dir(&cli.db_path);
    let meta_path = output_dir.join("meta.json");
    let forward_index_path = output_dir.join("forward_index.csv");
    let reverse_index_path = output_dir.join("reverse_index.csv");

    let db = Database::open(&cli.db_path)
        .with_context(|| format!("failed to open redb database {}", cli.db_path.display()))?;
    let meta = read_current_meta_record(&db)?;

    let meta_file = File::create(&meta_path)
        .with_context(|| format!("failed to create {}", meta_path.display()))?;
    export_meta_json(meta_file, &meta)
        .with_context(|| format!("failed to export {}", meta_path.display()))?;

    let forward_index_file = File::create(&forward_index_path)
        .with_context(|| format!("failed to create {}", forward_index_path.display()))?;
    let forward_rows = export_forward_index_csv(&db, forward_index_file)
        .with_context(|| format!("failed to export {}", forward_index_path.display()))?;

    let reverse_index_file = File::create(&reverse_index_path)
        .with_context(|| format!("failed to create {}", reverse_index_path.display()))?;
    let reverse_rows = export_reverse_index_csv(&db, reverse_index_file)
        .with_context(|| format!("failed to export {}", reverse_index_path.display()))?;

    println!("Exported redb font index tables:");
    println!("  {}", meta_path.display());
    println!("  {} ({forward_rows} row(s))", forward_index_path.display());
    println!("  {} ({reverse_rows} row(s))", reverse_index_path.display());

    Ok(())
}

fn output_dir(db_path: &Path) -> &Path {
    db_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn export_meta_json<W: Write>(mut writer: W, meta: &MetaRecord) -> Result<()> {
    serde_json::to_writer_pretty(&mut writer, meta)
        .context("failed to write redb font index metadata JSON")?;
    writer
        .write_all(b"\n")
        .context("failed to finish redb font index metadata JSON")?;
    Ok(())
}

fn export_forward_index_csv<W: Write>(db: &Database, writer: W) -> Result<usize> {
    let mut csv_writer = csv::Writer::from_writer(writer);
    csv_writer
        .write_record([
            "relative_path",
            "extension",
            "file_size",
            "modified_at",
            "name_count",
            "names_json",
        ])
        .context("failed to write forward index CSV header")?;

    let read_txn = db
        .begin_read()
        .context("failed to start redb font index read transaction")?;
    read_current_meta_record_from_txn(&read_txn)?;

    let forward_table = read_txn
        .open_table(FORWARD_INDEX_TABLE)
        .context("failed to open redb font index forward table")?;
    let mut row_count = 0usize;

    for entry in forward_table
        .iter()
        .context("failed to iterate redb font index forward table")?
    {
        let (key, value) = entry.context("failed to read redb font index forward row")?;
        let relative_path = key.value().to_owned();
        let record = decode::<FontFileIndexRecord>(value.value())
            .with_context(|| format!("failed to decode font index record {relative_path}"))?;
        let name_count = record.names.len();
        let names_json = serde_json::to_string(&record.names)
            .with_context(|| format!("failed to encode names for {relative_path}"))?;

        csv_writer
            .write_record([
                relative_path,
                record.extension,
                record.file_size.to_string(),
                record.modified_at.to_string(),
                name_count.to_string(),
                names_json,
            ])
            .context("failed to write forward index CSV row")?;
        row_count += 1;
    }

    csv_writer
        .flush()
        .context("failed to flush forward index CSV writer")?;
    Ok(row_count)
}

fn export_reverse_index_csv<W: Write>(db: &Database, writer: W) -> Result<usize> {
    let mut csv_writer = csv::Writer::from_writer(writer);
    csv_writer
        .write_record(["name_norm", "relative_path"])
        .context("failed to write reverse index CSV header")?;

    let read_txn = db
        .begin_read()
        .context("failed to start redb font index read transaction")?;
    read_current_meta_record_from_txn(&read_txn)?;

    let reverse_table = read_txn
        .open_table(REVERSE_INDEX_TABLE)
        .context("failed to open redb font index reverse table")?;
    let mut row_count = 0usize;

    for entry in reverse_table
        .iter()
        .context("failed to iterate redb font index reverse table")?
    {
        let (key, value) = entry.context("failed to read redb font index reverse row")?;
        let name_norm = key.value().to_owned();

        for relative_path in decode_reverse_paths(value.value()) {
            csv_writer
                .write_record([name_norm.clone(), relative_path])
                .context("failed to write reverse index CSV row")?;
            row_count += 1;
        }
    }

    csv_writer
        .flush()
        .context("failed to flush reverse index CSV writer")?;
    Ok(row_count)
}

fn read_current_meta_record(db: &Database) -> Result<MetaRecord> {
    let read_txn = db
        .begin_read()
        .context("failed to start redb font index read transaction")?;
    read_current_meta_record_from_txn(&read_txn)
}

fn read_current_meta_record_from_txn(read_txn: &redb::ReadTransaction) -> Result<MetaRecord> {
    let meta =
        read_meta_record_from_txn(read_txn)?.context("redb font index metadata is missing")?;
    if meta.schema_version != SCHEMA_VERSION {
        bail!("redb font index schema is outdated; update the font index first");
    }

    Ok(meta)
}

fn read_meta_record_from_txn(read_txn: &redb::ReadTransaction) -> Result<Option<MetaRecord>> {
    let meta_table = read_txn
        .open_table(META_TABLE)
        .context("failed to open redb font index meta table")?;
    meta_table
        .get(META_KEY)
        .context("failed to read redb font index metadata")?
        .map(|value| decode(value.value()))
        .transpose()
}

fn decode_reverse_paths(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let (value, _) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
        .context("failed to deserialize redb font index record")?;
    Ok(value)
}
