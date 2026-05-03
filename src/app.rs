use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
};

use anyhow::{Context, Result};

use crate::{cli::Cli, discover, input, session::FontSession};

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
    session.load_fonts(discovered)?;

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
