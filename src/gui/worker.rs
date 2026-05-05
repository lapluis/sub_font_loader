use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::{
    discover,
    font::index::{
        FontIndex, FontIndexInspection, ScanSummary, canonicalize_font_root,
        inspect_existing_index, normalize_font_name,
    },
    session::FontSession,
    subtitle,
};

use super::{system_fonts, view_model::SubtitleLoadView};

#[derive(Debug)]
pub enum GuiTask {
    EnsureIndexOnStartup {
        font_root: PathBuf,
        db_path: PathBuf,
    },
    UpdateIndex {
        font_root: PathBuf,
        db_path: PathBuf,
    },
    SwitchFontRoot {
        old_session: FontSession,
        font_root: PathBuf,
        db_path: PathBuf,
    },
    LoadSubtitleInputs {
        inputs: Vec<PathBuf>,
        font_root: PathBuf,
        db_path: PathBuf,
        avoid_system_fonts: bool,
        current_session: FontSession,
    },
    UnloadFonts {
        session: FontSession,
    },
}

#[derive(Debug)]
pub enum GuiEvent {
    IndexReady {
        summary: ScanSummary,
    },
    IndexUnavailable {
        inspection: FontIndexInspection,
    },
    IndexFailed {
        error: String,
    },
    FontRootSwitched {
        summary: ScanSummary,
        session: FontSession,
        unloaded_count: usize,
    },
    FontsLoaded {
        view: SubtitleLoadView,
        session: FontSession,
    },
    FontsUnloaded {
        session: FontSession,
        unloaded_count: usize,
    },
    Error(String),
}

pub fn run_task(task: GuiTask) -> GuiEvent {
    let is_index_task = matches!(
        task,
        GuiTask::EnsureIndexOnStartup { .. }
            | GuiTask::UpdateIndex { .. }
            | GuiTask::SwitchFontRoot { .. }
    );

    match run_task_inner(task) {
        Ok(event) => event,
        Err(error) if is_index_task => GuiEvent::IndexFailed {
            error: format!("{error:#}"),
        },
        Err(error) => GuiEvent::Error(format!("{error:#}")),
    }
}

pub fn inspect_index_status(font_root: &Path, db_path: &Path) -> Result<FontIndexInspection> {
    inspect_existing_index(db_path, font_root)
}

fn run_task_inner(task: GuiTask) -> Result<GuiEvent> {
    match task {
        GuiTask::EnsureIndexOnStartup { font_root, db_path } => {
            match ensure_index_on_startup(&font_root, &db_path)? {
                FontIndexInspection::Ready(summary) => Ok(GuiEvent::IndexReady { summary }),
                inspection => Ok(GuiEvent::IndexUnavailable { inspection }),
            }
        }
        GuiTask::UpdateIndex { font_root, db_path } => {
            let summary = update_index(&font_root, &db_path)?;
            Ok(GuiEvent::IndexReady { summary })
        }
        GuiTask::SwitchFontRoot {
            mut old_session,
            font_root,
            db_path,
        } => {
            let unload_summary = old_session
                .unload_all()
                .context("failed to unload fonts before switching font directory")?;
            let mut index = FontIndex::open(&db_path)?;
            let summary = index.rebuild_root(&font_root)?;
            Ok(GuiEvent::FontRootSwitched {
                summary,
                session: FontSession::new(),
                unloaded_count: unload_summary.unloaded.len(),
            })
        }
        GuiTask::LoadSubtitleInputs {
            inputs,
            font_root,
            db_path,
            avoid_system_fonts,
            mut current_session,
        } => {
            let loaded = load_subtitles(
                inputs,
                &font_root,
                &db_path,
                avoid_system_fonts,
                &mut current_session,
            )?;
            Ok(GuiEvent::FontsLoaded {
                view: loaded.view,
                session: current_session,
            })
        }
        GuiTask::UnloadFonts { mut session } => {
            let summary = session.unload_all().context("failed to unload fonts")?;
            Ok(GuiEvent::FontsUnloaded {
                session,
                unloaded_count: summary.unloaded.len(),
            })
        }
    }
}

fn ensure_index_on_startup(font_root: &Path, db_path: &Path) -> Result<FontIndexInspection> {
    let font_root = canonicalize_font_root(font_root)?;

    match inspect_index_status(&font_root, db_path)? {
        FontIndexInspection::Ready(summary) => Ok(FontIndexInspection::Ready(summary)),
        FontIndexInspection::RootMismatch { .. } | FontIndexInspection::MissingMetadata => {
            let mut index = FontIndex::open(db_path)?;
            index
                .rebuild_root(&font_root)
                .map(FontIndexInspection::Ready)
        }
        inspection @ FontIndexInspection::OutdatedSchema { .. } => Ok(inspection),
    }
}

