use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::{font::index::ResolveReport, session::LoadSummary};

#[derive(Debug, Clone, Default)]
pub struct SubtitleLoadView {
    pub subtitle_count: usize,
    pub required_alias_count: usize,
    pub declared_but_unused_alias_count: usize,
    pub skipped_system_alias_count: usize,
    pub loaded_local_font_count: usize,
    pub failed_local_font_count: usize,
    pub missing_alias_count: usize,
    pub local_groups: Vec<LocalFontGroup>,
    pub failed_local_fonts: Vec<FailedLocalFont>,
    pub declared_but_unused_aliases: Vec<String>,
    pub system_aliases: Vec<String>,
    pub missing_aliases: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LocalFontGroup {
    pub font_path: PathBuf,
    pub loaded: bool,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FailedLocalFont {
    pub font_path: PathBuf,
    pub error: String,
}

impl SubtitleLoadView {
    pub fn from_resolve_report(
        subtitle_count: usize,
        required_alias_count: usize,
        declared_but_unused_aliases: Vec<String>,
        system_aliases: Vec<String>,
        report: ResolveReport,
        load_summary: &LoadSummary,
    ) -> Self {
        let loaded_paths = path_set(load_summary.loaded.iter().map(|font| &font.path));
        let mut groups = BTreeMap::<PathBuf, LocalFontGroup>::new();

        for resolved in report.matched {
            for font_match in resolved.matches {
                let font_path = font_match.canonical_path.clone();
                let group = groups
                    .entry(font_path.clone())
                    .or_insert_with(|| LocalFontGroup {
                        loaded: loaded_paths.contains(&font_path),
                        font_path: font_path.clone(),
                        aliases: Vec::new(),
                    });

                push_unique_alias(&mut group.aliases, font_match.matched_alias);
            }
        }

        let mut local_groups = groups.into_values().collect::<Vec<_>>();
        local_groups.retain(|group| group.loaded);

        let loaded_local_font_count = local_groups.iter().filter(|group| group.loaded).count();
        let failed_local_fonts = load_summary
            .failed
            .iter()
            .map(|failure| FailedLocalFont {
                font_path: failure.path.clone(),
                error: failure.error.clone(),
            })
            .collect::<Vec<_>>();
        let failed_local_font_count = failed_local_fonts.len();
        let missing_aliases = report.missing;
        let missing_alias_count = missing_aliases.len();
        let declared_but_unused_alias_count = declared_but_unused_aliases.len();
        let skipped_system_alias_count = system_aliases.len();

        Self {
            subtitle_count,
            required_alias_count,
            declared_but_unused_alias_count,
            skipped_system_alias_count,
            loaded_local_font_count,
            failed_local_font_count,
            missing_alias_count,
            local_groups,
            failed_local_fonts,
            declared_but_unused_aliases,
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
        if self.failed_local_font_count > 0 {
            push_line(
                &mut output,
                format!("Failed local fonts: {}", self.failed_local_font_count),
            );
        }
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
        push_line(
            &mut output,
            format!(
                "Declared but unused: {}",
                self.declared_but_unused_alias_count
            ),
        );
        output.push('\n');

        push_line(&mut output, "[LOCAL LOADED]".to_owned());
        for group in &self.local_groups {
            push_line(
                &mut output,
                format!("- {}", font_file_label(&group.font_path)),
            );

            for alias in &group.aliases {
                push_line(&mut output, format!("  {alias}"));
            }
        }
        output.push('\n');

        if !self.failed_local_fonts.is_empty() {
            push_line(&mut output, "[LOCAL LOAD FAILED]".to_owned());
            for failure in &self.failed_local_fonts {
                push_line(
                    &mut output,
                    format!("- {}", font_file_label(&failure.font_path)),
                );

                for line in failure.error.lines() {
                    push_line(&mut output, format!("  {line}"));
                }
            }
            output.push('\n');
        }

        push_line(&mut output, "[SYSTEM SKIPPED]".to_owned());
        for alias in &self.system_aliases {
            push_line(&mut output, format!("- {alias}"));
        }
        output.push('\n');

        push_line(&mut output, "[MISSING]".to_owned());
        for alias in &self.missing_aliases {
            push_line(&mut output, format!("- {alias}"));
        }
        output.push('\n');

        push_line(&mut output, "[DECLARED BUT UNUSED]".to_owned());
        for alias in &self.declared_but_unused_aliases {
            push_line(&mut output, format!("- {alias}"));
        }

        output
    }
}

fn path_set<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> BTreeSet<PathBuf> {
    paths.cloned().collect()
}

fn push_unique_alias(aliases: &mut Vec<String>, alias: String) {
    if !aliases.iter().any(|existing| existing == &alias) {
        aliases.push(alias);
    }
}

fn font_file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn push_line(output: &mut String, line: String) {
    output.push_str(&line);
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::SubtitleLoadView;
    use crate::{
        font::index::{FontMatch, ResolveReport, ResolvedFont},
        session::{FailedFont, LoadSummary, LoadedFont},
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn render_text_uses_compact_sections_and_deduplicated_aliases() {
        let loaded_path = PathBuf::from("Fonts").join("Alpha.ttf");
        let failed_path = PathBuf::from("Fonts").join("Beta.ttf");
        let report = ResolveReport {
            matched: vec![
                ResolvedFont {
                    requested_name: "Alpha Family".to_owned(),
                    matches: vec![
                        font_match(&loaded_path, "Alpha Family", "family"),
                        font_match(&loaded_path, "Alpha Family", "full_name"),
                        font_match(&loaded_path, "AlphaPS", "postscript_name"),
                    ],
                },
                ResolvedFont {
                    requested_name: "Beta Family".to_owned(),
                    matches: vec![font_match(&failed_path, "Beta Family", "family")],
                },
            ],
            missing: vec!["Missing Serif".to_owned()],
            unique_font_paths: vec![loaded_path.clone(), failed_path.clone()],
        };
        let load_summary = LoadSummary {
            loaded: vec![LoadedFont { path: loaded_path }],
            failed: Vec::new(),
        };

        let view = SubtitleLoadView::from_resolve_report(
            13,
            4,
            vec!["Unused Display".to_owned()],
            vec!["System Sans".to_owned()],
            report,
            &load_summary,
        );

        assert_eq!(
            view.render_text(),
            concat!(
                "Loaded local fonts: 1\n",
                "Skipped system fonts: 1\n",
                "Missing fonts: 1\n",
                "Subtitle files: 13\n",
                "Required aliases: 4\n",
                "Declared but unused: 1\n",
                "\n",
                "[LOCAL LOADED]\n",
                "- Alpha.ttf\n",
                "  Alpha Family\n",
                "  AlphaPS\n",
                "\n",
                "[SYSTEM SKIPPED]\n",
                "- System Sans\n",
                "\n",
                "[MISSING]\n",
                "- Missing Serif\n",
                "\n",
                "[DECLARED BUT UNUSED]\n",
                "- Unused Display\n",
            )
        );
    }

    #[test]
    fn render_text_includes_failed_load_section_when_needed() {
        let loaded_path = PathBuf::from("Fonts").join("Alpha.ttf");
        let failed_path = PathBuf::from("Fonts").join("Beta.ttf");
        let report = ResolveReport {
            matched: vec![
                ResolvedFont {
                    requested_name: "Alpha Family".to_owned(),
                    matches: vec![font_match(&loaded_path, "Alpha Family", "family")],
                },
                ResolvedFont {
                    requested_name: "Beta Family".to_owned(),
                    matches: vec![font_match(&failed_path, "Beta Family", "family")],
                },
            ],
            missing: Vec::new(),
            unique_font_paths: vec![loaded_path.clone(), failed_path.clone()],
        };
        let load_summary = LoadSummary {
            loaded: vec![LoadedFont { path: loaded_path }],
            failed: vec![FailedFont {
                path: failed_path,
                error: "failed to add font resource".to_owned(),
            }],
        };

        let rendered = SubtitleLoadView::from_resolve_report(
            1,
            2,
            Vec::new(),
            Vec::new(),
            report,
            &load_summary,
        )
        .render_text();

        assert!(rendered.contains("Failed local fonts: 1\n"));
        assert!(rendered.contains("[LOCAL LOAD FAILED]\n"));
        assert!(rendered.contains("- Beta.ttf\n"));
        assert!(rendered.contains("  failed to add font resource\n"));
    }

    fn font_match(path: &Path, matched_alias: &str, alias_kind: &str) -> FontMatch {
        FontMatch {
            requested_name: matched_alias.to_owned(),
            matched_alias: matched_alias.to_owned(),
            alias_kind: alias_kind.to_owned(),
            font_path: path.to_path_buf(),
            relative_path: path.display().to_string(),
            name_id: 0,
            platform_id: None,
            encoding_id: None,
            language_id: None,
            canonical_path: path.to_path_buf(),
            face_index: 0,
            family_name: None,
            subfamily_name: None,
            full_name: None,
            postscript_name: None,
            weight_class: None,
            is_italic: false,
        }
    }
}
