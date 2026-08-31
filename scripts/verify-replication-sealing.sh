#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-replication-security --test sealing_contract --locked -- --test-threads=1
cargo test -p termirust-domain --test replication_contract --locked -- --test-threads=1
cargo clippy -p termirust-domain -p termirust-replication-security --all-targets --locked -- -D warnings
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'AT-E12.2-01 OK: exact authenticated put/delete vectors round-trip within domain bounds'
printf '%s\n' 'AT-E12.2-02 OK: metadata, byte, key, epoch, and operation tampering fails closed'
printf '%s\n' 'AT-E12.2-03 OK: hostile limits, invalid contexts, and RNG failure remain bounded'
printf '%s\n' 'AT-E12.2-04 OK: key, plaintext, context, and envelope debug/zeroization contracts hold'