fn update_index(font_root: &Path, db_path: &Path) -> Result<ScanSummary> {
    let mut index = FontIndex::open(db_path)?;
    let font_root = canonicalize_font_root(font_root)?;

    match index.active_font_root()? {
        Some(active_root) if paths_equal(&active_root, &font_root) => {
            index.update_bound_root(&font_root)
        }
        _ => index.rebuild_root(&font_root),
    }
}

struct SubtitleLoadResult {
    view: SubtitleLoadView,
}

fn load_subtitles(
    inputs: Vec<PathBuf>,
    font_root: &Path,
    db_path: &Path,
    avoid_system_fonts: bool,
    session: &mut FontSession,
) -> Result<SubtitleLoadResult> {
    ensure_index_can_load(font_root, db_path)?;

    let subtitles =
        discover::discover_subtitle_paths(&inputs).context("failed to discover subtitles")?;
    if subtitles.is_empty() {
        bail!("no supported subtitle files (.ass, .ssa) were found");
    }

    let font_report = subtitle::analyze_subtitle_font_report(&subtitles)?;
    let required_alias_count = font_report.required_fonts.len();
    let declared_but_unused_aliases = font_report
        .declared_but_unused_fonts
        .iter()
        .cloned()
        .collect();
    let mut system_aliases = Vec::new();
    let mut local_aliases = Vec::new();

    if avoid_system_fonts {
        let system_font_aliases = system_fonts::cached_system_font_aliases();
        for alias in &font_report.required_fonts {
            if system_font_aliases.contains(&normalize_font_name(alias)) {
                system_aliases.push(alias.clone());
            } else {
                local_aliases.push(alias.clone());
            }
        }
    } else {
        local_aliases.extend(font_report.required_fonts.iter().cloned());
    }

    let index = FontIndex::open(db_path)?;
    let resolve_report = index.resolve_required_fonts(&local_aliases)?;
    let load_summary = session
        .load_fonts(resolve_report.unique_font_paths.clone())
        .context("failed to load resolved local fonts")?;

    let view = SubtitleLoadView::from_resolve_report(
        subtitles.len(),
        required_alias_count,
        declared_but_unused_aliases,
        system_aliases,
        resolve_report,
        &load_summary,
    );

    Ok(SubtitleLoadResult { view })
}

fn ensure_index_can_load(font_root: &Path, db_path: &Path) -> Result<()> {
    match inspect_index_status(font_root, db_path)? {
        FontIndexInspection::Ready(_) => Ok(()),
        FontIndexInspection::RootMismatch {
            indexed_root,
            configured_root,
        } => bail!(
            "font index is bound to {}; build the index for {} before loading subtitles",
            indexed_root.display(),
            configured_root.display()
        ),
        FontIndexInspection::MissingMetadata => {
            bail!("font index is missing; build the index before loading subtitles")
        }
        FontIndexInspection::OutdatedSchema { schema_version } => bail!(
            "font index schema version {schema_version} is outdated; rebuild the index before loading subtitles"
        ),
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::{
        fs, thread,
        time::{Duration, SystemTime},
    };

    use super::*;

    #[test]
    fn inspect_missing_index_does_not_create_database() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let font_root = temp_dir.path().join("fonts");
        let db_path = temp_dir.path().join("font_index.redb");
        fs::create_dir(&font_root)?;

        let inspection = inspect_index_status(&font_root, &db_path)?;

        assert!(matches!(inspection, FontIndexInspection::MissingMetadata));
        assert!(!db_path.exists());
        Ok(())
    }

    #[test]
    fn startup_with_matching_index_does_not_modify_database() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let font_root = temp_dir.path().join("fonts");
        let db_path = temp_dir.path().join("font_index.redb");
        fs::create_dir(&font_root)?;

        {
            let mut index = FontIndex::open(&db_path)?;
            index.rebuild_root(&font_root)?;
        }

        let before_modified = modified_at(&db_path)?;
        thread::sleep(Duration::from_millis(20));

        let inspection = ensure_index_on_startup(&font_root, &db_path)?;

        assert!(matches!(inspection, FontIndexInspection::Ready(_)));
        assert_eq!(before_modified, modified_at(&db_path)?);
        Ok(())
    }

    fn modified_at(path: &Path) -> Result<SystemTime> {
        Ok(fs::metadata(path)?.modified()?)
    }
}
