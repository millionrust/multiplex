#!/bin/sh
set -eu

cargo fmt --all -- --check
cargo test -p termirust-replication-security --test secret_custody_contract --locked -- --test-threads=1
cargo test -p termirust-replication-security --test secret_custody_contract --locked --features os-keyring -- --test-threads=1
cargo test -p termirust-replication-security --test authority_lifecycle_contract --locked -- --test-threads=1
cargo clippy -p termirust-replication-security --all-targets --locked --features os-keyring -- -D warnings
python3 scripts/clippy-changed.py
git diff --check

printf '%s\n' 'AT-E12.5-01 OK: exact authority, device, and epoch custody fixture round-trips'
printf '%s\n' 'AT-E12.5-02 OK: malformed, mismatched, missing, locked, collision, and unavailable paths fail closed'
printf '%s\n' 'AT-E12.5-03 OK: exact historical lookup and deterministic retirement remain bounded and non-destructive'
printf '%s\n' 'AT-E12.5-04 OK: OS adapter compiles, secret buffers zeroize, and unsupported targets remain explicit'
