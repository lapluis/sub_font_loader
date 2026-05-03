use std::{
    io,
    path::{Path, PathBuf},
};

use walkdir::WalkDir;

mod windows_api_wrapper;

#[derive(Default)]
pub struct SessionFontLoader {
    loaded: Vec<PathBuf>,
}

impl SessionFontLoader {
    pub fn new() -> Self {
        Self { loaded: Vec::new() }
    }

    pub fn load_font_file<P: AsRef<Path>>(&mut self, path: P) -> io::Result<i32> {
        let path = path.as_ref();

        let count = windows_api_wrapper::add_font_resource(path).map_err(io::Error::other)?;

        self.loaded.push(path.to_path_buf());
        Ok(count)
    }

    pub fn load_font_dir<P: AsRef<Path>>(&mut self, dir: P) -> io::Result<usize> {
        let mut font_files = Vec::new();

        for entry in WalkDir::new(dir) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();

            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                continue;
            };

            let ext = ext.to_ascii_lowercase();
            if matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                font_files.push(path.to_path_buf());
            }
        }

        if font_files.is_empty() {
            return Ok(0);
        }

        let paths = font_files.iter().map(PathBuf::as_path).collect::<Vec<_>>();
        let loaded_files =
            windows_api_wrapper::add_font_resources(&paths).map_err(io::Error::other)?;

        self.loaded.extend(font_files);
        Ok(loaded_files)
    }

    pub fn unload_all(&mut self) {
        let paths = self
            .loaded
            .iter()
            .rev()
            .map(PathBuf::as_path)
            .collect::<Vec<_>>();
        if !paths.is_empty() {
            if let Err(err) = windows_api_wrapper::remove_font_resources(&paths) {
                eprintln!("{}", err);
            }
        }

        self.loaded.clear();
    }
}

impl Drop for SessionFontLoader {
    fn drop(&mut self) {
        self.unload_all();
    }
}

fn main() -> io::Result<()> {
    let font_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut loader = SessionFontLoader::new();

    let count = loader.load_font_dir(&font_dir)?;
    println!("Temporarily loaded {} font files.", count);
    println!("The fonts are now visible to other programs. Press Enter to unload and exit.");

    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;

    // Drop will also unload automatically.
    loader.unload_all();

    Ok(())
}
