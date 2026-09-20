#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p multiplex local_controller_conformance --locked
cargo test -p multiplex-tui --test local_controller_conformance --locked -- --test-threads=1
cargo clippy -p multiplex-tui -p multiplex-cli -p multiplex-domain -p multiplex-store --all-targets --locked -- -D warnings
python3 scripts/dev/clippy-changed.py
git diff --check

printf '%s\n' 'Local desktop, CLI, and TUI Session mutation conformance verified'
