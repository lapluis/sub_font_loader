# Sub Font Loader

Sub Font Loader is a Windows-focused Rust toolkit for working with fonts used by
ASS/SSA subtitles. It can inspect subtitle files for required font families,
analyze font name aliases, build a local SQLite index of fonts, and temporarily
load font files so other Windows programs can see them.

## Features

- Discover supported font files: `.ttf`, `.otf`, and `.ttc`
- Discover supported subtitle files: `.ass` and `.ssa`
- Extract font bundles from `.zip`, `.7z`, and `.rar` archives
- Temporarily register fonts with Windows and unload them on exit
- Parse ASS/SSA styles and override tags such as `\fn` and `\r`
- Analyze font family, full-name, and PostScript-name aliases
- Store searchable font aliases in a SQLite index
- Export font alias data to CSV

## Requirements

- Windows
- Rust toolchain with Cargo

The font loading path uses Windows GDI APIs (`AddFontResourceW`,
`RemoveFontResourceW`, and `WM_FONTCHANGE`), so this project is currently aimed
at Windows builds.

## Build

```powershell
cargo build --release
```

The compiled binaries are written under `target\release\`.

## Binaries

### `font_loader`

Temporarily load fonts from a directory or archive. Loaded fonts stay visible to
other programs until you press Enter, press Ctrl+C, or the process exits.

```powershell
cargo run --bin font_loader -- <directory-or-archive>
```

Examples:

```powershell
cargo run --bin font_loader -- .\fonts
cargo run --bin font_loader -- .\release-bundle.zip
cargo run --bin font_loader -- --no-recursive .\fonts
cargo run --bin font_loader -- --no-hold .\fonts
cargo run --bin font_loader -- --keep-extracted .\bundle.7z
```

Options:

- `--no-recursive`: scan only the top level of the input directory or extracted archive
- `--no-hold`: unload immediately after loading instead of waiting
- `--keep-extracted`: keep the temporary extraction directory for archive input

### `subtitle_fonts`

Analyze ASS/SSA subtitle files and print the fonts they require without loading
anything.

```powershell
cargo run --bin subtitle_fonts -- <subtitle-file-or-directory>
```

Examples:

```powershell
cargo run --bin subtitle_fonts -- .\subs
cargo run --bin subtitle_fonts -- --no-recursive .\episode.ass
```

The report includes:

- Required fonts used by styles and inline tags
- Declared style fonts
- Inline fonts from override tags
- Missing styles referenced by dialogue lines

### `font_analysis`

Analyze font files and print or export the aliases found in their name tables.

```powershell
cargo run --bin font_analysis -- <font-file-or-directory>
```

Examples:

```powershell
cargo run --bin font_analysis -- .\fonts
cargo run --bin font_analysis -- --no-recursive .\fonts
cargo run --bin font_analysis -- -o aliases.csv .\fonts
```

### `font_index`

Build and query a SQLite font alias index. The default database path is
`font_index.sqlite`.

Scan a font directory:

```powershell
cargo run --bin font_index -- scan --db font_index.sqlite .\fonts
```

Query one font name or alias:

```powershell
cargo run --bin font_index -- query --db font_index.sqlite "Noto Sans CJK SC"
```

Resolve fonts required by subtitle files:

```powershell
cargo run --bin font_index -- resolve-subtitles --db font_index.sqlite .\subs
```

Export indexed aliases:

```powershell
cargo run --bin font_index -- export-csv --db font_index.sqlite aliases.csv
```

The index normalizes font names with Unicode NFKC normalization, whitespace
collapse, case folding, and leading `@` removal. Re-scans skip unchanged files
and mark previously indexed files unavailable when they disappear from the scan
root.

## Typical Workflow

1. Index your local font library:

   ```powershell
   cargo run --bin font_index -- scan D:\Fonts
   ```

2. Check which subtitle fonts are required and which are missing:

   ```powershell
   cargo run --bin font_index -- resolve-subtitles .\subs
   ```

3. Temporarily load a folder or release archive of fonts while muxing, previewing,
   or rendering:

   ```powershell
   cargo run --bin font_loader -- .\fonts
   ```

## Library Layout

- `archive`: safe extraction for ZIP, 7z, and RAR inputs
- `discover`: recursive and top-level font discovery
- `font`: font name-table analysis and SQLite indexing
- `font_loader`: Windows font registration wrappers
- `input`: directory/archive preparation
- `session`: load/unload lifecycle management
- `subtitle`: ASS/SSA parsing and font usage analysis

## Development

```powershell
cargo fmt
cargo test
```

The project uses bundled SQLite through `rusqlite`, so a separate SQLite
installation is not required.

## License

This project is licensed under the terms in [LICENSE](LICENSE).
