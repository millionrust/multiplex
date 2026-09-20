#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p multiplex-cli --test management_facade removal --locked -- --test-threads=1
cargo test -p multiplex-tui removal --locked
cargo test -p multiplex-tui --test removal_lifecycle --locked -- --test-threads=1
cargo test -p multiplex-tui --test focus_input_separation --locked
cargo clippy -p multiplex-cli -p multiplex-tui -p multiplex-domain -p multiplex-store --all-targets --locked -- -D warnings
git diff --check

printf '%s\n' 'TUI Session removal preview, confirmation, races, quarantine, and focus verified'
