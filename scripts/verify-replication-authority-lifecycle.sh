#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-replication-security --test authority_lifecycle_contract --locked -- --test-threads=1
cargo test -p termirust-replication-security --lib --locked -- authority::tests --test-threads=1
cargo test -p termirust-domain --test replication_contract --locked -- --test-threads=1
cargo clippy -p termirust-domain -p termirust-replication-security --all-targets --locked -- -D warnings
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'AT-E12.4-01 OK: exact bootstrap, enrollment, rotation, and revocation vectors hold'
printf '%s\n' 'AT-E12.4-02 OK: revoked recipients are excluded and causal cutoffs remain exact'
printf '%s\n' 'AT-E12.4-03 OK: races, duplicates, overflow, authority, and entropy failures publish nothing'
printf '%s\n' 'AT-E12.4-04 OK: device limits, deterministic order, redaction, and zeroization hold'
