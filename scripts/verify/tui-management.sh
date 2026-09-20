#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

cargo test -p multiplex-tui management --locked
cargo test -p multiplex-tui --test lifecycle_commands --locked
cargo test -p multiplex-tui --test focus_input_separation --locked
cargo test -p multiplex-tui --test stop_sentinel --locked
cargo test -p multiplex-cli --test management_facade --locked -- --test-threads=1
cargo clippy -p multiplex-tui -p multiplex-domain -p multiplex-client --all-targets --locked -- -D warnings

git diff --check
echo "TUI management focus, lifecycle, replay, ownership, and sentinel behavior verified"
