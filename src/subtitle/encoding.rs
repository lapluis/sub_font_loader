use std::path::Path;

use anyhow::{Context, Result};
use encoding_rs::{GBK, UTF_16BE, UTF_16LE};

pub fn read_subtitle_text(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;

    if bytes.starts_with(b"\xEF\xBB\xBF") {
        return String::from_utf8(bytes[3..].to_vec())
            .with_context(|| format!("failed to decode {} as UTF-8", path.display()));
    }

    if bytes.starts_with(b"\xFF\xFE") {
        let (text, had_errors) = UTF_16LE.decode_without_bom_handling(&bytes[2..]);
        if had_errors {
            anyhow::bail!("failed to decode {} as UTF-16 LE", path.display());
        }

        return Ok(text.into_owned());
    }

    if bytes.starts_with(b"\xFE\xFF") {
        let (text, had_errors) = UTF_16BE.decode_without_bom_handling(&bytes[2..]);
        if had_errors {
            anyhow::bail!("failed to decode {} as UTF-16 BE", path.display());
        }

        return Ok(text.into_owned());
    }

    if let Ok(text) = std::str::from_utf8(&bytes) {
        return Ok(text.to_owned());
    }

    let (text, _, _) = GBK.decode(&bytes);
    Ok(text.into_owned())
}
