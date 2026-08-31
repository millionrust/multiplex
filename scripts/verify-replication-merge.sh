#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-domain --test replication_contract --locked -- --test-threads=1
cargo clippy -p termirust-domain --all-targets --locked -- -D warnings
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'AT-E12.1-01 OK: bounded schema and active/revoked replica policy fail closed'
printf '%s\n' 'AT-E12.1-02 OK: causal maxima, tombstones, conflicts, and reviewed resolution hold'
printf '%s\n' 'AT-E12.1-03 OK: merge is commutative, associative, and idempotent'
printf '%s\n' 'AT-E12.1-04 OK: hostile limits, denial, audit privacy, and redaction pass'
