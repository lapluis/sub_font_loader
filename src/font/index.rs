use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use ttf_parser::name_id;
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

use crate::discover;

use super::{FontAlias, FontFaceAnalysis, analyze_font_file};

pub struct FontIndex {
    conn: Connection,
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
    id: i64,
    file_size: i64,
    modified_at: i64,
}

impl FontIndex {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("failed to open font index database {}", db_path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("failed to enable SQLite foreign keys")?;
        initialize_schema(&conn)?;
        Ok(Self { conn })
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
        let tx = self
            .conn
            .transaction()
            .context("failed to start font index transaction")?;
        let root_id = upsert_scan_root(&tx, &root_path, now)?;
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

            if let Some(existing) = find_existing_font_file(&tx, &canonical_path_text)? {
                if existing.file_size == file_size && existing.modified_at == modified_at {
                    mark_font_file_seen(&tx, existing.id, root_id, &path_text, &extension, now)?;
                    summary.skipped_unchanged_files += 1;
                    continue;
                }
            }

            let file_id = upsert_font_file(
                &tx,
                root_id,
                &path_text,
                &canonical_path_text,
                &extension,
                file_size,
                modified_at,
                now,
            )?;
            remove_indexed_faces(&tx, file_id)?;

            match analyze_font_file(&path) {
                Ok(analysis) => {
                    for face in &analysis.faces {
                        insert_font_face(&tx, file_id, face, now)?;
                    }
                    summary.indexed_files += 1;
                }
                Err(error) => {
                    summary.failed_files += 1;
                    eprintln!("Warning: failed to analyze {}: {error:#}", path.display());
                }
            }
        }

        summary.unavailable_files = mark_unavailable_files(&tx, root_id, &seen_paths, now)?;
        tx.execute(
            "UPDATE scan_roots
             SET last_scanned_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![now, root_id],
        )
        .context("failed to update scan root timestamp")?;
        tx.commit()
            .context("failed to commit font index transaction")?;

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
        let tx = self
            .conn
            .transaction()
            .context("failed to start font index clear transaction")?;

        tx.execute("DELETE FROM font_aliases", [])
            .context("failed to clear font aliases")?;
        tx.execute("DELETE FROM font_faces", [])
            .context("failed to clear font faces")?;
        tx.execute("DELETE FROM font_files", [])
            .context("failed to clear font files")?;
        tx.execute("DELETE FROM scan_roots", [])
            .context("failed to clear scan roots")?;
        tx.execute("DELETE FROM index_meta WHERE key = 'active_font_root'", [])
            .context("failed to clear active font root metadata")?;

