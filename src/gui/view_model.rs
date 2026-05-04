use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
};

use crate::{
    font::index::ResolveReport,
    session::{FailedFont, LoadSummary},
};

#[derive(Debug, Clone, Default)]
pub struct SubtitleLoadView {
    pub subtitle_count: usize,
    pub required_alias_count: usize,
    pub skipped_system_alias_count: usize,
    pub loaded_local_font_count: usize,
    pub missing_alias_count: usize,
    pub local_groups: Vec<LocalFontGroup>,
    pub system_aliases: Vec<String>,
    pub missing_aliases: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LocalFontGroup {
    pub font_path: PathBuf,
    pub loaded: bool,
    pub load_error: Option<String>,
    pub aliases: Vec<AliasMatchView>,
}

#[derive(Debug, Clone, Default)]
pub struct AliasMatchView {
    pub requested_name: String,
    pub matched_alias: String,
    pub alias_kind: String,
    pub face_index: u32,
}

impl SubtitleLoadView {
    pub fn from_resolve_report(
        subtitle_count: usize,
        required_alias_count: usize,
        system_aliases: Vec<String>,
        report: ResolveReport,
        load_summary: &LoadSummary,
    ) -> Self {
        let loaded_paths = path_set(load_summary.loaded.iter().map(|font| &font.path));
        let failed_paths = failed_path_map(&load_summary.failed);
        let mut groups = BTreeMap::<PathBuf, LocalFontGroup>::new();

        for resolved in report.matched {
            for font_match in resolved.matches {
                let font_path = font_match.canonical_path.clone();
                let group = groups
                    .entry(font_path.clone())
                    .or_insert_with(|| LocalFontGroup {
                        loaded: loaded_paths.contains(&font_path),
                        load_error: failed_paths.get(&font_path).cloned(),
                        font_path: font_path.clone(),
                        aliases: Vec::new(),
                    });

                group.aliases.push(AliasMatchView {
                    requested_name: resolved.requested_name.clone(),
                    matched_alias: font_match.matched_alias,
                    alias_kind: font_match.alias_kind,
                    face_index: font_match.face_index,
                });
            }
        }

        let mut local_groups = groups.into_values().collect::<Vec<_>>();
        for group in &mut local_groups {
            group
                .aliases
                .sort_by(|left, right| left.requested_name.cmp(&right.requested_name));
        }

        let loaded_local_font_count = local_groups.iter().filter(|group| group.loaded).count();
        let missing_aliases = report.missing;
        let missing_alias_count = missing_aliases.len();
        let skipped_system_alias_count = system_aliases.len();

        Self {
            subtitle_count,
            required_alias_count,
            skipped_system_alias_count,
            loaded_local_font_count,
            missing_alias_count,
            local_groups,
            system_aliases,
            missing_aliases,
        }
    }

    pub fn render_text(&self) -> String {
        let mut output = String::new();

        push_line(
            &mut output,
            format!("Loaded local fonts: {}", self.loaded_local_font_count),
        );
        push_line(
            &mut output,
            format!("Skipped system fonts: {}", self.skipped_system_alias_count),
        );
        push_line(
            &mut output,
            format!("Missing fonts: {}", self.missing_alias_count),
        );
        push_line(
            &mut output,
            format!("Subtitle files: {}", self.subtitle_count),
        );
        push_line(
            &mut output,
            format!("Required aliases: {}", self.required_alias_count),
        );
        output.push('\n');

        for group in &self.local_groups {
            if group.loaded {
                push_line(
                    &mut output,
                    format!("[LOCAL LOADED] {}", group.font_path.display()),
                );
            } else {
                push_line(
                    &mut output,
                    format!("[LOCAL LOAD FAILED] {}", group.font_path.display()),
                );
                if let Some(error) = &group.load_error {
                    push_line(&mut output, format!("  Error: {error}"));
                }
            }

            for alias in &group.aliases {
                push_line(
                    &mut output,
                    format!("  - Requested: {}", alias.requested_name),
                );
                push_line(
                    &mut output,
                    format!("    Matched alias: {}", alias.matched_alias),
                );
                push_line(&mut output, format!("    Alias kind: {}", alias.alias_kind));
                push_line(&mut output, format!("    Face index: {}", alias.face_index));
            }
            output.push('\n');
        }

        if !self.system_aliases.is_empty() {
            push_line(&mut output, "[SYSTEM SKIPPED]".to_owned());
            for alias in &self.system_aliases {
                push_line(&mut output, format!("  - {alias}"));
            }
            output.push('\n');
        }

        if !self.missing_aliases.is_empty() {
            push_line(&mut output, "[MISSING]".to_owned());
            for alias in &self.missing_aliases {
                push_line(&mut output, format!("  - {alias}"));
            }
        }

        output
    }
}

fn path_set<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> BTreeSet<PathBuf> {
    paths.cloned().collect()
}

fn failed_path_map(failed: &[FailedFont]) -> HashMap<PathBuf, String> {
    failed
        .iter()
        .map(|font| (font.path.clone(), font.error.clone()))
        .collect()
}

fn push_line(output: &mut String, line: String) {
    output.push_str(&line);
    output.push('\n');
}
