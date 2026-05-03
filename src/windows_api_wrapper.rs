use std::{os::windows::ffi::OsStrExt, path::Path};

use windows_sys::Win32::{
    Foundation::{LPARAM, WPARAM},
    Graphics::Gdi::{AddFontResourceW, RemoveFontResourceW},
    UI::WindowsAndMessaging::{HWND_BROADCAST, SendMessageW, WM_FONTCHANGE},
};

pub fn broadcast_font_change() {
    unsafe {
        SendMessageW(HWND_BROADCAST, WM_FONTCHANGE, 0 as WPARAM, 0 as LPARAM);
    }
}

fn to_wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn add_font_resource(path: &Path) -> Result<i32, String> {
    let wide = to_wide_path(path);
    let count = unsafe { AddFontResourceW(wide.as_ptr()) };
    if count != 0 {
        Ok(count)
    } else {
        Err(format!("Failed to add font resource: {}", path.display()))
    }
}

pub fn remove_font_resource(path: &Path) -> Result<(), String> {
    let wide = to_wide_path(path);
    let result = unsafe { RemoveFontResourceW(wide.as_ptr()) };
    if result != 0 {
        Ok(())
    } else {
        Err(format!(
            "Failed to remove font resource: {}",
            path.display()
        ))
    }
}

pub fn add_font_resources(paths: &[&Path]) -> Result<usize, String> {
    let mut loaded = 0;

    for path in paths {
        if add_font_resource(path).is_ok() {
            loaded += 1;
        }
    }

    if loaded == 0 {
        return Err(format!(
            "Failed to add any font resources from {} path(s)",
            paths.len()
        ));
    }

    broadcast_font_change();

    Ok(loaded)
}

pub fn remove_font_resources(paths: &[&Path]) -> Result<usize, String> {
    let mut removed = 0;

    for path in paths {
        if remove_font_resource(path).is_ok() {
            removed += 1;
        }
    }

    if removed == 0 {
        return Err(format!(
            "Failed to remove any font resources from {} path(s)",
            paths.len()
        ));
    }

    broadcast_font_change();

    Ok(removed)
}
