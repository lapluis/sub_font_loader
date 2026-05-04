use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

use crate::discover;

use super::{FontFileAnalysis, analyze_font_file};

const SCHEMA_VERSION: u32 = 5;
const META_KEY: &str = "state";
const INDEXED_NAME_IDS: &[u16] = &[1, 2, 4, 6, 16, 17];
const REVERSE_NAME_IDS: &[u16] = &[1, 4, 6, 16];

const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const FORWARD_INDEX_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("forward_index");
const REVERSE_INDEX_TABLE: TableDefinition<&str, &str> = TableDefinition::new("reverse_index");

pub struct FontIndex {
    db: Database,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaRecord {
    pub schema_version: u32,

    /// Canonicalized absolute root path.
    pub root_path: String,

    /// Unix timestamp.
    pub scanned_at: i64,

    pub total_font_files: u64,
    pub ttf_count: u64,
    pub otf_count: u64,
    pub ttc_count: u64,

    pub total_names: u64,
    pub family_name_count: u64,
    pub subfamily_name_count: u64,
    pub full_name_count: u64,
    pub postscript_name_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontFileIndexRecord {
    pub file_size: u64,

    /// Unix timestamp in nanoseconds.
    pub modified_at: i64,

    /// Lowercase extension without dot, for example "ttf", "otf", or "ttc".
    pub extension: String,

    pub names: Vec<FontNameRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontNameRecord {
    pub face_index: u32,

    pub name_id: u16,
    pub platform_id: u16,
    pub encoding_id: u16,
    pub language_id: u16,

    pub name: String,
    pub name_norm: String,

    pub weight_class: Option<u16>,
    pub is_italic: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    pub root: PathBuf,
    pub scanned_files: usize,
    pub indexed_files: usize,
    pub skipped_unchanged_files: usize,
    pub failed_files: usize,
    pub unavailable_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontMatch {
    pub requested_name: String,
    pub matched_name: String,
    pub name_kind: String,
    pub font_path: PathBuf,
    pub relative_path: String,
    pub name_id: u16,
    pub platform_id: u16,
    pub encoding_id: u16,
    pub language_id: u16,

    pub canonical_path: PathBuf,
    pub face_index: u32,
    pub weight_class: Option<u16>,
    pub is_italic: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveReport {
    pub matched: Vec<ResolvedFont>,
    pub missing: Vec<String>,
    pub unique_font_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFont {
    pub requested_name: String,
    pub matches: Vec<FontMatch>,
}

#[derive(Debug, Clone)]
struct DiscoveredFontPath {
    path: PathBuf,
    relative_path: String,
}

impl FontIndex {
    pub fn open(db_path: &Path) -> Result<Self> {
        let db = Database::create(db_path)
            .with_context(|| format!("failed to open redb font index {}", db_path.display()))?;
        initialize_store(&db)?;

        Ok(Self { db })
    }

    pub fn scan_root(&mut self, root: &Path) -> Result<ScanSummary> {
        let root_path = canonicalize_font_root(root)?;
        let mut summary = ScanSummary {
            root: root_path.clone(),
            ..ScanSummary::default()
        };

        let font_paths = discover_font_paths(&root_path, &mut summary)?;
        summary.scanned_files = font_paths.len();

        let mut forward_records = BTreeMap::new();
        for font_path in font_paths {
            if let Some(record) = analyze_index_record(&font_path.path, &mut summary)? {
                summary.indexed_files += 1;
                forward_records.insert(font_path.relative_path, record);
            }
        }

        let meta = build_meta_record(&root_path, unix_timestamp_now(), forward_records.iter());
        self.write_rebuilt_index(&meta, &forward_records)?;
        Ok(summary)
    }

    pub fn rebuild_root(&mut self, root: &Path) -> Result<ScanSummary> {
        let summary = self.scan_root(root)?;
        self.set_active_font_root(&summary.root)?;
        Ok(summary)
    }

    pub fn update_bound_root(&mut self, root: &Path) -> Result<ScanSummary> {
        let root_path = canonicalize_font_root(root)?;
        let Some(meta) = self.read_meta_record()? else {
            return self.rebuild_root(&root_path);
        };

        if meta.schema_version != SCHEMA_VERSION
            || !paths_equal_text(&meta.root_path, &path_to_db_text(&root_path))
        {
            return self.rebuild_root(&root_path);
        }

        let mut summary = ScanSummary {
            root: root_path.clone(),
            ..ScanSummary::default()
        };
        let font_paths = discover_font_paths(&root_path, &mut summary)?;
        summary.scanned_files = font_paths.len();

        let mut existing_records = self.read_forward_records()?;
        let mut next_records = BTreeMap::new();

        for font_path in font_paths {
            let metadata = match fs::metadata(&font_path.path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    summary.failed_files += 1;
                    eprintln!(
                        "Warning: failed to read metadata for {}: {error}",
                        font_path.path.display()
                    );

                    if let Some(existing) = existing_records.remove(&font_path.relative_path) {
                        summary.indexed_files += 1;
                        next_records.insert(font_path.relative_path, existing);
                    }
                    continue;
                }
            };

            let file_size = metadata.len();
            let modified_at = metadata
                .modified()
                .map(system_time_to_unix_timestamp_nanos)
                .unwrap_or(0);
            let extension = font_path
                .path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();

            if let Some(existing) = existing_records.remove(&font_path.relative_path)
                && index_record_matches_file(&existing, file_size, modified_at, &extension)
            {
                summary.skipped_unchanged_files += 1;
                summary.indexed_files += 1;
                next_records.insert(font_path.relative_path, existing);
                continue;
            }

            if let Some(record) = analyze_index_record(&font_path.path, &mut summary)? {
                summary.indexed_files += 1;
                next_records.insert(font_path.relative_path, record);
            }
        }

        summary.unavailable_files = existing_records.len();

        let meta = build_meta_record(&root_path, unix_timestamp_now(), next_records.iter());
        self.write_rebuilt_index(&meta, &next_records)?;
        Ok(summary)
    }

    pub fn clear_index_data(&mut self) -> Result<()> {
        let write_txn = self
            .db
            .begin_write()
            .context("failed to start redb font index clear transaction")?;

        write_txn
            .delete_table(META_TABLE)
            .context("failed to delete redb font index meta table")?;
        write_txn
            .delete_table(FORWARD_INDEX_TABLE)
            .context("failed to delete redb font index forward table")?;
        write_txn
            .delete_table(REVERSE_INDEX_TABLE)
            .context("failed to delete redb font index reverse table")?;

        {
            write_txn
                .open_table(META_TABLE)
                .context("failed to initialize redb font index meta table")?;
            write_txn
                .open_table(FORWARD_INDEX_TABLE)
                .context("failed to initialize redb font index forward table")?;
            write_txn
                .open_table(REVERSE_INDEX_TABLE)
                .context("failed to initialize redb font index reverse table")?;
        }

        write_txn
            .commit()
            .context("failed to commit redb font index clear transaction")
    }

    pub fn active_font_root(&self) -> Result<Option<PathBuf>> {
        Ok(self
            .read_meta_record()?
            .map(|meta| PathBuf::from(meta.root_path)))
    }

    pub fn set_active_font_root(&self, root: &Path) -> Result<()> {
        let root_path = canonicalize_font_root(root)?;
        let root_text = path_to_db_text(&root_path);

        if let Some(meta) = self.read_meta_record()? {
            if paths_equal_text(&meta.root_path, &root_text) {
                return Ok(());
            }

            bail!(
                "cannot set active font root without rebuilding index: {}",
                root_path.display()
            );
        }

        let meta = build_empty_meta_record(&root_path);
        self.write_meta_record(&meta)
    }

    pub fn summary_for_root(&self, root: &Path) -> Result<ScanSummary> {
        let root_path = canonicalize_font_root(root)?;
        let root_text = path_to_db_text(&root_path);
        let Some(meta) = self.read_meta_record()? else {
            return Ok(ScanSummary {
                root: root_path,
                ..ScanSummary::default()
            });
        };

        if !paths_equal_text(&meta.root_path, &root_text) {
            return Ok(ScanSummary {
                root: root_path,
                ..ScanSummary::default()
            });
        }

        if meta.schema_version != SCHEMA_VERSION {
            return Ok(ScanSummary {
                root: root_path,
                ..ScanSummary::default()
            });
        }

        let total_font_files = usize::try_from(meta.total_font_files).unwrap_or(usize::MAX);
        let mut summary = ScanSummary {
            root: root_path.clone(),
            indexed_files: total_font_files,
            ..ScanSummary::default()
        };
        let font_paths = discover_font_paths(&root_path, &mut summary)?;
        summary.scanned_files = font_paths.len();

        if summary.indexed_files > summary.scanned_files {
            summary.unavailable_files = summary.indexed_files - summary.scanned_files;
        }

        Ok(summary)
    }

    pub fn query_name(&self, font_name: &str) -> Result<Vec<FontMatch>> {
        self.query_name_with_style(font_name, None, None)
    }

    pub fn query_name_with_style(
        &self,
        font_name: &str,
        weight_class: Option<u16>,
        is_italic: Option<bool>,
    ) -> Result<Vec<FontMatch>> {
        let name_norm = normalize_font_name(font_name);
        if name_norm.is_empty() {
            return Ok(Vec::new());
        }

        let read_txn = self
            .db
            .begin_read()
            .context("failed to start redb font index read transaction")?;

        let meta =
            read_meta_record_from_txn(&read_txn)?.context("redb font index metadata is missing")?;
        if meta.schema_version != SCHEMA_VERSION {
            bail!("redb font index schema is outdated; update the font index first");
        }

        let reverse_table = read_txn
            .open_table(REVERSE_INDEX_TABLE)
            .context("failed to open redb font index reverse table")?;
        let Some(relative_paths) = reverse_table
            .get(name_norm.as_str())
            .context("failed to read redb font index reverse table")?
            .map(|value| decode_reverse_paths(value.value()))
        else {
            return Ok(Vec::new());
        };
        drop(reverse_table);

        let forward_table = read_txn
            .open_table(FORWARD_INDEX_TABLE)
            .context("failed to open redb font index forward table")?;
        let mut matches = Vec::new();
        let mut seen_matches = HashSet::new();

        for relative_path in relative_paths {
            let record = forward_table
                .get(relative_path.as_str())
                .context("failed to read redb font index forward table")?
                .map(|value| decode::<FontFileIndexRecord>(value.value()))
                .transpose()?
                .with_context(|| {
                    format!("reverse index points to missing font record {relative_path}")
                })?;

            for name in record
                .names
                .iter()
                .filter(|name| name.name_norm == name_norm)
            {
                if !style_matches(name, weight_class, is_italic) {
                    continue;
                }

                let match_key = (
                    relative_path.clone(),
                    name.face_index,
                    name.name_id,
                    name.platform_id,
                    name.encoding_id,
                    name.language_id,
                    name.name_norm.clone(),
                );
                if !seen_matches.insert(match_key) {
                    continue;
                }

                let font_path =
                    PathBuf::from(&meta.root_path).join(relative_path_to_path_buf(&relative_path));

                matches.push(FontMatch {
                    requested_name: font_name.to_owned(),
                    matched_name: name.name.clone(),
                    name_kind: name_kind(name.name_id).to_owned(),
                    font_path: font_path.clone(),
                    relative_path: relative_path.clone(),
                    name_id: name.name_id,
                    platform_id: name.platform_id,
                    encoding_id: name.encoding_id,
                    language_id: name.language_id,
                    canonical_path: font_path,
                    face_index: name.face_index,
                    weight_class: name.weight_class,
                    is_italic: name.is_italic,
                });
            }
        }

        sort_font_matches(&mut matches);
        Ok(matches)
    }

    pub fn resolve_required_fonts<I, S>(&self, required_names: I) -> Result<ResolveReport>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut report = ResolveReport::default();

        for required_name in required_names {
            let requested_name = required_name.as_ref().to_owned();
            let matches = self.query_name(&requested_name)?;

            if matches.is_empty() {
                report.missing.push(requested_name);
            } else {
                report.matched.push(ResolvedFont {
                    requested_name,
                    matches,
                });
            }
        }

        let mut seen_paths = HashSet::new();
        for resolved in &report.matched {
            for font_match in &resolved.matches {
                let key = path_to_db_text(&font_match.canonical_path);
                if seen_paths.insert(key) {
                    report
                        .unique_font_paths
                        .push(font_match.canonical_path.clone());
                }
            }
        }
        report.unique_font_paths.sort();

        Ok(report)
    }

    pub fn export_names_csv<W: Write>(&self, writer: W) -> Result<()> {
        let mut csv_writer = csv::Writer::from_writer(writer);
        csv_writer
            .write_record([
                "relative_path",
                "extension",
                "file_size",
                "modified_at",
                "name",
                "name_norm",
                "face_index",
                "weight_class",
                "is_italic",
                "name_id",
                "platform_id",
                "encoding_id",
                "language_id",
            ])
            .context("failed to write font name CSV header")?;

        let read_txn = self
            .db
            .begin_read()
            .context("failed to start redb font index read transaction")?;
        let meta =
            read_meta_record_from_txn(&read_txn)?.context("redb font index metadata is missing")?;
        if meta.schema_version != SCHEMA_VERSION {
            bail!("redb font index schema is outdated; update the font index first");
        }

        let forward_table = read_txn
            .open_table(FORWARD_INDEX_TABLE)
            .context("failed to open redb font index forward table")?;

        for entry in forward_table
            .iter()
            .context("failed to iterate redb font index forward table")?
        {
            let (key, value) = entry.context("failed to read redb font index forward row")?;
            let relative_path = key.value().to_owned();
            let record = decode::<FontFileIndexRecord>(value.value())
                .with_context(|| format!("failed to decode font index record {relative_path}"))?;

            for name in record.names {
                csv_writer
                    .write_record([
                        relative_path.clone(),
                        record.extension.clone(),
                        record.file_size.to_string(),
                        record.modified_at.to_string(),
                        name.name,
                        name.name_norm,
                        name.face_index.to_string(),
                        name.weight_class
                            .map(|value| value.to_string())
                            .unwrap_or_default(),
                        name.is_italic.to_string(),
                        name.name_id.to_string(),
                        name.platform_id.to_string(),
                        name.encoding_id.to_string(),
                        name.language_id.to_string(),
                    ])
                    .context("failed to write font name CSV row")?;
            }
        }

        csv_writer
            .flush()
            .context("failed to flush font name CSV writer")?;
        Ok(())
    }

    fn read_meta_record(&self) -> Result<Option<MetaRecord>> {
        let read_txn = self
            .db
            .begin_read()
            .context("failed to start redb font index read transaction")?;
        read_meta_record_from_txn(&read_txn)
    }

    fn write_meta_record(&self, meta: &MetaRecord) -> Result<()> {
        let data = encode(meta)?;
        let write_txn = self
            .db
            .begin_write()
            .context("failed to start redb font index meta transaction")?;
        {
            let mut meta_table = write_txn
                .open_table(META_TABLE)
                .context("failed to open redb font index meta table")?;
            meta_table
                .insert(META_KEY, data.as_slice())
                .context("failed to write redb font index metadata")?;
        }
        write_txn
            .commit()
            .context("failed to commit redb font index meta transaction")
    }

    fn write_rebuilt_index(
        &self,
        meta: &MetaRecord,
        forward_records: &BTreeMap<String, FontFileIndexRecord>,
    ) -> Result<()> {
        let write_txn = self
            .db
            .begin_write()
            .context("failed to start redb font index write transaction")?;

        write_txn
            .delete_table(META_TABLE)
            .context("failed to delete redb font index meta table")?;
        write_txn
            .delete_table(FORWARD_INDEX_TABLE)
            .context("failed to delete redb font index forward table")?;
        write_txn
            .delete_table(REVERSE_INDEX_TABLE)
            .context("failed to delete redb font index reverse table")?;

        {
            let mut forward_table = write_txn
                .open_table(FORWARD_INDEX_TABLE)
                .context("failed to open redb font index forward table")?;

            for (relative_path, record) in forward_records {
                let data = encode(record)?;
                forward_table
                    .insert(relative_path.as_str(), data.as_slice())
                    .with_context(|| {
                        format!("failed to write forward index record {relative_path}")
                    })?;
            }
        }

        {
            let mut reverse_table = write_txn
                .open_table(REVERSE_INDEX_TABLE)
                .context("failed to open redb font index reverse table")?;

            for (name_norm, paths) in build_reverse_index_records(forward_records) {
                let encoded_paths = encode_reverse_paths(&paths);
                reverse_table
                    .insert(name_norm.as_str(), encoded_paths.as_str())
                    .with_context(|| format!("failed to write reverse index name {name_norm}"))?;
            }
        }

        {
            let mut meta_table = write_txn
                .open_table(META_TABLE)
                .context("failed to open redb font index meta table")?;
            let data = encode(meta)?;
            meta_table
                .insert(META_KEY, data.as_slice())
                .context("failed to write redb font index metadata")?;
        }

        write_txn
            .commit()
            .context("failed to commit redb font index write transaction")
    }

    fn read_forward_records(&self) -> Result<BTreeMap<String, FontFileIndexRecord>> {
        let read_txn = self
            .db
            .begin_read()
            .context("failed to start redb font index read transaction")?;
        let forward_table = read_txn
            .open_table(FORWARD_INDEX_TABLE)
            .context("failed to open redb font index forward table")?;
        let mut records = BTreeMap::new();

        for entry in forward_table
            .iter()
            .context("failed to iterate redb font index forward table")?
        {
            let (key, value) = entry.context("failed to read redb font index forward row")?;
            let relative_path = key.value().to_owned();
            let record = decode::<FontFileIndexRecord>(value.value())
                .with_context(|| format!("failed to decode font index record {relative_path}"))?;
            records.insert(relative_path, record);
        }

        Ok(records)
    }
}

pub fn normalize_font_name(name: &str) -> String {
    let normalized = name.trim().nfkc().collect::<String>();
    let mut collapsed = String::new();
    let mut last_was_space = false;

    for ch in normalized.chars() {
        if ch.is_whitespace() {
            if !last_was_space && !collapsed.is_empty() {
                collapsed.push(' ');
                last_was_space = true;
            }
        } else {
            collapsed.push(ch);
            last_was_space = false;
        }
    }

    let lowered = collapsed.trim().to_lowercase();
    lowered
        .strip_prefix('@')
        .unwrap_or(&lowered)
        .trim_start()
        .trim()
        .to_owned()
}

pub fn canonicalize_font_root(root: &Path) -> Result<PathBuf> {
    if !root.is_dir() {
        bail!("font root is not a directory: {}", root.display());
    }

    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    Ok(PathBuf::from(normalize_windows_verbatim_path(
        &root.to_string_lossy(),
    )))
}

fn initialize_store(db: &Database) -> Result<()> {
    let write_txn = db
        .begin_write()
        .context("failed to start redb font index initialization transaction")?;
    {
        write_txn
            .open_table(META_TABLE)
            .context("failed to initialize redb font index meta table")?;
        write_txn
            .open_table(FORWARD_INDEX_TABLE)
            .context("failed to initialize redb font index forward table")?;
        write_txn
            .open_table(REVERSE_INDEX_TABLE)
            .context("failed to initialize redb font index reverse table")?;
    }
    write_txn
        .commit()
        .context("failed to commit redb font index initialization transaction")
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

fn discover_font_paths(root: &Path, summary: &mut ScanSummary) -> Result<Vec<DiscoveredFontPath>> {
    let mut font_paths = Vec::new();

    for entry in WalkDir::new(root) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                summary.failed_files += 1;
                eprintln!("Warning: failed to scan {}: {error}", root.display());
                continue;
            }
        };

        if entry.file_type().is_file() && discover::is_supported_font(entry.path()) {
            font_paths.push(DiscoveredFontPath {
                path: entry.path().to_path_buf(),
                relative_path: relative_path_text(root, entry.path())?,
            });
        }
    }

    font_paths.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(font_paths)
}

fn analyze_index_record(
    path: &Path,
    summary: &mut ScanSummary,
) -> Result<Option<FontFileIndexRecord>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            summary.failed_files += 1;
            eprintln!(
                "Warning: failed to read metadata for {}: {error}",
                path.display()
            );
            return Ok(None);
        }
    };

    let file_size = metadata.len();
    let modified_at = metadata
        .modified()
        .map(system_time_to_unix_timestamp_nanos)
        .unwrap_or(0);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let analysis = match analyze_font_file(path) {
        Ok(analysis) => analysis,
        Err(error) => {
            summary.failed_files += 1;
            eprintln!("Warning: failed to analyze {}: {error:#}", path.display());
            return Ok(None);
        }
    };

    Ok(Some(FontFileIndexRecord {
        file_size,
        modified_at,
        extension,
        names: collect_name_records(&analysis),
    }))
}

