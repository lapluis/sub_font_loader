use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
};

use anyhow::{Context, Result};

use crate::{
    cli::Cli,
    discover, input,
    report::{self, LoadReport},
    session::FontSession,
};

pub fn run(cli: Cli) -> Result<()> {
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

    let mut session = FontSession::new();
    let load_summary = session.load_fonts(discovered.clone())?;

    report::print_load_report(
        &LoadReport {
            input: prepared.original_path(),
            source: prepared.source().as_str(),
            scan_root: prepared.scan_root(),
            extracted_to: prepared.extracted_to(),
            recursive: !cli.no_recursive,
            discovered: &discovered,
            load: &load_summary,
        },
        cli.json,
    )?;

    if !cli.no_hold && session.loaded_count() > 0 {
        shutdown.wait_for_enter_or_ctrl_c(cli.json)?;
    }

    let unload_summary = session.unload_all()?;
    report::print_unload_report(&unload_summary, cli.json)?;

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

    fn wait_for_enter_or_ctrl_c(&self, json: bool) -> Result<ShutdownSignal> {
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

        if json {
            eprintln!("Press Enter or Ctrl+C to unload and exit.");
        } else {
            println!(
                "The fonts are now visible to other programs. Press Enter or Ctrl+C to unload and exit."
            );
        }

        self.rx.recv().context("failed to wait for Enter or Ctrl+C")
    }
}
