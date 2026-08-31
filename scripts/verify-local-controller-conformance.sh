#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust local_controller_conformance --locked
cargo test -p termirust-tui --test local_controller_conformance --locked -- --test-threads=1
cargo clippy -p termirust-tui -p termirust-cli -p termirust-domain -p termirust-store --all-targets --locked -- -D warnings
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'Local desktop, CLI, and TUI Session mutation conformance verified'