fn index_record_matches_file(
    record: &FontFileIndexRecord,
    file_size: u64,
    modified_at: i64,
    extension: &str,
) -> bool {
    record.file_size == file_size
        && record.modified_at == modified_at
        && record.extension.eq_ignore_ascii_case(extension)
}

fn collect_name_records(analysis: &FontFileAnalysis) -> Vec<FontNameRecord> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();

    for face in &analysis.faces {
        for name in &face.name_records {
            if !INDEXED_NAME_IDS.contains(&name.name_id)
                || !is_indexed_platform_id(name.platform_id)
            {
                continue;
            }

            let name_norm = normalize_font_name(&name.value);
            if name_norm.is_empty() {
                continue;
            }

            let key = (
                face.face_index,
                name.name_id,
                name.platform_id,
                name.encoding_id,
                name.language_id,
                name_norm.clone(),
            );
            if !seen.insert(key) {
                continue;
            }

            names.push(FontNameRecord {
                face_index: face.face_index,
                name_id: name.name_id,
                platform_id: name.platform_id,
                encoding_id: name.encoding_id,
                language_id: name.language_id,
                name: name.value.clone(),
                name_norm,
                weight_class: face.weight_class,
                is_italic: face.is_italic,
            });
        }
    }

    names.sort_by(|left, right| {
        left.face_index
            .cmp(&right.face_index)
            .then_with(|| left.name_id.cmp(&right.name_id))
            .then_with(|| left.name_norm.cmp(&right.name_norm))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.platform_id.cmp(&right.platform_id))
            .then_with(|| left.encoding_id.cmp(&right.encoding_id))
            .then_with(|| left.language_id.cmp(&right.language_id))
    });
    names
}

