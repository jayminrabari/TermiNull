# TermiNull (`terminull`)

Lightweight Rust terminal grid for low-end hardware.

## What it does

- Spawns one PTY per pane in parallel
- Supports preset layouts: `2`, `2x2`, `3x4`, `4x4`
- Uses `nix` forkpty and `vte`
- Runs either inside an existing terminal with `ratatui` or as its own native window with `winit` + `softbuffer`
- Routes keyboard input only to the active pane
- Shows editable pane titles
- Switches active pane with `Tab` and `Shift+Tab`
- Renames the active pane with `Ctrl-r`
- Closes active pane with `Ctrl-w`
- Restarts active pane with `Ctrl+Shift+r`
- Zooms font with `Ctrl+=`, `Ctrl+-`, and `Ctrl+0`
- Scrolls pane history with mouse wheel, `PageUp`, and `PageDown`
- Exits with `Ctrl-q`

## Build

This workspace has a local Rust toolchain installed under `.cargo` / `.rustup`.

```bash
CARGO_HOME="$PWD/.cargo" RUSTUP_HOME="$PWD/.rustup" ./.cargo/bin/cargo build --release
```

## Run

```bash
./target/release/terminull gui 2
./target/release/terminull gui 2x2
./target/release/terminull gui 3x4
./target/release/terminull gui 4x4
./target/release/terminull gui --config config.example.json

./target/release/terminull run 2
./target/release/terminull run 2x2
./target/release/terminull run 3x4
./target/release/terminull run 4x4
./target/release/terminull run --config config.example.json
```

## Config

```json
{
  "layout": "4x4",
  "shell": "bash",
  "ui": {
    "font": "Ubuntu Sans Mono",
    "font_size": 13,
    "scrollback_lines": 4000,
    "theme": {
      "background": "#101010",
      "pane_background": "#111111",
      "title_background": "#202020",
      "title_active_background": "#263b2b",
      "border": "#5a5a5a",
      "border_active": "#4ec36b",
      "text": "#e8e8e8",
      "dim_text": "#a8a8a8"
    }
  },
  "panes": [
    { "title": "Monitor", "command": "htop" },
    { "title": "Logs", "command": "tail -f logs" },
    { "title": "Frontend", "command": "npm start" },
    { "title": "Server", "command": "node server.js" }
  ]
}
```

Old string-only pane entries still work.

If there are fewer commands than panes, the remaining panes open the shell.

GUI redraw is output/event driven, so it does not continuously repaint while idle.

## Verification

```bash
CARGO_HOME="$PWD/.cargo" RUSTUP_HOME="$PWD/.rustup" ./.cargo/bin/cargo check
CARGO_HOME="$PWD/.cargo" RUSTUP_HOME="$PWD/.rustup" ./.cargo/bin/cargo test
```

`run 2x2`, `run 4x4`, and `gui 2x2` were smoke tested on this machine. The GUI smoke test opened as a standalone desktop window and was stopped after 3 seconds with `timeout`.
