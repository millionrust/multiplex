#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p multiplex-tui resume --locked
cargo test -p multiplex-tui --test session_resume --locked -- --test-threads=1
cargo test -p multiplex-tui --test focus_input_separation --locked
cargo test -p multiplex-cli --test session_resume --locked -- --test-threads=1
cargo clippy -p multiplex-tui -p multiplex-cli -p multiplex-session-host -p multiplex-domain -p multiplex-store --all-targets --locked -- -D warnings
git diff --check

printf '%s\n' 'TUI exact Codex Session resume review, commit, races, privacy, and Host lifecycle verified'