fn build_reverse_index_records(
    forward_records: &BTreeMap<String, FontFileIndexRecord>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut reverse_records = BTreeMap::<String, BTreeSet<String>>::new();

    for (relative_path, record) in forward_records {
        for name in &record.names {
            if REVERSE_NAME_IDS.contains(&name.name_id) {
                reverse_records
                    .entry(name.name_norm.clone())
                    .or_default()
                    .insert(relative_path.clone());
            }
        }
    }

    reverse_records
}

fn encode_reverse_paths(paths: &BTreeSet<String>) -> String {
    paths.iter().cloned().collect::<Vec<_>>().join("\n")
}

fn decode_reverse_paths(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn style_matches(
    name: &FontNameRecord,
    weight_class: Option<u16>,
    is_italic: Option<bool>,
) -> bool {
    if is_italic.is_some_and(|requested_italic| name.is_italic != requested_italic) {
        return false;
    }

    if weight_class.is_some_and(|requested_weight| name.weight_class != Some(requested_weight)) {
        return false;
    }

    true
}

fn sort_font_matches(matches: &mut [FontMatch]) {
    matches.sort_by(|left, right| {
        left.relative_path
            .cmp(&right.relative_path)
            .then_with(|| left.face_index.cmp(&right.face_index))
            .then_with(|| left.weight_class.cmp(&right.weight_class))
            .then_with(|| left.is_italic.cmp(&right.is_italic))
            .then_with(|| left.name_id.cmp(&right.name_id))
            .then_with(|| left.matched_name.cmp(&right.matched_name))
            .then_with(|| left.platform_id.cmp(&right.platform_id))
            .then_with(|| left.encoding_id.cmp(&right.encoding_id))
            .then_with(|| left.language_id.cmp(&right.language_id))
    });
}

fn build_meta_record<'a>(
    root_path: &Path,
    scanned_at: i64,
    forward_records: impl IntoIterator<Item = (&'a String, &'a FontFileIndexRecord)>,
) -> MetaRecord {
    let mut meta = build_empty_meta_record(root_path);
    meta.scanned_at = scanned_at;

    for (_, record) in forward_records {
        meta.total_font_files += 1;
        match record.extension.as_str() {
            "ttf" => meta.ttf_count += 1,
            "otf" => meta.otf_count += 1,
            "ttc" => meta.ttc_count += 1,
            _ => {}
        }

        for name in &record.names {
            meta.total_names += 1;
            match name.name_id {
                1 => meta.family_name_count += 1,
                2 => meta.subfamily_name_count += 1,
                4 => meta.full_name_count += 1,
                6 => meta.postscript_name_count += 1,
                _ => {}
            }
        }
    }

    meta
}

