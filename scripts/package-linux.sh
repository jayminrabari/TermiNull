#!/usr/bin/env bash
# Build release binary and optional .deb (needs: cargo install cargo-deb).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ -x "$ROOT/.cargo/bin/cargo" ]]; then
  export CARGO_HOME="${CARGO_HOME:-$ROOT/.cargo}"
  export RUSTUP_HOME="${RUSTUP_HOME:-$ROOT/.rustup}"
  PATH="$ROOT/.cargo/bin:$PATH"
fi

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
ARCH="${TRIPLE%%-*}"
[[ "$ARCH" == x86_64 ]] || ARCH="${TRIPLE##*-}"

cargo build --release

mkdir -p target/dist
TAR="target/dist/terminull-${VERSION}-linux-${ARCH}.tar.gz"
tar -czvf "$TAR" -C target/release terminull
echo "Portable archive: $TAR (copy to another machine and run ./terminull from extracted dir, or put in PATH)"

if cargo deb --version >/dev/null 2>&1; then
  cargo deb
  echo ".deb packages are under target/debian/"
  ls -1 target/debian/*.deb 2>/dev/null || true
else
  echo "Skip .deb: install cargo-deb with  cargo install cargo-deb  then re-run this script."
fi
