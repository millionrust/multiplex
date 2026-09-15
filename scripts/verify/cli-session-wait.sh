#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-cli --test session_wait --locked -- --test-threads=1
cargo test -p termirust-cli --test json_v1_golden --locked -- --test-threads=1
cargo test -p termirust-cli --test exit_codes --locked -- --test-threads=1
cargo test -p termirust-cli --all-targets --locked -- --test-threads=1
cargo clippy -p termirust-cli -p termirust-domain -p termirust-store --all-targets --locked -- -D warnings
git diff --check