        tx.commit()
            .context("failed to commit font index clear transaction")?;
        Ok(())
    }

    pub fn active_font_root(&self) -> Result<Option<PathBuf>> {
        self.conn
            .query_row(
                "SELECT value FROM index_meta WHERE key = 'active_font_root'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to read active font root metadata")
            .map(|value| value.map(PathBuf::from))
    }

    pub fn set_active_font_root(&self, root: &Path) -> Result<()> {
        let root_path = canonicalize_font_root(root)?;
        self.conn
            .execute(
                "INSERT INTO index_meta (key, value)
                 VALUES ('active_font_root', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![path_to_db_text(&root_path)],
            )
            .context("failed to write active font root metadata")?;
        Ok(())
    }

    pub fn summary_for_root(&self, root: &Path) -> Result<ScanSummary> {
        let root_path = canonicalize_font_root(root)?;
        let root_text = path_to_db_text(&root_path);

        let scanned_files = self
            .conn
            .query_row(
                "SELECT COUNT(*)
                 FROM font_files files
                 JOIN scan_roots roots ON roots.id = files.root_id
                 WHERE roots.root_path = ?1
                   AND files.is_available = 1",
                params![&root_text],
                |row| row.get::<_, i64>(0),
            )
            .context("failed to count indexed font files")
            .map(i64_to_usize)?;

        let indexed_files = self
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT files.id)
                 FROM font_files files
                 JOIN scan_roots roots ON roots.id = files.root_id
                 JOIN font_faces faces ON faces.file_id = files.id
                 WHERE roots.root_path = ?1
                   AND files.is_available = 1",
                params![&root_text],
                |row| row.get::<_, i64>(0),
            )
            .context("failed to count indexed font faces")
            .map(i64_to_usize)?;

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
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    aliases.alias_raw,
                    aliases.alias_kind,
                    files.path,
                    files.canonical_path,
                    faces.face_index,
                    faces.family_name,
                    faces.subfamily_name,
                    faces.full_name,
                    faces.postscript_name,
                    faces.weight_class,
                    faces.is_italic
                 FROM font_aliases aliases
                 JOIN font_faces faces ON faces.id = aliases.face_id
                 JOIN font_files files ON files.id = faces.file_id
                 JOIN scan_roots roots ON roots.id = files.root_id
                 WHERE aliases.alias_norm = ?1
                   AND files.is_available = 1
                 ORDER BY roots.priority DESC,
                          aliases.priority DESC,
                          files.path ASC,
                          faces.face_index ASC,
                          aliases.alias_raw ASC",
            )
            .context("failed to prepare font alias query")?;

        let rows = stmt
            .query_map(params![alias_norm], |row| {
                let path_text: String = row.get(2)?;
                let canonical_path_text: String = row.get(3)?;
                let face_index: i64 = row.get(4)?;
                let weight_class: Option<i64> = row.get(9)?;
                let is_italic: i64 = row.get(10)?;

                Ok(FontMatch {
                    requested_name: requested_name.clone(),
                    matched_alias: row.get(0)?,
                    alias_kind: row.get(1)?,
                    font_path: PathBuf::from(path_text),
                    canonical_path: PathBuf::from(canonical_path_text),
                    face_index: i64_to_u32(face_index),
                    family_name: row.get(5)?,
                    subfamily_name: row.get(6)?,
                    full_name: row.get(7)?,
                    postscript_name: row.get(8)?,
                    weight_class: weight_class.map(i64_to_u16),
                    is_italic: is_italic != 0,
                })
            })
            .context("failed to query font aliases")?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to read font alias matches")
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

        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    aliases.alias_raw,
                    aliases.alias_norm,
                    aliases.alias_kind,
                    faces.family_name,
                    faces.full_name,
                    faces.face_index,
                    files.path,
                    files.is_available
                 FROM font_aliases aliases
                 JOIN font_faces faces ON faces.id = aliases.face_id
                 JOIN font_files files ON files.id = faces.file_id
                 ORDER BY aliases.alias_norm ASC,
                          files.path ASC,
                          faces.face_index ASC,
                          aliases.alias_kind ASC",
            )
            .context("failed to prepare alias CSV export")?;

        let rows = stmt
            .query_map([], |row| {
                Ok([
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    row.get::<_, i64>(5)?.to_string(),
                    row.get::<_, String>(6)?,
                    (row.get::<_, i64>(7)? != 0).to_string(),
                ])
            })
            .context("failed to query alias CSV rows")?;

        for row in rows {
            csv_writer
                .write_record(row.context("failed to read alias CSV row")?)
                .context("failed to write alias CSV row")?;
        }

        csv_writer
            .flush()
            .context("failed to flush alias CSV writer")?;
        Ok(())
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

fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS index_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS scan_roots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            root_path TEXT NOT NULL UNIQUE,
            root_kind TEXT NOT NULL DEFAULT 'directory',
            priority INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_scanned_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS font_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            root_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            canonical_path TEXT NOT NULL,
            extension TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            modified_at INTEGER NOT NULL,
            content_hash TEXT,
            is_available INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(root_id) REFERENCES scan_roots(id)
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_font_files_canonical_path
        ON font_files(canonical_path);

        CREATE INDEX IF NOT EXISTS idx_font_files_root_id
        ON font_files(root_id);

        CREATE TABLE IF NOT EXISTS font_faces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER NOT NULL,
            face_index INTEGER NOT NULL DEFAULT 0,
            family_name TEXT,
            subfamily_name TEXT,
            full_name TEXT,
            postscript_name TEXT,
            weight_class INTEGER,
            width_class INTEGER,
            is_italic INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(file_id) REFERENCES font_files(id)
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_font_faces_file_face
        ON font_faces(file_id, face_index);

        CREATE INDEX IF NOT EXISTS idx_font_faces_file_id
        ON font_faces(file_id);

        CREATE TABLE IF NOT EXISTS font_aliases (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            face_id INTEGER NOT NULL,
            alias_raw TEXT NOT NULL,
            alias_norm TEXT NOT NULL,
            alias_kind TEXT NOT NULL,
            language TEXT,
            platform_id INTEGER,
            name_id INTEGER,
            priority INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(face_id) REFERENCES font_faces(id)
        );

        CREATE INDEX IF NOT EXISTS idx_font_aliases_norm
        ON font_aliases(alias_norm);

        CREATE INDEX IF NOT EXISTS idx_font_aliases_face_id
        ON font_aliases(face_id);

        CREATE UNIQUE INDEX IF NOT EXISTS idx_font_aliases_unique
        ON font_aliases(
            face_id,
            alias_norm,
            alias_kind,
            COALESCE(language, ''),
            COALESCE(platform_id, -1),
            COALESCE(name_id, -1)
        );
        ",
    )
    .context("failed to initialize font index schema")
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

fn upsert_scan_root(tx: &Transaction<'_>, root_path: &Path, now: i64) -> Result<i64> {
    let root_path = path_to_db_text(root_path);
    tx.execute(
        "INSERT INTO scan_roots (root_path, root_kind, priority, created_at, updated_at)
         VALUES (?1, 'directory', 0, ?2, ?2)
         ON CONFLICT(root_path) DO UPDATE SET
            updated_at = excluded.updated_at",
        params![root_path, now],
    )
    .context("failed to upsert scan root")?;

    tx.query_row(
        "SELECT id FROM scan_roots WHERE root_path = ?1",
        params![root_path],
        |row| row.get(0),
    )
    .context("failed to read scan root id")
}

fn find_existing_font_file(
    tx: &Transaction<'_>,
    canonical_path: &str,
) -> Result<Option<ExistingFontFile>> {
    tx.query_row(
        "SELECT id, file_size, modified_at
         FROM font_files
         WHERE canonical_path = ?1",
        params![canonical_path],
        |row| {
            Ok(ExistingFontFile {
                id: row.get(0)?,
                file_size: row.get(1)?,
                modified_at: row.get(2)?,
            })
        },
    )
    .optional()
    .context("failed to find indexed font file")
}

fn mark_font_file_seen(
    tx: &Transaction<'_>,
    file_id: i64,
    root_id: i64,
    path: &str,
    extension: &str,
    now: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE font_files
         SET root_id = ?1,
             path = ?2,
             extension = ?3,
             is_available = 1,
             updated_at = ?4
         WHERE id = ?5",
        params![root_id, path, extension, now, file_id],
    )
    .context("failed to mark indexed font file as available")?;
    Ok(())
}

fn upsert_font_file(
    tx: &Transaction<'_>,
    root_id: i64,
    path: &str,
    canonical_path: &str,
    extension: &str,
    file_size: i64,
    modified_at: i64,
    now: i64,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO font_files (
            root_id,
            path,
            canonical_path,
            extension,
            file_size,
            modified_at,
            is_available,
            created_at,
            updated_at
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)
         ON CONFLICT(canonical_path) DO UPDATE SET
            root_id = excluded.root_id,
            path = excluded.path,
            extension = excluded.extension,
            file_size = excluded.file_size,
            modified_at = excluded.modified_at,
            is_available = 1,
            updated_at = excluded.updated_at",
        params![
            root_id,
            path,
            canonical_path,
            extension,
            file_size,
            modified_at,
            now
        ],
    )
    .context("failed to upsert font file")?;

    tx.query_row(
        "SELECT id FROM font_files WHERE canonical_path = ?1",
        params![canonical_path],
        |row| row.get(0),
    )
    .context("failed to read font file id")
}

