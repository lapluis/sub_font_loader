use std::{
    ffi::OsStr,
    io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use walkdir::WalkDir;
use windows_sys::Win32::{
    Graphics::Gdi::{AddFontResourceW, RemoveFontResourceW},
    UI::WindowsAndMessaging::{SendMessageW, HWND_BROADCAST, WM_FONTCHANGE},
};

fn to_wide_null(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn broadcast_font_change() {
    unsafe {
        // wParam/lParam are unused, so 0 is fine.
        SendMessageW(HWND_BROADCAST, WM_FONTCHANGE, 0, 0);
    }
}

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

        let wide = to_wide_null(path.as_os_str());

        let count = unsafe { AddFontResourceW(wide.as_ptr()) };

        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("AddFontResourceW failed: {}", path.display()),
            ));
        }

        self.loaded.push(path.to_path_buf());
        Ok(count)
    }

    pub fn load_font_dir<P: AsRef<Path>>(&mut self, dir: P) -> io::Result<usize> {
        let mut loaded_files = 0usize;

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
                match self.load_font_file(path) {
                    Ok(_) => loaded_files += 1,
                    Err(err) => {
                        eprintln!("Failed to load: {}: {}", path.display(), err);
                    }
                }
            }
        }

        if loaded_files > 0 {
            broadcast_font_change();
        }

        Ok(loaded_files)
    }

    pub fn unload_all(&mut self) {
        for path in self.loaded.iter().rev() {
            let wide = to_wide_null(path.as_os_str());

            unsafe {
                RemoveFontResourceW(wide.as_ptr());
            }
        }

        self.loaded.clear();
        broadcast_font_change();
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
