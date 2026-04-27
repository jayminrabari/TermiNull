# gridterm

Lightweight Rust terminal grid for low-end hardware.

## What it does

- Spawns one PTY per pane in parallel
- Supports preset layouts: `2`, `2x2`, `3x4`, `4x4`
- Uses `nix` forkpty and `vte`
- Renders in a TUI using `ratatui`
- Routes keyboard input only to the active pane
- Switches active pane with `Tab` and `Shift+Tab`
- Exits with `Ctrl-q`

## Build

This workspace has a local Rust toolchain installed under `.cargo` / `.rustup`.

```bash
CARGO_HOME="$PWD/.cargo" RUSTUP_HOME="$PWD/.rustup" ./.cargo/bin/cargo build --release
```

## Run

```bash
./target/release/gridterm run 2
./target/release/gridterm run 2x2
./target/release/gridterm run 3x4
./target/release/gridterm run 4x4
./target/release/gridterm run --config config.example.json
```

## Config

```json
{
  "layout": "4x4",
  "shell": "bash",
  "panes": [
    "htop",
    "tail -f logs",
    "npm start",
    "node server.js"
  ]
}
```

If there are fewer commands than panes, the remaining panes open the shell.

## Verification

```bash
CARGO_HOME="$PWD/.cargo" RUSTUP_HOME="$PWD/.rustup" ./.cargo/bin/cargo check
CARGO_HOME="$PWD/.cargo" RUSTUP_HOME="$PWD/.rustup" ./.cargo/bin/cargo test
```

`2x2` and `4x4` were smoke tested in a real TTY on this machine.