fn build_empty_meta_record(root_path: &Path) -> MetaRecord {
    MetaRecord {
        schema_version: SCHEMA_VERSION,
        root_path: path_to_db_text(root_path),
        scanned_at: unix_timestamp_now(),
        total_font_files: 0,
        ttf_count: 0,
        otf_count: 0,
        ttc_count: 0,
        total_names: 0,
        family_name_count: 0,
        subfamily_name_count: 0,
        full_name_count: 0,
        postscript_name_count: 0,
    }
}

fn name_kind(name_id: u16) -> &'static str {
    match name_id {
        1 => "family_name",
        2 => "subfamily_name",
        4 => "full_name",
        6 => "postscript_name",
        16 => "typographic_family_name",
        17 => "typographic_subfamily_name",
        _ => "name",
    }
}

fn is_indexed_platform_id(platform_id: u16) -> bool {
    matches!(platform_id, 0 | 1 | 3)
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    bincode::serde::encode_to_vec(value, bincode::config::standard())
        .context("failed to serialize redb font index record")
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let (value, _) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
        .context("failed to deserialize redb font index record")?;
    Ok(value)
}

fn relative_path_text(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "failed to make font path {} relative to {}",
            path.display(),
            root.display()
        )
    })?;
    let mut parts = Vec::new();

    for component in relative.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => bail!("font path is not relative: {}", relative.display()),
        }
    }

    if parts.is_empty() {
        bail!(
            "font path has no relative file name: {}",
            relative.display()
        );
    }

    Ok(parts.join("/"))
}

