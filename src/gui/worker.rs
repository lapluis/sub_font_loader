use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::{
    discover,
    font::index::{FontIndex, ScanSummary, canonicalize_font_root, normalize_font_name},
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
    RebuildIndex {
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
        unloaded_before_load: usize,
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
            | GuiTask::RebuildIndex { .. }
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

pub fn inspect_index_status(font_root: &Path, db_path: &Path) -> Result<Option<ScanSummary>> {
    if !db_path.exists() {
        return Ok(None);
    }

    let index = FontIndex::open(db_path)?;
    let Some(active_root) = index.active_font_root()? else {
        return Ok(None);
    };

    let font_root = canonicalize_font_root(font_root)?;
    if paths_equal(&active_root, &font_root) {
        Ok(Some(index.summary_for_root(&font_root)?))
    } else {
        Ok(None)
    }
}

fn run_task_inner(task: GuiTask) -> Result<GuiEvent> {
    match task {
        GuiTask::EnsureIndexOnStartup { font_root, db_path } => {
            let summary = ensure_index_on_startup(&font_root, &db_path)?;
            Ok(GuiEvent::IndexReady { summary })
        }
        GuiTask::RebuildIndex { font_root, db_path } => {
            let mut index = FontIndex::open(&db_path)?;
            let summary = index.rebuild_root(&font_root)?;
            Ok(GuiEvent::IndexReady { summary })
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
            db_path,
            avoid_system_fonts,
            mut current_session,
        } => {
            let loaded =
                load_subtitles(inputs, &db_path, avoid_system_fonts, &mut current_session)?;
            Ok(GuiEvent::FontsLoaded {
                view: loaded.view,
                session: current_session,
                unloaded_before_load: loaded.unloaded_before_load,
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

fn ensure_index_on_startup(font_root: &Path, db_path: &Path) -> Result<ScanSummary> {
    let db_exists = db_path.exists();
    let mut index = FontIndex::open(db_path)?;
    let font_root = canonicalize_font_root(font_root)?;

    if !db_exists {
        return index.rebuild_root(&font_root);
    }

    match index.active_font_root()? {
        Some(active_root) if paths_equal(&active_root, &font_root) => {
            index.update_bound_root(&font_root)
        }
        _ => index.rebuild_root(&font_root),
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
    unloaded_before_load: usize,
}

fn load_subtitles(
    inputs: Vec<PathBuf>,
    db_path: &Path,
    avoid_system_fonts: bool,
    session: &mut FontSession,
) -> Result<SubtitleLoadResult> {
    let subtitles =
        discover::discover_subtitle_paths(&inputs).context("failed to discover subtitles")?;
    if subtitles.is_empty() {
        bail!("no supported subtitle files (.ass, .ssa) were found");
    }

    let font_report = subtitle::analyze_subtitle_font_report(&subtitles)?;
    let required_alias_count = font_report.required_fonts.len();
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
    let unload_summary = session
        .unload_all()
        .context("failed to unload previously loaded fonts")?;
    let load_summary = session
        .load_fonts(resolve_report.unique_font_paths.clone())
        .context("failed to load resolved local fonts")?;

    let view = SubtitleLoadView::from_resolve_report(
        subtitles.len(),
        required_alias_count,
        system_aliases,
        resolve_report,
        &load_summary,
    );

    Ok(SubtitleLoadResult {
        view,
        unloaded_before_load: unload_summary.unloaded.len(),
    })
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
}
