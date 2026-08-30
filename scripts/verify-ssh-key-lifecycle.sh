#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

command -v ssh-keygen >/dev/null
command -v docker >/dev/null
docker info >/dev/null

cargo test -p termirust ssh_keys::tests --locked -- --test-threads=1
cargo test -p termirust generated_identity --locked -- --test-threads=1
cargo test -p termirust sftp::tests::pre_cancelled_generated_key_operation_is_bounded_and_does_not_connect --locked -- --exact --test-threads=1
cargo test -p termirust docker_generated_key --locked -- --test-threads=1
cargo test -p termirust sftp::tests::docker_concurrent_generated_key_deployments_preserve_both_keys --locked -- --exact --test-threads=1
cargo test -p termirust ui::app::tests::e2e_keychain_generate_flow_creates_encrypted_identity_and_review --locked -- --exact --test-threads=1
cargo test -p termirust ui::app::tests::e2e_keychain_ui_deploys_verifies_and_exactly_removes_generated_key --locked -- --exact --test-threads=1
cargo fmt --all -- --check
git diff --check

printf 'SSH key generation and deployment verification passed\n'
