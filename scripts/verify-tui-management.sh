#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

cargo test -p termirust-tui management --locked
cargo test -p termirust-tui --test lifecycle_commands --locked
cargo test -p termirust-tui --test focus_input_separation --locked
cargo test -p termirust-tui --test stop_sentinel --locked
cargo test -p termirust-cli --test management_facade --locked -- --test-threads=1
cargo clippy -p termirust-tui -p termirust-domain -p termirust-client --all-targets --locked -- -D warnings

git diff --check
echo "TUI management focus, lifecycle, replay, ownership, and sentinel behavior verified"