fn remove_indexed_faces(tx: &Transaction<'_>, file_id: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM font_aliases
         WHERE face_id IN (SELECT id FROM font_faces WHERE file_id = ?1)",
        params![file_id],
    )
    .context("failed to remove old font aliases")?;
    tx.execute(
        "DELETE FROM font_faces WHERE file_id = ?1",
        params![file_id],
    )
    .context("failed to remove old font faces")?;
    Ok(())
}

fn insert_font_face(
    tx: &Transaction<'_>,
    file_id: i64,
    face: &FontFaceAnalysis,
    now: i64,
) -> Result<()> {
    let family_name = first_alias_for_name_id(face, name_id::FAMILY);
    let subfamily_name: Option<String> = None;
    let full_name = first_alias_for_name_id(face, name_id::FULL_NAME);
    let postscript_name = first_alias_for_name_id(face, name_id::POST_SCRIPT_NAME);
    let weight_class: Option<i64> = None;
    let width_class: Option<i64> = None;
    let is_italic = 0i64;

    tx.execute(
        "INSERT INTO font_faces (
            file_id,
            face_index,
            family_name,
            subfamily_name,
            full_name,
            postscript_name,
            weight_class,
            width_class,
            is_italic,
            created_at,
            updated_at
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            file_id,
            i64::from(face.face_index),
            family_name,
            subfamily_name,
            full_name,
            postscript_name,
            weight_class,
            width_class,
            is_italic,
            now
        ],
    )
    .context("failed to insert font face")?;

    let face_id = tx.last_insert_rowid();
    insert_font_aliases(tx, face_id, &face.aliases, now)
}

fn insert_font_aliases(
    tx: &Transaction<'_>,
    face_id: i64,
    aliases: &[FontAlias],
    now: i64,
) -> Result<()> {
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

            tx.execute(
                "INSERT OR IGNORE INTO font_aliases (
                    face_id,
                    alias_raw,
                    alias_norm,
                    alias_kind,
                    language,
                    platform_id,
                    name_id,
                    priority,
                    created_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    face_id,
                    alias_raw,
                    alias_norm,
                    alias_kind,
                    language,
                    platform_id,
                    name_id,
                    priority,
                    now
                ],
            )
            .context("failed to insert font alias")?;
        }
    }

    Ok(())
}

fn mark_unavailable_files(
    tx: &Transaction<'_>,
    root_id: i64,
    seen_paths: &HashSet<String>,
    now: i64,
) -> Result<usize> {
    let indexed_files = {
        let mut stmt = tx
            .prepare(
                "SELECT id, canonical_path
                 FROM font_files
                 WHERE root_id = ?1
                   AND is_available = 1",
            )
            .context("failed to prepare unavailable font query")?;

        let rows = stmt
            .query_map(params![root_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .context("failed to query unavailable font candidates")?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to read unavailable font candidates")?
    };

    let mut unavailable_files = 0usize;
    for (file_id, canonical_path) in indexed_files {
        if seen_paths.contains(&canonical_path) {
            continue;
        }

        tx.execute(
            "UPDATE font_files
             SET is_available = 0,
                 updated_at = ?1
             WHERE id = ?2",
            params![now, file_id],
        )
        .context("failed to mark font file unavailable")?;
        unavailable_files += 1;
    }

    Ok(unavailable_files)
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

fn i64_to_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn i64_to_u16(value: i64) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn i64_to_usize(value: i64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}
