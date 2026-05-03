use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::session::{LoadSummary, UnloadSummary};

pub struct LoadReport<'a> {
    pub input: &'a Path,
    pub source: &'a str,
    pub scan_root: &'a Path,
    pub extracted_to: Option<&'a Path>,
    pub recursive: bool,
    pub discovered: &'a [PathBuf],
    pub load: &'a LoadSummary,
}

pub fn print_load_report(report: &LoadReport<'_>, json: bool) -> Result<()> {
    if json {
        println!("{}", to_json(report));
        return Ok(());
    }

    println!("Input: {}", report.input.display());
    println!("Source: {}", report.source);
    println!("Scan root: {}", report.scan_root.display());
    if let Some(extracted_to) = report.extracted_to {
        println!("Extracted to: {}", extracted_to.display());
    }
    println!("Recursive: {}", report.recursive);
    println!("Discovered: {} font file(s)", report.discovered.len());
    println!(
        "Loaded: {} font file(s) ({} font resource(s))",
        report.load.loaded.len(),
        loaded_resource_count(report.load)
    );
    println!("Failed: {} font file(s)", report.load.failed.len());

    if !report.load.failed.is_empty() {
        println!();
        println!("Failed fonts:");
        for failure in &report.load.failed {
            println!("  {}: {}", failure.path.display(), failure.error);
        }
    }

    Ok(())
}

pub fn print_unload_report(summary: &UnloadSummary, json: bool) -> Result<()> {
    if json {
        for failure in &summary.failed {
            eprintln!(
                "Failed to unload {}: {}",
                failure.path.display(),
                failure.error
            );
        }
        return Ok(());
    }

    println!("Unloaded: {} font file(s)", summary.unloaded.len());

    if !summary.failed.is_empty() {
        println!("Failed to unload: {} font file(s)", summary.failed.len());
        for failure in &summary.failed {
            println!("  {}: {}", failure.path.display(), failure.error);
        }
    }

    Ok(())
}

fn loaded_resource_count(summary: &LoadSummary) -> i32 {
    summary.loaded.iter().map(|font| font.resource_count).sum()
}

fn to_json(report: &LoadReport<'_>) -> String {
    let mut output = String::new();

    output.push_str("{\n");
    push_json_field(
        &mut output,
        "input",
        &report.input.display().to_string(),
        true,
    );
    push_json_field(&mut output, "source", report.source, true);
    push_json_field(
        &mut output,
        "scan_root",
        &report.scan_root.display().to_string(),
        true,
    );

    output.push_str("  \"extracted_to\": ");
    if let Some(extracted_to) = report.extracted_to {
        output.push_str(&json_string(&extracted_to.display().to_string()));
    } else {
        output.push_str("null");
    }
    output.push_str(",\n");

    output.push_str(&format!("  \"recursive\": {},\n", report.recursive));
    output.push_str(&format!(
        "  \"discovered_count\": {},\n",
        report.discovered.len()
    ));
    output.push_str(&format!(
        "  \"loaded_count\": {},\n",
        report.load.loaded.len()
    ));
    output.push_str(&format!(
        "  \"loaded_resource_count\": {},\n",
        loaded_resource_count(report.load)
    ));
    output.push_str(&format!(
        "  \"failed_count\": {},\n",
        report.load.failed.len()
    ));

    output.push_str("  \"discovered\": [");
    for (index, path) in report.discovered.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(&json_string(&path.display().to_string()));
    }
    output.push_str("],\n");

    output.push_str("  \"loaded\": [");
    for (index, font) in report.load.loaded.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str("{\"path\": ");
        output.push_str(&json_string(&font.path.display().to_string()));
        output.push_str(&format!(", \"resource_count\": {}", font.resource_count));
        output.push('}');
    }
    output.push_str("],\n");

    output.push_str("  \"failed\": [");
    for (index, failure) in report.load.failed.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str("{\"path\": ");
        output.push_str(&json_string(&failure.path.display().to_string()));
        output.push_str(", \"error\": ");
        output.push_str(&json_string(&failure.error));
        output.push('}');
    }
    output.push_str("]\n");
    output.push('}');

    output
}

fn push_json_field(output: &mut String, name: &str, value: &str, comma: bool) {
    output.push_str("  ");
    output.push_str(&json_string(name));
    output.push_str(": ");
    output.push_str(&json_string(value));
    if comma {
        output.push(',');
    }
    output.push('\n');
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");

    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }

    output.push('"');
    output
}
