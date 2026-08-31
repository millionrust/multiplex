#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-store --test replication_repository_contract --locked -- --test-threads=1
cargo test -p termirust-store --test replication_sync_acceptance --locked -- --test-threads=1
cargo clippy -p termirust-store --all-targets --all-features --locked -- -D warnings
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'AT-E12.7-01 OK: review states, tokens, ordering, bounds, and stale rejection are deterministic'
printf '%s\n' 'AT-E12.7-02 OK: exact explicit resolutions dominate candidates and retry remains lossless'
printf '%s\n' 'AT-E12.7-03 OK: recovery preserves evidence and rejects unsafe, newer, invalid-policy, or denied state'
printf '%s\n' 'AT-E12.7-04 OK: two devices diverge, resolve, restart, converge byte-identically, and honor revocation'
