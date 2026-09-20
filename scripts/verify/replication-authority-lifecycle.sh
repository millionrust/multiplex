#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p multiplex-replication-security --test authority_lifecycle_contract --locked -- --test-threads=1
cargo test -p multiplex-replication-security --lib --locked -- authority::tests --test-threads=1
cargo test -p multiplex-domain --test replication_contract --locked -- --test-threads=1
cargo clippy -p multiplex-domain -p multiplex-replication-security --all-targets --locked -- -D warnings
python3 scripts/dev/clippy-changed.py
git diff --check

printf '%s\n' 'AT-E12.4-01 OK: exact bootstrap, enrollment, rotation, and revocation vectors hold'
printf '%s\n' 'AT-E12.4-02 OK: revoked recipients are excluded and causal cutoffs remain exact'
printf '%s\n' 'AT-E12.4-03 OK: races, duplicates, overflow, authority, and entropy failures publish nothing'
printf '%s\n' 'AT-E12.4-04 OK: device limits, deterministic order, redaction, and zeroization hold'
