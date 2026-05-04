use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use ttf_parser::name_id;
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

use crate::discover;

use super::{FontFileAnalysis, analyze_font_file};

const SCHEMA_VERSION: u32 = 1;
const META_KEY: &str = "state";

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

    pub total_aliases: u64,
    pub family_alias_count: u64,
    pub full_name_alias_count: u64,
    pub postscript_alias_count: u64,
    pub other_alias_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontFileIndexRecord {
    pub file_size: u64,

    /// Unix timestamp.
    pub modified_at: i64,

    /// Lowercase extension without dot, for example "ttf", "otf", or "ttc".
    pub extension: String,

    pub aliases: Vec<FontAliasRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontAliasRecord {
    pub alias_raw: String,
    pub alias_norm: String,
    pub alias_kind: String,

    pub name_id: u16,

    pub platform_id: Option<u16>,
    pub encoding_id: Option<u16>,
    pub language_id: Option<u16>,

    pub priority: i16,
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
    pub matched_alias: String,
    pub alias_kind: String,
    pub font_path: PathBuf,
    pub relative_path: String,
    pub name_id: u16,
    pub platform_id: Option<u16>,
    pub encoding_id: Option<u16>,
    pub language_id: Option<u16>,

    // Compatibility fields retained while the rest of the project moves away
    // from face-level index data.
    pub canonical_path: PathBuf,
    pub face_index: u32,
    pub family_name: Option<String>,
    pub subfamily_name: Option<String>,
    pub full_name: Option<String>,
    pub postscript_name: Option<String>,
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

        self.clear_index_data()?;

        let font_paths = discover_font_paths(&root_path, &mut summary)?;
        summary.scanned_files = font_paths.len();

        let mut forward_records = Vec::new();
        for font_path in font_paths {
            let metadata = match fs::metadata(&font_path.path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    summary.failed_files += 1;
                    eprintln!(
                        "Warning: failed to read metadata for {}: {error}",
                        font_path.path.display()
                    );
                    continue;
                }
            };

            let mut record = FontFileIndexRecord {
                file_size: metadata.len(),
                modified_at: metadata
                    .modified()
                    .map(system_time_to_unix_timestamp)
                    .unwrap_or(0),
                extension: font_path
                    .path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
                aliases: Vec::new(),
            };

            match analyze_font_file(&font_path.path) {
                Ok(analysis) => {
                    record.aliases = collect_alias_records(&analysis);
                    summary.indexed_files += 1;
                }
                Err(error) => {
                    summary.failed_files += 1;
                    eprintln!(
                        "Warning: failed to analyze {}: {error:#}",
                        font_path.path.display()
                    );
                }
            }

            forward_records.push((font_path.relative_path, record));
        }

        let meta = build_meta_record(&root_path, unix_timestamp_now(), &forward_records);
        self.write_rebuilt_index(&meta, &forward_records)?;
        Ok(summary)
    }

    pub fn rebuild_root(&mut self, root: &Path) -> Result<ScanSummary> {
        let summary = self.scan_root(root)?;
        self.set_active_font_root(&summary.root)?;
        Ok(summary)
    }

    pub fn update_bound_root(&mut self, root: &Path) -> Result<ScanSummary> {
        let summary = self.scan_root(root)?;
        self.set_active_font_root(&summary.root)?;
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

        let total_font_files = u64_to_usize(meta.total_font_files);
        Ok(ScanSummary {
            root: root_path,
            scanned_files: total_font_files,
            indexed_files: total_font_files,
            ..ScanSummary::default()
        })
    }

    pub fn query_alias(&self, font_name: &str) -> Result<Vec<FontMatch>> {
        let alias_norm = normalize_font_name(font_name);
        if alias_norm.is_empty() {
            return Ok(Vec::new());
        }

        let read_txn = self
            .db
            .begin_read()
            .context("failed to start redb font index read transaction")?;

        let reverse_table = read_txn
            .open_table(REVERSE_INDEX_TABLE)
            .context("failed to open redb font index reverse table")?;
        let Some(relative_path) = reverse_table
            .get(alias_norm.as_str())
            .context("failed to read redb font index reverse table")?
            .map(|value| value.value().to_owned())
        else {
            return Ok(Vec::new());
        };
        drop(reverse_table);

        let meta =
            read_meta_record_from_txn(&read_txn)?.context("redb font index metadata is missing")?;

        let forward_table = read_txn
            .open_table(FORWARD_INDEX_TABLE)
            .context("failed to open redb font index forward table")?;
        let record = forward_table
            .get(relative_path.as_str())
            .context("failed to read redb font index forward table")?
            .map(|value| decode::<FontFileIndexRecord>(value.value()))
            .transpose()?
            .with_context(|| {
                format!("reverse index points to missing font record {relative_path}")
            })?;

        let Some(alias) = record
            .aliases
            .iter()
            .find(|alias| alias.alias_norm == alias_norm)
        else {
            return Ok(Vec::new());
        };

        let font_path =
            PathBuf::from(&meta.root_path).join(relative_path_to_path_buf(&relative_path));

        Ok(vec![FontMatch {
            requested_name: font_name.to_owned(),
            matched_alias: alias.alias_raw.clone(),
            alias_kind: alias.alias_kind.clone(),
            font_path: font_path.clone(),
            relative_path,
            name_id: alias.name_id,
            platform_id: alias.platform_id,
            encoding_id: alias.encoding_id,
            language_id: alias.language_id,
            canonical_path: font_path,
            face_index: 0,
            family_name: None,
            subfamily_name: None,
            full_name: None,
            postscript_name: None,
            weight_class: None,
            is_italic: false,
        }])
    }

    pub fn resolve_required_fonts<I, S>(&self, required_names: I) -> Result<ResolveReport>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut report = ResolveReport::default();

        for required_name in required_names {
            let requested_name = required_name.as_ref().to_owned();
            let matches = self.query_alias(&requested_name)?;

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

    pub fn export_aliases_csv<W: Write>(&self, writer: W) -> Result<()> {
        let mut csv_writer = csv::Writer::from_writer(writer);
        csv_writer
            .write_record([
                "relative_path",
                "extension",
                "file_size",
                "modified_at",
                "alias_raw",
                "alias_norm",
                "alias_kind",
                "name_id",
                "platform_id",
                "encoding_id",
                "language_id",
                "priority",
            ])
            .context("failed to write alias CSV header")?;

        let read_txn = self
            .db
            .begin_read()
            .context("failed to start redb font index read transaction")?;
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

            for alias in record.aliases {
                csv_writer
                    .write_record([
                        relative_path.clone(),
                        record.extension.clone(),
                        record.file_size.to_string(),
                        record.modified_at.to_string(),
                        alias.alias_raw,
                        alias.alias_norm,
                        alias.alias_kind,
                        alias.name_id.to_string(),
                        optional_u16_text(alias.platform_id),
                        optional_u16_text(alias.encoding_id),
                        optional_u16_text(alias.language_id),
                        alias.priority.to_string(),
                    ])
                    .context("failed to write alias CSV row")?;
            }
        }

        csv_writer
            .flush()
            .context("failed to flush alias CSV writer")?;
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
        forward_records: &[(String, FontFileIndexRecord)],
    ) -> Result<()> {
        let write_txn = self
            .db
            .begin_write()
            .context("failed to start redb font index write transaction")?;

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
            let mut seen_alias_norms = HashSet::new();

            for (relative_path, record) in forward_records {
                for alias in &record.aliases {
                    if seen_alias_norms.insert(alias.alias_norm.clone()) {
                        reverse_table
                            .insert(alias.alias_norm.as_str(), relative_path.as_str())
                            .with_context(|| {
                                format!("failed to write reverse index alias {}", alias.alias_norm)
                            })?;
                    }
                }
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

    Ok(canonicalize_path(root))
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

fn collect_alias_records(analysis: &FontFileAnalysis) -> Vec<FontAliasRecord> {
    let mut seen = HashSet::new();
    let mut aliases = Vec::new();

    for face in &analysis.faces {
        for alias in &face.aliases {
            let alias_kind = alias_kind(alias.name_id);
            let platform_id = platform_id_value(&alias.platform_id);
            let encoding_id = Some(alias.encoding_id);
            let language_id = Some(alias.language_id);

            for alias_raw in alias_raw_variants(&alias.value) {
                let alias_norm = normalize_font_name(&alias_raw);
                if alias_norm.is_empty() {
                    continue;
                }

                let key = (
                    alias_raw.clone(),
                    alias_norm.clone(),
                    alias_kind.to_owned(),
                    alias.name_id,
                    platform_id,
                    encoding_id,
                    language_id,
                );
                if !seen.insert(key) {
                    continue;
                }

                aliases.push(FontAliasRecord {
                    alias_raw,
                    alias_norm,
                    alias_kind: alias_kind.to_owned(),
                    name_id: alias.name_id,
                    platform_id,
                    encoding_id,
                    language_id,
                    priority: alias_priority(alias.name_id),
                });
            }
        }
    }

    sort_alias_records(&mut aliases);
    aliases
}

fn sort_alias_records(aliases: &mut [FontAliasRecord]) {
    aliases.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.alias_kind.cmp(&right.alias_kind))
            .then_with(|| left.alias_raw.cmp(&right.alias_raw))
            .then_with(|| left.name_id.cmp(&right.name_id))
    });
}

