use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use redb::{Database, ReadableDatabase, TableDefinition};
use serde::{Deserialize, Serialize};
use ttf_parser::name_id;
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

use crate::discover;

use super::{FontAlias, FontFaceAnalysis, analyze_font_file};

const INDEX_STATE_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("index_state");
const INDEX_STATE_KEY: &str = "state";

pub struct FontIndex {
    db: Database,
    state: RefCell<IndexState>,
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

#[derive(Debug, Clone, Copy)]
struct ExistingFontFile {
    id: u64,
    file_size: i64,
    modified_at: i64,
}

#[derive(Debug, Clone)]
struct SortedFontMatch {
    root_priority: i64,
    alias_priority: i64,
    file_path: String,
    face_index: u32,
    alias_raw: String,
    font_match: FontMatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct IndexState {
    next_root_id: u64,
    next_file_id: u64,
    next_face_id: u64,
    next_alias_id: u64,
    active_font_root: Option<String>,
    scan_roots: BTreeMap<u64, ScanRootRecord>,
    root_by_path: BTreeMap<String, u64>,
    font_files: BTreeMap<u64, FontFileRecord>,
    file_by_canonical_path: BTreeMap<String, u64>,
    font_faces: BTreeMap<u64, FontFaceRecord>,
    face_ids_by_file: BTreeMap<u64, Vec<u64>>,
    font_aliases: BTreeMap<u64, FontAliasRecord>,
    alias_ids_by_face: BTreeMap<u64, Vec<u64>>,
    alias_ids_by_norm: BTreeMap<String, Vec<u64>>,
}

impl Default for IndexState {
    fn default() -> Self {
        Self {
            next_root_id: 1,
            next_file_id: 1,
            next_face_id: 1,
            next_alias_id: 1,
            active_font_root: None,
            scan_roots: BTreeMap::new(),
            root_by_path: BTreeMap::new(),
            font_files: BTreeMap::new(),
            file_by_canonical_path: BTreeMap::new(),
            font_faces: BTreeMap::new(),
            face_ids_by_file: BTreeMap::new(),
            font_aliases: BTreeMap::new(),
            alias_ids_by_face: BTreeMap::new(),
            alias_ids_by_norm: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScanRootRecord {
    root_path: String,
    root_kind: String,
    priority: i64,
    created_at: i64,
    updated_at: i64,
    last_scanned_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FontFileRecord {
    root_id: u64,
    path: String,
    canonical_path: String,
    extension: String,
    file_size: i64,
    modified_at: i64,
    is_available: bool,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FontFaceRecord {
    file_id: u64,
    face_index: u32,
    family_name: Option<String>,
    subfamily_name: Option<String>,
    full_name: Option<String>,
    postscript_name: Option<String>,
    weight_class: Option<i64>,
    width_class: Option<i64>,
    is_italic: bool,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FontAliasRecord {
    face_id: u64,
    alias_raw: String,
    alias_norm: String,
    alias_kind: String,
    language: Option<String>,
    platform_id: Option<i64>,
    name_id: i64,
    priority: i64,
    created_at: i64,
}

impl FontIndex {
    pub fn open(db_path: &Path) -> Result<Self> {
        let db = Database::create(db_path)
            .with_context(|| format!("failed to open redb font index {}", db_path.display()))?;
        initialize_store(&db)?;
        let state = load_state(&db)?;

        Ok(Self {
            db,
            state: RefCell::new(state),
        })
    }

    pub fn scan_root(&mut self, root: &Path) -> Result<ScanSummary> {
        if !root.is_dir() {
            bail!("font root is not a directory: {}", root.display());
        }

        let root_path = canonicalize_path(root);
        let mut summary = ScanSummary {
            root: root_path.clone(),
            ..ScanSummary::default()
        };
        let font_paths = discover_font_paths(root, &mut summary)?;
        summary.scanned_files = font_paths.len();

        let now = unix_timestamp_now();
        let mut state = self.state.borrow().clone();
        let root_id = upsert_scan_root(&mut state, &root_path, now);
        let mut seen_paths = HashSet::new();

        for path in font_paths {
            let canonical_path = canonicalize_path(&path);
            let canonical_path_text = path_to_db_text(&canonical_path);
            seen_paths.insert(canonical_path_text.clone());

            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    summary.failed_files += 1;
                    eprintln!(
                        "Warning: failed to read metadata for {}: {error}",
                        path.display()
                    );
                    continue;
                }
            };

            let file_size = u64_to_i64(metadata.len());
            let modified_at = metadata
                .modified()
                .map(system_time_to_unix_timestamp)
                .unwrap_or(0);
            let path_text = path_to_db_text(&path);
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();

            if let Some(existing) = find_existing_font_file(&state, &canonical_path_text) {
                if existing.file_size == file_size && existing.modified_at == modified_at {
                    mark_font_file_seen(
                        &mut state,
                        existing.id,
                        root_id,
                        &path_text,
                        &extension,
                        now,
                    )?;
                    summary.skipped_unchanged_files += 1;
                    continue;
                }
            }

            let file_id = upsert_font_file(
                &mut state,
                root_id,
                &path_text,
                &canonical_path_text,
                &extension,
                file_size,
                modified_at,
                now,
            );
            remove_indexed_faces(&mut state, file_id);

            match analyze_font_file(&path) {
                Ok(analysis) => {
                    for face in &analysis.faces {
                        insert_font_face(&mut state, file_id, face, now);
                    }
                    summary.indexed_files += 1;
                }
                Err(error) => {
                    summary.failed_files += 1;
                    eprintln!("Warning: failed to analyze {}: {error:#}", path.display());
                }
            }
        }

        summary.unavailable_files = mark_unavailable_files(&mut state, root_id, &seen_paths, now);
        if let Some(root) = state.scan_roots.get_mut(&root_id) {
            root.last_scanned_at = Some(now);
            root.updated_at = now;
        }

        self.replace_state(state)?;
        Ok(summary)
    }

    pub fn rebuild_root(&mut self, root: &Path) -> Result<ScanSummary> {
        let root_path = canonicalize_font_root(root)?;
        self.clear_index_data()?;
        let summary = self.scan_root(&root_path)?;
        self.set_active_font_root(&summary.root)?;
        Ok(summary)
    }

    pub fn update_bound_root(&mut self, root: &Path) -> Result<ScanSummary> {
        let root_path = canonicalize_font_root(root)?;
        let summary = self.scan_root(&root_path)?;
        self.set_active_font_root(&summary.root)?;
        Ok(summary)
    }

    pub fn clear_index_data(&mut self) -> Result<()> {
        self.replace_state(IndexState::default())
    }

    pub fn active_font_root(&self) -> Result<Option<PathBuf>> {
        Ok(self
            .state
            .borrow()
            .active_font_root
            .as_ref()
            .map(PathBuf::from))
    }

    pub fn set_active_font_root(&self, root: &Path) -> Result<()> {
        let root_path = canonicalize_font_root(root)?;
        let mut state = self.state.borrow().clone();
        state.active_font_root = Some(path_to_db_text(&root_path));
        self.replace_state(state)
    }

    pub fn summary_for_root(&self, root: &Path) -> Result<ScanSummary> {
        let root_path = canonicalize_font_root(root)?;
        let root_text = path_to_db_text(&root_path);
        let state = self.state.borrow();
        let Some(root_id) = state.root_by_path.get(&root_text).copied() else {
            return Ok(ScanSummary {
                root: root_path,
                ..ScanSummary::default()
            });
        };

        let scanned_files = state
            .font_files
            .values()
            .filter(|file| file.root_id == root_id && file.is_available)
            .count();

        let indexed_files = state
            .font_files
            .iter()
            .filter(|(file_id, file)| {
                file.root_id == root_id
                    && file.is_available
                    && state
                        .face_ids_by_file
                        .get(file_id)
                        .is_some_and(|face_ids| !face_ids.is_empty())
            })
            .count();

        Ok(ScanSummary {
            root: root_path,
            scanned_files,
            indexed_files,
            ..ScanSummary::default()
        })
    }

    pub fn query_alias(&self, font_name: &str) -> Result<Vec<FontMatch>> {
        let alias_norm = normalize_font_name(font_name);
        if alias_norm.is_empty() {
            return Ok(Vec::new());
        }

        let requested_name = font_name.to_owned();
        let state = self.state.borrow();
        let Some(alias_ids) = state.alias_ids_by_norm.get(&alias_norm) else {
            return Ok(Vec::new());
        };

        let mut rows = Vec::new();
        for alias_id in alias_ids {
            let Some(alias) = state.font_aliases.get(alias_id) else {
                continue;
            };
            let Some(face) = state.font_faces.get(&alias.face_id) else {
                continue;
            };
            let Some(file) = state.font_files.get(&face.file_id) else {
                continue;
            };
            if !file.is_available {
                continue;
            }
            let Some(root) = state.scan_roots.get(&file.root_id) else {
                continue;
            };

            rows.push(SortedFontMatch {
                root_priority: root.priority,
                alias_priority: alias.priority,
                file_path: file.path.clone(),
                face_index: face.face_index,
                alias_raw: alias.alias_raw.clone(),
                font_match: FontMatch {
                    requested_name: requested_name.clone(),
                    matched_alias: alias.alias_raw.clone(),
                    alias_kind: alias.alias_kind.clone(),
                    font_path: PathBuf::from(&file.path),
                    canonical_path: PathBuf::from(&file.canonical_path),
                    face_index: face.face_index,
                    family_name: face.family_name.clone(),
                    subfamily_name: face.subfamily_name.clone(),
                    full_name: face.full_name.clone(),
                    postscript_name: face.postscript_name.clone(),
                    weight_class: face.weight_class.map(i64_to_u16),
                    is_italic: face.is_italic,
                },
            });
        }

        rows.sort_by(|left, right| {
            right
                .root_priority
                .cmp(&left.root_priority)
                .then_with(|| right.alias_priority.cmp(&left.alias_priority))
                .then_with(|| left.file_path.cmp(&right.file_path))
                .then_with(|| left.face_index.cmp(&right.face_index))
                .then_with(|| left.alias_raw.cmp(&right.alias_raw))
        });

        Ok(rows.into_iter().map(|row| row.font_match).collect())
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
                "alias_raw",
                "alias_norm",
                "alias_kind",
                "family_name",
                "full_name",
                "face_index",
                "path",
                "is_available",
            ])
            .context("failed to write alias CSV header")?;

        let state = self.state.borrow();
        let mut rows = Vec::new();
        for alias in state.font_aliases.values() {
            let Some(face) = state.font_faces.get(&alias.face_id) else {
                continue;
            };
            let Some(file) = state.font_files.get(&face.file_id) else {
                continue;
            };

            rows.push((
                alias.alias_norm.clone(),
                file.path.clone(),
                face.face_index,
                alias.alias_kind.clone(),
                [
                    alias.alias_raw.clone(),
                    alias.alias_norm.clone(),
                    alias.alias_kind.clone(),
                    face.family_name.clone().unwrap_or_default(),
                    face.full_name.clone().unwrap_or_default(),
                    face.face_index.to_string(),
                    file.path.clone(),
                    file.is_available.to_string(),
                ],
            ));
        }

        rows.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.3.cmp(&right.3))
        });

