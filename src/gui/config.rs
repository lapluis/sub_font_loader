use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiConfig {
    pub font_root: String,
    pub auto_index_on_startup: bool,
    pub auto_load_startup_subtitles: bool,
    pub avoid_system_fonts: bool,
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            font_root: String::new(),
            auto_index_on_startup: true,
            auto_load_startup_subtitles: true,
            avoid_system_fonts: true,
        }
    }
}

impl GuiConfig {
    pub fn load(config_path: &Path) -> Result<Self> {
        if !config_path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(config_path)
            .with_context(|| format!("failed to read config {}", config_path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse config {}", config_path.display()))
    }

    pub fn save(&self, config_path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self).context("failed to serialize GUI config")?;
        fs::write(config_path, content)
            .with_context(|| format!("failed to write config {}", config_path.display()))
    }

    pub fn resolved_font_root(&self, exe_dir: &Path) -> PathBuf {
        if self.font_root.trim().is_empty() {
            exe_dir.to_path_buf()
        } else {
            PathBuf::from(self.font_root.trim())
        }
    }
}
