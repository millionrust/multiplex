#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo test -p termirust-tui attach --locked
cargo test -p termirust-tui input --locked
cargo test -p termirust-tui --test pty_attach --locked -- --test-threads=1
cargo test -p termirust-tui --test lease_race --locked -- --test-threads=1
cargo test -p termirust-tui --test terminal_restore --locked -- --test-threads=1
cargo clippy -p termirust-tui -p termirust-client --all-targets --locked -- -D warnings
./scripts/verify-controller-security-vectors.sh --check

if cargo tree -p termirust-controller-security --edges normal --prefix none \
  | rg -q '^(termirust-tui|termirust-session-host|vt100) '; then
  printf 'controller security acquired a forbidden TUI or terminal dependency\n' >&2
  exit 1
fi

cargo fmt --all -- --check
git diff --check
printf 'bounded interactive TUI verification passed\n'
