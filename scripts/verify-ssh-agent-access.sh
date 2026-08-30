#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" && "$(uname -s)" != "Linux" ]]; then
  printf 'SSH-agent verification requires a Unix platform\n' >&2
  exit 1
fi

command -v ssh-agent >/dev/null
command -v ssh-add >/dev/null
command -v ssh-keygen >/dev/null

cargo test -p termirust ssh_auth::tests --locked
cargo test -p termirust local_agent --locked
cargo test -p termirust identity_agent --locked
cargo test -p termirust mobile_vault_export_rejects_ssh_agent_hosts_without_downgrade --locked
cargo test -p termirust ssh::tests::docker_ssh_agent_authenticates_terminal_and_remote_exec --locked -- --exact
cargo test -p termirust ssh::tests::docker_ssh_agent_rejects_unavailable_empty_and_untrusted_agents_without_fallback --locked -- --exact
cargo test -p termirust ssh::tests::docker_jump_host_accepts_ssh_agent_authentication_on_both_hops --locked -- --exact
cargo test -p termirust ssh::tests::docker_ssh_agent_forwarding_requires_explicit_per_connection_approval --locked -- --exact
cargo test -p termirust sftp::tests::docker_sftp_lists_directory_with_ssh_agent_authentication --locked -- --exact
cargo test -p termirust ui::app::tests::e2e_choose_protocol_agent_forwarding_action_is_one_shot --locked -- --exact --test-threads=1
cargo fmt --all -- --check
git diff --check

printf 'SSH-agent authentication and forwarding verification passed\n'
