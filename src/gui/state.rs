use std::path::PathBuf;

use crate::{font::index::ScanSummary, session::FontSession};

use super::{commands, config::GuiConfig, view_model::SubtitleLoadView};

#[derive(Debug)]
pub struct AppState {
    pub config: GuiConfig,
    pub config_path: PathBuf,
    pub exe_dir: PathBuf,
    pub font_root: PathBuf,
    pub db_path: PathBuf,
    pub index_status: IndexStatus,
    pub load_status: LoadStatus,
    pub last_subtitle_inputs: Vec<PathBuf>,
    pub last_view: Option<SubtitleLoadView>,
    pub font_session: Option<FontSession>,
    pub is_busy: bool,
}

impl AppState {
    pub fn new(config: GuiConfig, config_path: PathBuf, exe_dir: PathBuf) -> Self {
        let font_root = config.resolved_font_root(&exe_dir);
        let db_path = exe_dir.join(commands::INDEX_FILE_NAME);

        Self {
            config,
            config_path,
            exe_dir,
            font_root,
            db_path,
            index_status: IndexStatus::Unknown,
            load_status: LoadStatus::Idle,
            last_subtitle_inputs: Vec::new(),
            last_view: None,
            font_session: Some(FontSession::new()),
            is_busy: false,
        }
    }

    pub fn loaded_font_count(&self) -> usize {
        self.font_session
            .as_ref()
            .map(FontSession::loaded_count)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub enum IndexStatus {
    Unknown,
    DisabledByConfig,
    Missing,
    Building,
    Ready {
        font_root: PathBuf,
        scanned_files: usize,
        indexed_files: usize,
    },
    Updating,
    Failed(String),
}

impl IndexStatus {
    pub fn from_summary(summary: &ScanSummary) -> Self {
        Self::Ready {
            font_root: summary.root.clone(),
            scanned_files: summary.scanned_files,
            indexed_files: summary.indexed_files,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub fn status_text(&self) -> String {
        match self {
            Self::Unknown => "Index status: unknown".to_owned(),
            Self::DisabledByConfig => "Index status: startup indexing disabled".to_owned(),
            Self::Missing => "Index status: missing or bound to another font directory".to_owned(),
            Self::Building => "Index status: rebuilding".to_owned(),
            Self::Ready {
                font_root,
                scanned_files,
                indexed_files,
            } => format!(
                "Index ready: {} available file(s), {} indexed file(s) under {}",
                scanned_files,
                indexed_files,
                font_root.display()
            ),
            Self::Updating => "Index status: updating".to_owned(),
            Self::Failed(error) => format!("Index failed: {error}"),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum LoadStatus {
    Idle,
    AnalyzingSubtitles,
    ResolvingFonts,
    LoadingFonts,
    Loaded(SubtitleLoadView),
    Failed(String),
}

impl LoadStatus {
    pub fn status_text(&self) -> String {
        match self {
            Self::Idle => "Load status: idle".to_owned(),
            Self::AnalyzingSubtitles => "Load status: analyzing subtitles".to_owned(),
            Self::ResolvingFonts => "Load status: resolving fonts".to_owned(),
            Self::LoadingFonts => "Load status: loading fonts".to_owned(),
            Self::Loaded(view) => format!(
                "Load complete: {} local, {} system skipped, {} missing, {} declared unused",
                view.loaded_local_font_count,
                view.skipped_system_alias_count,
                view.missing_alias_count,
                view.declared_but_unused_alias_count
            ),
            Self::Failed(error) => format!("Load failed: {error}"),
        }
    }
}
