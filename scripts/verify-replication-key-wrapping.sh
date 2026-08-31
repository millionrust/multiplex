#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-replication-security --test key_wrapping_contract --locked -- --test-threads=1
cargo test -p termirust-replication-security --test sealing_contract --locked -- --test-threads=1
cargo clippy -p termirust-replication-security --all-targets --locked -- -D warnings
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'AT-E12.3-01 OK: exact authority-authenticated device package round-trips'
printf '%s\n' 'AT-E12.3-02 OK: context, key, header, epoch, and ciphertext substitution fails closed'
printf '%s\n' 'AT-E12.3-03 OK: malformed bounds, invalid keys, and entropy failure remain bounded'
printf '%s\n' 'AT-E12.3-04 OK: private and epoch material redact and zeroize'
