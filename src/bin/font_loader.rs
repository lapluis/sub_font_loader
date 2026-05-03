use sub_font_loader::{discover, input, session};

use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
};

use anyhow::{Context, Result};
use argh::FromArgs;
use session::FontSession;

/// temporarily load fonts from a directory or archive
#[derive(Debug, FromArgs)]
struct Cli {
    /// directory or archive to scan; defaults to the current directory
    #[argh(positional)]
    input: Option<PathBuf>,

    /// scan only the top level of the input directory or extracted archive
    #[argh(switch)]
    no_recursive: bool,

    /// unload immediately after loading instead of waiting for Enter or Ctrl+C
    #[argh(switch)]
    no_hold: bool,

    /// keep the temporary extraction directory when the input is an archive
    #[argh(switch)]
    keep_extracted: bool,
}

fn main() -> Result<()> {
    let cli: Cli = argh::from_env();
    run(cli)
}

fn run(cli: Cli) -> Result<()> {
    let shutdown = Shutdown::install()?;
    let input = cli.input.unwrap_or_else(|| PathBuf::from("."));
    let prepared = input::prepare_input(&input, cli.keep_extracted)
        .with_context(|| format!("failed to prepare input {}", input.display()))?;
    let discovered = discover::discover_fonts(prepared.scan_root(), !cli.no_recursive)
        .with_context(|| {
            format!(
                "failed to discover fonts in {}",
                prepared.scan_root().display()
            )
        })?;

    if discovered.is_empty() {
        anyhow::bail!(
            "no supported font files (.ttf, .otf, .ttc) found in {}",
            prepared.scan_root().display()
        );
    }

    let mut session = FontSession::new();
    let summary = session.load_fonts(discovered)?;
    println!(
        "Loaded {} font file{}.",
        summary.loaded.len(),
        if summary.loaded.len() == 1 { "" } else { "s" }
    );

    if !cli.no_hold && session.loaded_count() > 0 {
        shutdown.wait_for_enter_or_ctrl_c()?;
    }

    session.unload_all()?;

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ShutdownSignal {
    Enter,
    CtrlC,
}

struct Shutdown {
    tx: Sender<ShutdownSignal>,
    rx: Receiver<ShutdownSignal>,
}

impl Shutdown {
    fn install() -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let ctrlc_tx = tx.clone();

        ctrlc::set_handler(move || {
            let _ = ctrlc_tx.send(ShutdownSignal::CtrlC);
        })
        .context("failed to install Ctrl+C handler")?;

        Ok(Self { tx, rx })
    }

    fn wait_for_enter_or_ctrl_c(&self) -> Result<ShutdownSignal> {
        match self.rx.try_recv() {
            Ok(signal) => return Ok(signal),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                anyhow::bail!("shutdown signal channel disconnected");
            }
        }

        let enter_tx = self.tx.clone();
        thread::spawn(move || {
            let mut line = String::new();
            let _ = io::stdin().read_line(&mut line);
            let _ = enter_tx.send(ShutdownSignal::Enter);
        });

        println!(
            "The fonts are now visible to other programs. Press Enter or Ctrl+C to unload and exit."
        );

        self.rx.recv().context("failed to wait for Enter or Ctrl+C")
    }
}