        for (_, _, _, _, row) in rows {
            csv_writer
                .write_record(row)
                .context("failed to write alias CSV row")?;
        }

        csv_writer
            .flush()
            .context("failed to flush alias CSV writer")?;
        Ok(())
    }

    fn replace_state(&self, state: IndexState) -> Result<()> {
        *self.state.borrow_mut() = state;
        self.save_state()
    }

    fn save_state(&self) -> Result<()> {
        let data = serde_json::to_vec(&*self.state.borrow())
            .context("failed to serialize redb font index state")?;
        let write_txn = self
            .db
            .begin_write()
            .context("failed to start redb font index write transaction")?;
        {
            let mut table = write_txn
                .open_table(INDEX_STATE_TABLE)
                .context("failed to open redb font index state table")?;
            table
                .insert(INDEX_STATE_KEY, data.as_slice())
                .context("failed to write redb font index state")?;
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

fn initialize_store(db: &Database) -> Result<()> {
    let write_txn = db
        .begin_write()
        .context("failed to start redb font index initialization transaction")?;
    {
        write_txn
            .open_table(INDEX_STATE_TABLE)
            .context("failed to initialize redb font index state table")?;
    }
    write_txn
        .commit()
        .context("failed to commit redb font index initialization transaction")
}

fn load_state(db: &Database) -> Result<IndexState> {
    let read_txn = db
        .begin_read()
        .context("failed to start redb font index read transaction")?;
    let table = read_txn
        .open_table(INDEX_STATE_TABLE)
        .context("failed to open redb font index state table")?;

    let state = match table
        .get(INDEX_STATE_KEY)
        .context("failed to read redb font index state")?
    {
        Some(value) => serde_json::from_slice(value.value())
            .context("failed to deserialize redb font index state")?,
        None => IndexState::default(),
    };

    Ok(state)
}

pub fn canonicalize_font_root(root: &Path) -> Result<PathBuf> {
    if !root.is_dir() {
        bail!("font root is not a directory: {}", root.display());
    }

    Ok(canonicalize_path(root))
}

fn discover_font_paths(root: &Path, summary: &mut ScanSummary) -> Result<Vec<PathBuf>> {
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
            font_paths.push(entry.path().to_path_buf());
        }
    }

    font_paths.sort();
    Ok(font_paths)
}

fn upsert_scan_root(state: &mut IndexState, root_path: &Path, now: i64) -> u64 {
    let root_path = path_to_db_text(root_path);
    if let Some(root_id) = state.root_by_path.get(&root_path).copied() {
        if let Some(root) = state.scan_roots.get_mut(&root_id) {
            root.updated_at = now;
        }
        return root_id;
    }

    let root_id = next_id(&mut state.next_root_id);
    state.scan_roots.insert(
        root_id,
        ScanRootRecord {
            root_path: root_path.clone(),
            root_kind: "directory".to_owned(),
            priority: 0,
            created_at: now,
            updated_at: now,
            last_scanned_at: None,
        },
    );
    state.root_by_path.insert(root_path, root_id);
    root_id
}

fn find_existing_font_file(state: &IndexState, canonical_path: &str) -> Option<ExistingFontFile> {
    let file_id = state.file_by_canonical_path.get(canonical_path).copied()?;
    let file = state.font_files.get(&file_id)?;
    Some(ExistingFontFile {
        id: file_id,
        file_size: file.file_size,
        modified_at: file.modified_at,
    })
}

fn mark_font_file_seen(
    state: &mut IndexState,
    file_id: u64,
    root_id: u64,
    path: &str,
    extension: &str,
    now: i64,
) -> Result<()> {
    let file = state
        .font_files
        .get_mut(&file_id)
        .context("indexed font file disappeared from redb state")?;
    file.root_id = root_id;
    file.path = path.to_owned();
    file.extension = extension.to_owned();
    file.is_available = true;
    file.updated_at = now;
    Ok(())
}

fn upsert_font_file(
    state: &mut IndexState,
    root_id: u64,
    path: &str,
    canonical_path: &str,
    extension: &str,
    file_size: i64,
    modified_at: i64,
    now: i64,
) -> u64 {
    if let Some(file_id) = state.file_by_canonical_path.get(canonical_path).copied() {
        if let Some(file) = state.font_files.get_mut(&file_id) {
            file.root_id = root_id;
            file.path = path.to_owned();
            file.canonical_path = canonical_path.to_owned();
            file.extension = extension.to_owned();
            file.file_size = file_size;
            file.modified_at = modified_at;
            file.is_available = true;
            file.updated_at = now;
        }
        return file_id;
    }

    let file_id = next_id(&mut state.next_file_id);
    state.font_files.insert(
        file_id,
        FontFileRecord {
            root_id,
            path: path.to_owned(),
            canonical_path: canonical_path.to_owned(),
            extension: extension.to_owned(),
            file_size,
            modified_at,
            is_available: true,
            created_at: now,
            updated_at: now,
        },
    );
    state
        .file_by_canonical_path
        .insert(canonical_path.to_owned(), file_id);
    file_id
}

fn remove_indexed_faces(state: &mut IndexState, file_id: u64) {
    let Some(face_ids) = state.face_ids_by_file.remove(&file_id) else {
        return;
    };

    for face_id in face_ids {
        if let Some(alias_ids) = state.alias_ids_by_face.remove(&face_id) {
            for alias_id in alias_ids {
                remove_alias(state, alias_id);
            }
        }
        state.font_faces.remove(&face_id);
    }
}

fn insert_font_face(state: &mut IndexState, file_id: u64, face: &FontFaceAnalysis, now: i64) {
    let family_name = first_alias_for_name_id(face, name_id::FAMILY);
    let subfamily_name: Option<String> = None;
    let full_name = first_alias_for_name_id(face, name_id::FULL_NAME);
    let postscript_name = first_alias_for_name_id(face, name_id::POST_SCRIPT_NAME);
    let weight_class: Option<i64> = None;
    let width_class: Option<i64> = None;
    let is_italic = false;
    let face_id = next_id(&mut state.next_face_id);

    state.font_faces.insert(
        face_id,
        FontFaceRecord {
            file_id,
            face_index: face.face_index,
            family_name,
            subfamily_name,
            full_name,
            postscript_name,
            weight_class,
            width_class,
            is_italic,
            created_at: now,
            updated_at: now,
        },
    );
    state
        .face_ids_by_file
        .entry(file_id)
        .or_default()
        .push(face_id);

    insert_font_aliases(state, face_id, &face.aliases, now);
}

fn insert_font_aliases(state: &mut IndexState, face_id: u64, aliases: &[FontAlias], now: i64) {
    let mut seen = HashSet::new();

    for alias in aliases {
        let alias_kind = alias_kind(alias.name_id);
        let priority = alias_priority(alias.name_id);
        let language = Some(alias.language_id.to_string());
        let platform_id = platform_id_value(&alias.platform_id);
        let name_id = i64::from(alias.name_id);

        for alias_raw in alias_raw_variants(&alias.value) {
            let alias_norm = normalize_font_name(&alias_raw);
            if alias_norm.is_empty() {
                continue;
            }

            let key = (
                alias_norm.clone(),
                alias_kind.to_owned(),
                language.clone(),
                platform_id,
                name_id,
            );
            if !seen.insert(key) {
                continue;
            }

            let alias_id = next_id(&mut state.next_alias_id);
            state.font_aliases.insert(
                alias_id,
                FontAliasRecord {
                    face_id,
                    alias_raw,
                    alias_norm: alias_norm.clone(),
                    alias_kind: alias_kind.to_owned(),
                    language: language.clone(),
                    platform_id,
                    name_id,
                    priority,
                    created_at: now,
                },
            );
            state
                .alias_ids_by_face
                .entry(face_id)
                .or_default()
                .push(alias_id);
            state
                .alias_ids_by_norm
                .entry(alias_norm)
                .or_default()
                .push(alias_id);
        }
    }
}

fn mark_unavailable_files(
    state: &mut IndexState,
    root_id: u64,
    seen_paths: &HashSet<String>,
    now: i64,
) -> usize {
    let mut unavailable_files = 0usize;
    for file in state.font_files.values_mut() {
        if file.root_id != root_id
            || !file.is_available
            || seen_paths.contains(&file.canonical_path)
        {
            continue;
        }

        file.is_available = false;
        file.updated_at = now;
        unavailable_files += 1;
    }

    unavailable_files
}

fn remove_alias(state: &mut IndexState, alias_id: u64) {
    let Some(alias) = state.font_aliases.remove(&alias_id) else {
        return;
    };

    let should_remove = if let Some(alias_ids) = state.alias_ids_by_norm.get_mut(&alias.alias_norm)
    {
        alias_ids.retain(|candidate| *candidate != alias_id);
        alias_ids.is_empty()
    } else {
        false
    };

    if should_remove {
        state.alias_ids_by_norm.remove(&alias.alias_norm);
    }
}

fn first_alias_for_name_id(face: &FontFaceAnalysis, name_id: u16) -> Option<String> {
    face.aliases
        .iter()
        .find(|alias| alias.name_id == name_id)
        .map(|alias| alias.value.clone())
}

fn alias_kind(name_id: u16) -> &'static str {
    match name_id {
        name_id::FAMILY => "family",
        name_id::FULL_NAME => "full_name",
        name_id::POST_SCRIPT_NAME => "postscript_name",
        _ => "name",
    }
}

fn alias_priority(name_id: u16) -> i64 {
    match name_id {
        name_id::FAMILY => 300,
        name_id::FULL_NAME => 200,
        name_id::POST_SCRIPT_NAME => 100,
        _ => 0,
    }
}

fn platform_id_value(platform_id: &str) -> Option<i64> {
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

fn next_id(counter: &mut u64) -> u64 {
    if *counter == 0 {
        *counter = 1;
    }

    let id = *counter;
    *counter = counter.saturating_add(1);
    id
}

fn canonicalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn path_to_db_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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

fn i64_to_u16(value: i64) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}
