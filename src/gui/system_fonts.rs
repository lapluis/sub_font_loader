use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use walkdir::WalkDir;

use crate::{
    discover,
    font::{analyze_font_file, index::normalize_font_name},
};

static SYSTEM_FONT_ALIASES: OnceLock<HashSet<String>> = OnceLock::new();

pub fn cached_system_font_aliases() -> &'static HashSet<String> {
    SYSTEM_FONT_ALIASES.get_or_init(build_system_font_aliases)
}

fn build_system_font_aliases() -> HashSet<String> {
    let mut aliases = HashSet::new();

    for root in system_font_roots() {
        if !root.is_dir() {
            continue;
        }

        scan_system_font_root(&root, &mut aliases);
    }

    aliases
}

fn system_font_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(windir) = env::var_os("WINDIR") {
        roots.push(PathBuf::from(windir).join("Fonts"));
    }

    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local_app_data).join("Microsoft\\Windows\\Fonts"));
    }

    roots
}

fn scan_system_font_root(root: &Path, aliases: &mut HashSet<String>) {
    for entry in WalkDir::new(root) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "Warning: failed to scan system font directory {}: {error}",
                    root.display()
                );
                continue;
            }
        };

        if !entry.file_type().is_file() || !discover::is_supported_font(entry.path()) {
            continue;
        }

        match analyze_font_file(entry.path()) {
            Ok(analysis) => {
                for face in analysis.faces {
                    for alias in face.aliases {
                        let normalized = normalize_font_name(&alias.value);
                        if !normalized.is_empty() {
                            aliases.insert(normalized);
                        }
                    }
                }
            }
            Err(error) => eprintln!(
                "Warning: failed to analyze system font {}: {error:#}",
                entry.path().display()
            ),
        }
    }
}