fn relative_path_to_path_buf(relative_path: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for part in relative_path.split('/') {
        if !part.is_empty() {
            path.push(part);
        }
    }
    path
}

fn path_to_db_text(path: &Path) -> String {
    normalize_windows_verbatim_path(&path.to_string_lossy())
}

fn normalize_windows_verbatim_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = path.strip_prefix("\\\\?\\") {
        rest.to_owned()
    } else {
        path.to_owned()
    }
}

fn paths_equal_text(left: &str, right: &str) -> bool {
    left == right || left.eq_ignore_ascii_case(right)
}

fn unix_timestamp_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn system_time_to_unix_timestamp_nanos(value: SystemTime) -> i64 {
    value
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        FontFileIndexRecord, FontNameRecord, build_reverse_index_records, decode_reverse_paths,
        encode_reverse_paths,
    };
    use std::collections::BTreeMap;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn reverse_index_maps_name_to_sorted_deduplicated_paths() {
        let mut records = BTreeMap::new();
        records.insert(
            "Family-Regular.ttf".to_owned(),
            record(vec![name("Family", 1, 400, false, 0)]),
        );
        records.insert(
            "Family-Bold.ttf".to_owned(),
            record(vec![
                name("Family", 1, 700, false, 0),
                name("Family Regular", 4, 400, false, 0),
            ]),
        );
        records.insert(
            "Family-Italic.ttf".to_owned(),
            record(vec![name("Family", 1, 400, true, 0)]),
        );

        let reverse = build_reverse_index_records(&records);
        let paths = reverse.get("family").expect("family name exists");

        assert_eq!(
            paths.iter().cloned().collect::<Vec<_>>(),
            vec![
                "Family-Bold.ttf".to_owned(),
                "Family-Italic.ttf".to_owned(),
                "Family-Regular.ttf".to_owned(),
            ]
        );
    }

    #[test]
    fn reverse_index_ignores_subfamily_names() {
        let mut records = BTreeMap::new();
        records.insert(
            "Family-Regular.ttf".to_owned(),
            record(vec![name("Regular", 2, 400, false, 0)]),
        );
        let reverse = build_reverse_index_records(&records);

        assert!(!reverse.contains_key("regular"));
    }

    #[test]
    fn reverse_index_includes_typographic_family_names() {
        let mut records = BTreeMap::new();
        records.insert(
            "Family-Regular.ttf".to_owned(),
            record(vec![name("Preferred Family", 16, 400, false, 0)]),
        );
        let reverse = build_reverse_index_records(&records);

        assert!(reverse.contains_key("preferred family"));
    }

    #[test]
    fn reverse_index_ignores_typographic_subfamily_names() {
        let mut records = BTreeMap::new();
        records.insert(
            "Family-Regular.ttf".to_owned(),
            record(vec![name("Preferred Regular", 17, 400, false, 0)]),
        );
        let reverse = build_reverse_index_records(&records);

        assert!(!reverse.contains_key("preferred regular"));
    }

    #[test]
    fn typographic_name_ids_have_specific_labels() {
        assert_eq!(super::name_kind(16), "typographic_family_name");
        assert_eq!(super::name_kind(17), "typographic_subfamily_name");
    }

    #[test]
    fn reverse_paths_round_trip() {
        let mut records = BTreeMap::new();
        records.insert(
            "Family-Regular.ttf".to_owned(),
            record(vec![
                name("Family", 1, 400, false, 7),
                name("Family", 1, 400, false, 8),
            ]),
        );

        let reverse = build_reverse_index_records(&records);
        let paths = reverse.get("family").expect("family name exists");
        let encoded = encode_reverse_paths(paths);
        let decoded = decode_reverse_paths(&encoded);

        assert_eq!(decoded, vec!["Family-Regular.ttf".to_owned()]);
    }

    #[test]
    fn modified_timestamp_keeps_subsecond_precision() {
        let first = UNIX_EPOCH + Duration::new(1, 100);
        let second = UNIX_EPOCH + Duration::new(1, 200);

        assert_ne!(
            super::system_time_to_unix_timestamp_nanos(first),
            super::system_time_to_unix_timestamp_nanos(second)
        );
    }

    fn record(names: Vec<FontNameRecord>) -> FontFileIndexRecord {
        FontFileIndexRecord {
            file_size: 1,
            modified_at: 1,
            extension: "ttf".to_owned(),
            names,
        }
    }

    fn name(
        value: &str,
        name_id: u16,
        weight_class: u16,
        is_italic: bool,
        face_index: u32,
    ) -> FontNameRecord {
        FontNameRecord {
            face_index,
            name_id,
            platform_id: 3,
            encoding_id: 1,
            language_id: 0x0409,
            name: value.to_owned(),
            name_norm: value.to_lowercase(),
            weight_class: Some(weight_class),
            is_italic,
        }
    }
}
