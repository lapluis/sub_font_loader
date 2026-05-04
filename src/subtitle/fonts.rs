use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use super::{ass, encoding};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubtitleFontUsage {
    pub declared_fonts: BTreeSet<String>,
    pub required_fonts: BTreeSet<String>,
    pub declared_but_unused_fonts: BTreeSet<String>,
    pub inline_fonts: BTreeSet<String>,
    pub missing_styles: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubtitleFontReport {
    pub files: BTreeMap<PathBuf, SubtitleFontUsage>,
    pub declared_fonts: BTreeSet<String>,
    pub required_fonts: BTreeSet<String>,
    pub declared_but_unused_fonts: BTreeSet<String>,
    pub inline_fonts: BTreeSet<String>,
    pub missing_styles: BTreeSet<String>,
}

pub fn analyze_ass_text(text: &str) -> SubtitleFontUsage {
    let document = ass::parse_ass(text);
    let mut usage = SubtitleFontUsage::default();
    let mut style_fonts = BTreeMap::new();

    for style in document.styles {
        let Some(font_name) = normalize_font_name(&style.font_name) else {
            continue;
        };

        insert_ci(&mut usage.declared_fonts, font_name.clone());
        style_fonts.insert(style_key(&style.name), font_name);
    }

    for dialogue in document.dialogues {
        add_style_font(&dialogue.style, &style_fonts, &mut usage);
        analyze_override_tags(&dialogue.text, &dialogue.style, &style_fonts, &mut usage);
    }

    usage.declared_but_unused_fonts =
        declared_but_unused_fonts(&usage.declared_fonts, &usage.required_fonts);

    usage
}

pub fn analyze_subtitle_fonts(path: &Path) -> Result<SubtitleFontUsage> {
    let text = encoding::read_subtitle_text(path)?;
    Ok(analyze_ass_text(&text))
}

pub fn analyze_subtitle_font_report(paths: &[PathBuf]) -> Result<SubtitleFontReport> {
    let mut report = SubtitleFontReport::default();

    for path in paths {
        let usage = analyze_subtitle_fonts(path)
            .with_context(|| format!("failed to analyze {}", path.display()))?;

        extend_ci(&mut report.declared_fonts, &usage.declared_fonts);
        extend_ci(&mut report.required_fonts, &usage.required_fonts);
        extend_ci(&mut report.inline_fonts, &usage.inline_fonts);
        extend_ci(&mut report.missing_styles, &usage.missing_styles);
        report.files.insert(path.clone(), usage);
    }

    report.declared_but_unused_fonts =
        declared_but_unused_fonts(&report.declared_fonts, &report.required_fonts);

    Ok(report)
}

fn analyze_override_tags(
    text: &str,
    original_style: &str,
    style_fonts: &BTreeMap<String, String>,
    usage: &mut SubtitleFontUsage,
) {
    let mut remaining = text;

    while let Some(open_index) = remaining.find('{') {
        let block_start = open_index + 1;
        let Some(close_offset) = remaining[block_start..].find('}') else {
            break;
        };

        let block_end = block_start + close_offset;
        parse_override_block(
            &remaining[block_start..block_end],
            original_style,
            style_fonts,
            usage,
        );
        remaining = &remaining[block_end + 1..];
    }
}

fn parse_override_block(
    block: &str,
    original_style: &str,
    style_fonts: &BTreeMap<String, String>,
    usage: &mut SubtitleFontUsage,
) {
    let mut index = 0;

    while let Some(command_offset) = block[index..].find('\\') {
        index += command_offset + 1;
        let command = &block[index..];

        if starts_with_ci(command, "fn") {
            let (font_name, consumed) = read_command_argument(&command[2..]);
            if let Some(font_name) = normalize_font_name(font_name) {
                insert_ci(&mut usage.inline_fonts, font_name.clone());
                insert_ci(&mut usage.required_fonts, font_name);
            }
            index += 2 + consumed;
            continue;
        }

        if starts_with_ci(command, "r") {
            let (style_name, consumed) = read_command_argument(&command[1..]);
            let reset_style = if style_name.trim().is_empty() {
                original_style
            } else {
                style_name
            };
            add_style_font(reset_style, style_fonts, usage);
            index += 1 + consumed;
            continue;
        }
    }
}

fn read_command_argument(value: &str) -> (&str, usize) {
    match value.find('\\') {
        Some(index) => (&value[..index], index),
        None => (value, value.len()),
    }
}

fn add_style_font(
    style_name: &str,
    style_fonts: &BTreeMap<String, String>,
    usage: &mut SubtitleFontUsage,
) {
    let style_name = style_name.trim();

    if style_name.is_empty() {
        return;
    }

    if let Some(font_name) = style_fonts.get(&style_key(style_name)) {
        insert_ci(&mut usage.required_fonts, font_name.clone());
        return;
    }

    insert_ci(&mut usage.missing_styles, style_name.to_owned());

    if let Some(default_font) = style_fonts.get("default") {
        insert_ci(&mut usage.required_fonts, default_font.clone());
    }
}

fn normalize_font_name(value: &str) -> Option<String> {
    let mut value = value.trim();

    if let Some(stripped) = value.strip_prefix('@') {
        value = stripped.trim_start();
    }

    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn style_key(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn starts_with_ci(value: &str, prefix: &str) -> bool {
    let Some(actual_prefix) = value.get(..prefix.len()) else {
        return false;
    };

    actual_prefix.eq_ignore_ascii_case(prefix)
}

fn extend_ci(target: &mut BTreeSet<String>, source: &BTreeSet<String>) {
    for value in source {
        insert_ci(target, value.clone());
    }
}

fn declared_but_unused_fonts(
    declared_fonts: &BTreeSet<String>,
    required_fonts: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut unused_fonts = BTreeSet::new();

    for font_name in declared_fonts {
        if !contains_ci(required_fonts, font_name) {
            insert_ci(&mut unused_fonts, font_name.clone());
        }
    }

    unused_fonts
}

fn insert_ci(target: &mut BTreeSet<String>, value: String) {
    if !target
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&value))
    {
        target.insert(value);
    }
}

fn contains_ci(values: &BTreeSet<String>, value: &str) -> bool {
    values
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(value))
}
