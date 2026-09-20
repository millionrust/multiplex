#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p multiplex-cli --test session_wait --locked -- --test-threads=1
cargo test -p multiplex-cli --test json_v1_golden --locked -- --test-threads=1
cargo test -p multiplex-cli --test exit_codes --locked -- --test-threads=1
cargo test -p multiplex-cli --all-targets --locked -- --test-threads=1
cargo clippy -p multiplex-cli -p multiplex-domain -p multiplex-store --all-targets --locked -- -D warnings
git diff --check
