mod app;
mod archive;
mod cli;
mod discover;
mod font_loader;
mod input;
mod report;
mod session;

fn main() -> anyhow::Result<()> {
    let cli = argh::from_env();
    app::run(cli)
}
