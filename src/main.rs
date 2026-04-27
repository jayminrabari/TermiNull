mod input;
mod layout;
mod pty;
mod renderer;

use anyhow::{Context, Result};
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) != Some("run") {
        eprintln!("usage: gridterm run [2|2x2|3x4|4x4|--config config.json]");
        std::process::exit(2);
    }
    args.remove(0);

    let config = if args.first().map(String::as_str) == Some("--config") {
        let path = args.get(1).context("missing config path")?;
        let raw = fs::read_to_string(PathBuf::from(path))?;
        Some(layout::Config::from_json(&raw)?)
    } else {
        None
    };

    let layout_name = args.first().cloned().unwrap_or_else(|| "2x2".to_string());
    let spec = config
        .as_ref()
        .map(|c| c.layout.clone())
        .unwrap_or_else(|| layout::LayoutPreset::from_cli(&layout_name));

    let shell = config
        .as_ref()
        .map(|c| c.shell.clone())
        .unwrap_or_else(|| "bash".to_string());

    let commands = config.map(|c| c.panes).unwrap_or_default();
    let app = pty::App::new(spec, shell, commands)?;
    input::run(app)
}