fn build_meta_record(
    root_path: &Path,
    scanned_at: i64,
    forward_records: &[(String, FontFileIndexRecord)],
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

        for alias in &record.aliases {
            meta.total_aliases += 1;
            match alias.alias_kind.as_str() {
                "family" => meta.family_alias_count += 1,
                "full_name" => meta.full_name_alias_count += 1,
                "postscript_name" => meta.postscript_alias_count += 1,
                _ => meta.other_alias_count += 1,
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
        total_aliases: 0,
        family_alias_count: 0,
        full_name_alias_count: 0,
        postscript_alias_count: 0,
        other_alias_count: 0,
    }
}

fn alias_kind(name_id: u16) -> &'static str {
    match name_id {
        name_id::FAMILY => "family",
        name_id::FULL_NAME => "full_name",
        name_id::POST_SCRIPT_NAME => "postscript_name",
        _ => "name",
    }
}

fn alias_priority(name_id: u16) -> i16 {
    match name_id {
        name_id::FAMILY => 300,
        name_id::FULL_NAME => 200,
        name_id::POST_SCRIPT_NAME => 100,
        _ => 0,
    }
}

fn platform_id_value(platform_id: &str) -> Option<u16> {
    match platform_id {
        "Unicode" => Some(0),
        "Macintosh" => Some(1),
        "ISO" => Some(2),
        "Windows" => Some(3),
        "Custom" => Some(4),
        _ => None,
    }
}

fn alias_raw_variants(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }

    let mut variants = vec![value.to_owned()];
    if let Some(stripped) = value.strip_prefix('@') {
        let stripped = stripped.trim_start();
        if !stripped.is_empty() {
            variants.push(stripped.to_owned());
        }
    }

    variants
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
    slash_separated_path(relative)
}

fn slash_separated_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();

    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => bail!("font path is not relative: {}", path.display()),
        }
    }

    if parts.is_empty() {
        bail!("font path has no relative file name: {}", path.display());
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

fn canonicalize_path(path: &Path) -> PathBuf {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    PathBuf::from(normalize_windows_verbatim_path(&path.to_string_lossy()))
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

fn optional_u16_text(value: Option<u16>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn unix_timestamp_now() -> i64 {
    system_time_to_unix_timestamp(SystemTime::now())
}

fn system_time_to_unix_timestamp(value: SystemTime) -> i64 {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => u64_to_i64(duration.as_secs()),
        Err(_) => 0,
    }
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn u64_to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}
