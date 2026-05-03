use std::path::PathBuf;

use argh::FromArgs;

/// temporarily load fonts from a directory or archive
#[derive(Debug, FromArgs)]
pub struct Cli {
    /// directory or archive to scan; defaults to the current directory
    #[argh(positional)]
    pub input: Option<PathBuf>,

    /// scan only the top level of the input directory or extracted archive
    #[argh(switch)]
    pub no_recursive: bool,

    /// unload immediately after loading instead of waiting for Enter or Ctrl+C
    #[argh(switch)]
    pub no_hold: bool,

    /// keep the temporary extraction directory when the input is an archive
    #[argh(switch)]
    pub keep_extracted: bool,

    /// print the load report as JSON
    #[argh(switch)]
    pub json: bool,
}
