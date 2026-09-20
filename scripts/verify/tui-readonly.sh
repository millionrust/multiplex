#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

cargo test -p multiplex-store fleet --locked -- --test-threads=1
cargo test -p multiplex-tui --all-targets --locked -- --test-threads=1
cargo clippy -p multiplex-tui --all-targets --locked -- -D warnings

DEPENDENCIES="$(cargo tree -p multiplex-tui --edges normal --prefix none)"
if printf '%s\n' "$DEPENDENCIES" | rg -q '^(multiplex-client|multiplex-session-host|multiplex-host-protocol|russh|portable-pty) '; then
  printf 'read-only TUI acquired a forbidden runtime or mutation-capable dependency\n' >&2
  exit 1
fi

cargo fmt --all -- --check
git diff --check
printf 'bounded read-only TUI verification passed\n'
