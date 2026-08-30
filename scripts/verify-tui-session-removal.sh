#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-cli --test management_facade removal --locked -- --test-threads=1
cargo test -p termirust-tui removal --locked
cargo test -p termirust-tui --test removal_lifecycle --locked -- --test-threads=1
cargo test -p termirust-tui --test focus_input_separation --locked
cargo clippy -p termirust-cli -p termirust-tui -p termirust-domain -p termirust-store --all-targets --locked -- -D warnings
git diff --check

printf '%s\n' 'TUI Session removal preview, confirmation, races, quarantine, and focus verified'
