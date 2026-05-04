use std::{collections::BTreeSet, ptr::null_mut};

use anyhow::{Result, bail};
use argh::FromArgs;
use windows_sys::Win32::{
    Foundation::LPARAM,
    Graphics::Gdi::{
        DEFAULT_CHARSET, EnumFontFamiliesExW, GetDC, HDC, LOGFONTW, ReleaseDC, TEXTMETRICW,
    },
};

/// list font families currently visible through Windows GDI
#[derive(Debug, FromArgs)]
struct Cli {
    /// optional case-insensitive substring filter
    #[argh(positional)]
    filter: Option<String>,

    /// include GDI vertical font aliases whose names start with @
    #[argh(switch)]
    include_vertical: bool,
}

fn main() -> Result<()> {
    let cli: Cli = argh::from_env();
    let families = enumerate_font_families(cli.include_vertical)?;
    let filter = cli.filter.as_ref().map(|value| value.to_lowercase());
    let mut printed = 0usize;

    for family in &families {
        if let Some(filter) = &filter {
            if !family.to_lowercase().contains(filter) {
                continue;
            }
        }

        println!("{family}");
        printed += 1;
    }

    if filter.is_some() {
        eprintln!(
            "Printed {printed} matching font family/families out of {} visible family/families.",
            families.len()
        );
    } else {
        eprintln!("Printed {printed} visible font family/families.");
    }

    Ok(())
}

fn enumerate_font_families(include_vertical: bool) -> Result<BTreeSet<String>> {
    let _dc = ScreenDeviceContext::get()?;
    let mut query = LOGFONTW::default();
    query.lfCharSet = DEFAULT_CHARSET;

    let mut collector = FontFamilyCollector {
        families: BTreeSet::new(),
        include_vertical,
    };

    unsafe {
        EnumFontFamiliesExW(
            _dc.hdc,
            &query,
            Some(collect_font_family),
            &mut collector as *mut FontFamilyCollector as LPARAM,
            0,
        );
    }

    if collector.families.is_empty() {
        bail!("Windows did not return any visible font families");
    }

    Ok(collector.families)
}

struct ScreenDeviceContext {
    hdc: HDC,
}

impl ScreenDeviceContext {
    fn get() -> Result<Self> {
        let hdc = unsafe { GetDC(null_mut()) };

        if hdc.is_null() {
            bail!("failed to get the screen device context");
        }

        Ok(Self { hdc })
    }
}

impl Drop for ScreenDeviceContext {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(null_mut(), self.hdc);
        }
    }
}

struct FontFamilyCollector {
    families: BTreeSet<String>,
    include_vertical: bool,
}

unsafe extern "system" fn collect_font_family(
    log_font: *const LOGFONTW,
    _text_metric: *const TEXTMETRICW,
    _font_type: u32,
    lparam: LPARAM,
) -> i32 {
    if log_font.is_null() || lparam == 0 {
        return 1;
    }

    let collector = unsafe { &mut *(lparam as *mut FontFamilyCollector) };
    let face_name = wide_z_to_string(unsafe { &(*log_font).lfFaceName });

    if face_name.is_empty() {
        return 1;
    }

    if !collector.include_vertical && face_name.starts_with('@') {
        return 1;
    }

    collector.families.insert(face_name);
    1
}

fn wide_z_to_string(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());

    String::from_utf16_lossy(&value[..end])
}
