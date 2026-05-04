use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use encoding_rs::UTF_16BE;
use ttf_parser::{Face, PlatformId, name::Name, name_id};

const ALIAS_NAME_IDS: &[u16] = &[
    name_id::FAMILY,
    name_id::TYPOGRAPHIC_FAMILY,
    name_id::WWS_FAMILY,
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
    pub family_name: Option<String>,
    pub subfamily_name: Option<String>,
    pub full_name: Option<String>,
    pub postscript_name: Option<String>,
    pub weight_class: Option<u16>,
    pub is_italic: bool,
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
    if name.platform_id == PlatformId::Windows
        && let Some(value) = decode_utf16be_name(name.name)
        && is_usable_name(&value)
    {
        return Some(value);
    }

    name.to_string().filter(|value| is_usable_name(value))
}

fn decode_utf16be_name(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return None;
    }

    let (value, had_errors) = UTF_16BE.decode_without_bom_handling(bytes);
    if had_errors {
        None
    } else {
        Some(value.into_owned())
    }
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
    let mut family_name = None;
    let mut subfamily_name = None;
    let mut full_name = None;
    let mut postscript_name = None;

    for name in face.names() {
        let Some(value) = decode_name_record(&name).and_then(|value| normalize_alias(&value))
        else {
            continue;
        };

        match name.name_id {
            name_id::FAMILY | name_id::TYPOGRAPHIC_FAMILY | name_id::WWS_FAMILY => {
                prefer_name(&mut family_name, &name, &value);
            }
            name_id::SUBFAMILY | name_id::TYPOGRAPHIC_SUBFAMILY | name_id::WWS_SUBFAMILY => {
                prefer_name(&mut subfamily_name, &name, &value);
            }
            name_id::FULL_NAME => {
                prefer_name(&mut full_name, &name, &value);
            }
            name_id::POST_SCRIPT_NAME => {
                prefer_name(&mut postscript_name, &name, &value);
            }
            _ => {}
        }

        if !ALIAS_NAME_IDS.contains(&name.name_id) {
            continue;
        }

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
        family_name: family_name.map(|(_, value)| value),
        subfamily_name: subfamily_name.map(|(_, value)| value),
        full_name: full_name.map(|(_, value)| value),
        postscript_name: postscript_name.map(|(_, value)| value),
        weight_class: Some(face.weight().to_number()),
        is_italic: face.is_italic() || face.is_oblique(),
        aliases,
    }
}

fn prefer_name(target: &mut Option<(u8, String)>, name: &Name<'_>, value: &str) {
    let priority = name_priority(name);
    if target
        .as_ref()
        .is_none_or(|(current_priority, _)| priority > *current_priority)
    {
        *target = Some((priority, value.to_owned()));
    }
}

fn name_priority(name: &Name<'_>) -> u8 {
    let platform_priority = match (name.platform_id, name.language_id) {
        (PlatformId::Windows, 0x0409) => 5,
        (PlatformId::Windows, _) => 4,
        (PlatformId::Unicode, _) => 3,
        (PlatformId::Macintosh, 0) => 2,
        (PlatformId::Macintosh, _) => 1,
        _ => 0,
    };
    let name_priority = match name.name_id {
        name_id::TYPOGRAPHIC_FAMILY | name_id::TYPOGRAPHIC_SUBFAMILY => 3,
        name_id::WWS_FAMILY | name_id::WWS_SUBFAMILY => 2,
        name_id::FAMILY | name_id::SUBFAMILY => 1,
        _ => 0,
    };

    platform_priority * 10 + name_priority
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
    let normalized = value.trim();

    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_owned())
    }
}
