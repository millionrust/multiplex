#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-tui resume --locked
cargo test -p termirust-tui --test session_resume --locked -- --test-threads=1
cargo test -p termirust-tui --test focus_input_separation --locked
cargo test -p termirust-cli --test session_resume --locked -- --test-threads=1
cargo clippy -p termirust-tui -p termirust-cli -p termirust-session-host -p termirust-domain -p termirust-store --all-targets --locked -- -D warnings
git diff --check

printf '%s\n' 'TUI exact Codex Session resume review, commit, races, privacy, and Host lifecycle verified'
