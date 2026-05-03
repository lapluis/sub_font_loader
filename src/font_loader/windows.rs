use std::{os::windows::ffi::OsStrExt, path::Path};

use anyhow::{bail, Result};
use windows_sys::Win32::{
    Foundation::{LPARAM, WPARAM},
    Graphics::Gdi::{AddFontResourceW, RemoveFontResourceW},
    UI::WindowsAndMessaging::{SendMessageW, HWND_BROADCAST, WM_FONTCHANGE},
};

fn to_wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn add_font_resource(path: &Path) -> Result<i32> {
    let wide = to_wide_path(path);
    let count = unsafe { AddFontResourceW(wide.as_ptr()) };

    if count == 0 {
        bail!("failed to add font resource: {}", path.display());
    }

    Ok(count)
}

pub fn remove_font_resource(path: &Path) -> Result<()> {
    let wide = to_wide_path(path);
    let removed = unsafe { RemoveFontResourceW(wide.as_ptr()) };

    if removed == 0 {
        bail!("failed to remove font resource: {}", path.display());
    }

    Ok(())
}

pub fn broadcast_font_change() -> Result<()> {
    unsafe {
        SendMessageW(HWND_BROADCAST, WM_FONTCHANGE, 0 as WPARAM, 0 as LPARAM);
    }

    Ok(())
}
