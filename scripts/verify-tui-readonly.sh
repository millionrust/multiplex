#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo test -p termirust-store fleet --locked -- --test-threads=1
cargo test -p termirust-tui --all-targets --locked -- --test-threads=1
cargo clippy -p termirust-tui --all-targets --locked -- -D warnings

DEPENDENCIES="$(cargo tree -p termirust-tui --edges normal --prefix none)"
if printf '%s\n' "$DEPENDENCIES" | rg -q '^(termirust-client|termirust-session-host|termirust-host-protocol|russh|portable-pty) '; then
  printf 'read-only TUI acquired a forbidden runtime or mutation-capable dependency\n' >&2
  exit 1
fi

cargo fmt --all -- --check
git diff --check
printf 'bounded read-only TUI verification passed\n'
