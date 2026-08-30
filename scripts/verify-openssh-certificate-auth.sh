#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo test -p termirust ssh_auth --locked
cargo test -p termirust openssh_certificate_profiles_and_sessions_round_trip_without_downgrade --locked
cargo test -p termirust password_profile_discards_stale_certificate_fields --locked
cargo test -p termirust certificate_file --locked
cargo test -p termirust mobile_vault_export_rejects_certificate_hosts_without_downgrade --locked
cargo test -p termirust ssh::tests::docker_ssh_openssh_certificate_connects_and_streams_output --locked -- --exact
cargo test -p termirust ssh::tests::docker_ssh_openssh_certificate_rejects_untrusted_signer_without_fallback --locked -- --exact
cargo test -p termirust ssh::tests::docker_jump_host_accepts_openssh_certificates_on_both_hops --locked -- --exact
cargo test -p termirust sftp::tests::docker_sftp_lists_directory_with_openssh_certificate --locked -- --exact
cargo fmt --all -- --check
git diff --check

printf 'OpenSSH certificate authentication verification passed\n'
