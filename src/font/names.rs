use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use ttf_parser::{Face, PlatformId, name::Name, name_id};

const ALIAS_NAME_IDS: &[u16] = &[
    name_id::FAMILY,
    name_id::FULL_NAME,
    name_id::POST_SCRIPT_NAME,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontAlias {
    pub name_id: u16,
    pub platform_id: String,
    pub encoding_id: u16,
    pub language_id: u16,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFaceAnalysis {
    pub face_index: u32,
    pub aliases: Vec<FontAlias>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFileAnalysis {
    pub path: PathBuf,
    pub faces: Vec<FontFaceAnalysis>,
}

pub fn analyze_font_file(path: &Path) -> Result<FontFileAnalysis> {
    let data =
        fs::read(path).with_context(|| format!("failed to read font file {}", path.display()))?;

    analyze_font_data(path.to_path_buf(), &data)
}

pub fn analyze_font_data(path: PathBuf, data: &[u8]) -> Result<FontFileAnalysis> {
    let face_count = ttf_parser::fonts_in_collection(data).unwrap_or(1);
    let mut faces = Vec::new();

    for face_index in 0..face_count {
        let face = Face::parse(data, face_index)
            .with_context(|| format!("failed to parse face #{face_index} in {}", path.display()))?;

        faces.push(analyze_face(face_index, &face));
    }

    Ok(FontFileAnalysis { path, faces })
}

pub fn analyze_font_files(paths: &[PathBuf]) -> Result<Vec<FontFileAnalysis>> {
    paths
        .iter()
        .map(|path| analyze_font_file(path))
        .collect::<Result<Vec<_>>>()
}

fn decode_name_record(name: &Name<'_>) -> Option<String> {
    if name.platform_id == PlatformId::Windows {
        if let Some(value) = decode_utf16be_name(name.name) {
            if is_usable_name(&value) {
                return Some(value);
            }
        }
    }

    name.to_string().filter(|value| is_usable_name(value))
}

fn decode_utf16be_name(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() % 2 != 0 {
        return None;
    }

    let units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();

    String::from_utf16(&units).ok()
}

fn is_usable_name(value: &str) -> bool {
    let value = value.trim_matches('\0').trim();

    if value.is_empty() {
        return false;
    }

    !value
        .chars()
        .any(|ch| ch == '\u{FFFD}' || ch == '\0' || (ch.is_control() && !ch.is_whitespace()))
}

fn analyze_face(face_index: u32, face: &Face<'_>) -> FontFaceAnalysis {
    let mut seen = HashSet::new();
    let mut aliases = Vec::new();

    for name in face.names() {
        if !ALIAS_NAME_IDS.contains(&name.name_id) {
            continue;
        }

        let Some(value) = decode_name_record(&name).and_then(|value| normalize_alias(&value))
        else {
            continue;
        };

        let alias_key = value.to_lowercase();
        if !seen.insert((name.name_id, alias_key)) {
            continue;
        }

        aliases.push(FontAlias {
            name_id: name.name_id,
            platform_id: platform_id_name(name.platform_id).to_owned(),
            encoding_id: name.encoding_id,
            language_id: name.language_id,
            value,
        });
    }

    FontFaceAnalysis {
        face_index,
        aliases,
    }
}

fn platform_id_name(platform_id: PlatformId) -> &'static str {
    match platform_id {
        PlatformId::Unicode => "Unicode",
        PlatformId::Macintosh => "Macintosh",
        PlatformId::Iso => "ISO",
        PlatformId::Windows => "Windows",
        PlatformId::Custom => "Custom",
    }
}

fn normalize_alias(value: &str) -> Option<String> {
    let normalized = value.trim().trim_start_matches('@').trim();

    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_owned())
    }
}
