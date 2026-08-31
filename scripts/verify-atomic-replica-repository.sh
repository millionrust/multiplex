#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-replication-security --test secret_custody_contract --locked -- --test-threads=1
cargo test -p termirust-store --test replication_repository_contract --locked -- --test-threads=1
cargo clippy -p termirust-replication-security -p termirust-store --all-targets --all-features --locked -- -D warnings
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'AT-E12.6-01 OK: private repository round-trip, bounds, revisions, modes, and durability hold'
printf '%s\n' 'AT-E12.6-02 OK: corruption, newer data, symlinks, and interrupted activation preserve evidence'
printf '%s\n' 'AT-E12.6-03 OK: commit-before-retirement journal and locked/restart retries are idempotent'
printf '%s\n' 'AT-E12.6-04 OK: exact transport CAS, races, canonical hashes, and conflict evidence remain bounded'
