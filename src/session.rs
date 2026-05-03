use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::font_loader::windows;

#[derive(Debug, Clone)]
pub struct LoadedFont {
    pub path: PathBuf,
    pub resource_count: i32,
}

#[derive(Debug, Clone)]
pub struct FailedFont {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, Default)]
pub struct LoadSummary {
    pub loaded: Vec<LoadedFont>,
    pub failed: Vec<FailedFont>,
}

#[derive(Debug, Clone, Default)]
pub struct UnloadSummary {
    pub unloaded: Vec<LoadedFont>,
    pub failed: Vec<FailedFont>,
}

#[derive(Debug, Default)]
pub struct FontSession {
    loaded: Vec<LoadedFont>,
}

impl FontSession {
    pub fn new() -> Self {
        Self { loaded: Vec::new() }
    }

    pub fn load_fonts<I>(&mut self, paths: I) -> Result<LoadSummary>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let mut summary = LoadSummary::default();

        for path in paths {
            match windows::add_font_resource(&path) {
                Ok(resource_count) => {
                    let loaded = LoadedFont {
                        path,
                        resource_count,
                    };
                    self.loaded.push(loaded.clone());
                    summary.loaded.push(loaded);
                }
                Err(error) => summary.failed.push(FailedFont {
                    path,
                    error: format!("{error:#}"),
                }),
            }
        }

        if !summary.loaded.is_empty() {
            windows::broadcast_font_change()
                .context("failed to broadcast font-change notification after loading fonts")?;
        }

        Ok(summary)
    }

    pub fn unload_all(&mut self) -> Result<UnloadSummary> {
        let loaded = std::mem::take(&mut self.loaded);
        let mut summary = UnloadSummary::default();

        for font in loaded.into_iter().rev() {
            match windows::remove_font_resource(&font.path) {
                Ok(()) => summary.unloaded.push(font),
                Err(error) => summary.failed.push(FailedFont {
                    path: font.path,
                    error: format!("{error:#}"),
                }),
            }
        }

        if !summary.unloaded.is_empty() || !summary.failed.is_empty() {
            windows::broadcast_font_change()
                .context("failed to broadcast font-change notification after unloading fonts")?;
        }

        Ok(summary)
    }

    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }
}

impl Drop for FontSession {
    fn drop(&mut self) {
        match self.unload_all() {
            Ok(summary) => {
                for failure in summary.failed {
                    eprintln!(
                        "Failed to unload {}: {}",
                        failure.path.display(),
                        failure.error
                    );
                }
            }
            Err(error) => eprintln!("{error:#}"),
        }
    }
}
