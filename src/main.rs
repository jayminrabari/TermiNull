mod gui;
mod input;
mod layout;
mod pty;
mod renderer;

use anyhow::{Context, Result};
use std::{env, fs, path::PathBuf};

/// libxkbcommon (used by winit) only parses UTF-8 Compose tables. Legacy locales such as
/// `en_US` resolve to `iso8859-1/Compose`, which contains octal escapes that are not valid UTF-8,
/// producing errors like: `iso8859-1/Compose:39:34: string literal is not a valid UTF-8 string`.
fn fix_locale_for_xkbcommon() {
    if let Ok(lc_all) = env::var("LC_ALL") {
        if !lc_all.is_empty() && !locale_is_utf8_or_c(&lc_all) {
            env::remove_var("LC_ALL");
        }
    }
    let ctype = env::var("LC_CTYPE").ok().filter(|s| !s.is_empty());
    let lang = env::var("LANG").ok().filter(|s| !s.is_empty());
    let effective = ctype.as_deref().or(lang.as_deref()).unwrap_or("C");
    if !locale_is_utf8_or_c(effective) {
        env::set_var("LC_CTYPE", "C.UTF-8");
    }
}

fn locale_is_utf8_or_c(s: &str) -> bool {
    let lower = s.trim().to_ascii_lowercase();
    matches!(lower.as_str(), "c" | "posix")
        || lower.contains("utf-8")
        || lower.ends_with(".utf8")
}

fn main() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let mode = args.first().cloned().unwrap_or_default();
    if !matches!(mode.as_str(), "run" | "gui") {
        eprintln!("usage: terminull [run|gui] [2|2x2|3x4|4x4|--config config.json]");
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

    let ui = config.as_ref().map(|c| c.ui.clone()).unwrap_or_default();
    let scrollback_lines = ui.scrollback_lines.unwrap_or(4000);
    let panes = config.map(|c| c.panes).unwrap_or_default();
    let app = pty::App::new_with_scrollback(spec, shell, panes, scrollback_lines)?;
    if mode == "gui" {
        fix_locale_for_xkbcommon();
        gui::run(app, ui)
    } else {
        input::run(app)
    }
}
