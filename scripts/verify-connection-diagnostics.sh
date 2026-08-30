#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

command -v docker >/dev/null 2>&1 || {
  echo 'Docker is required for the connection-diagnostics acceptance gate.' >&2
  exit 1
}
docker info >/dev/null 2>&1 || {
  echo 'Docker is installed but its engine is unavailable.' >&2
  exit 1
}

test -f docs/decisions/connection-diagnostics.md
rg -q 'MAX_ACTIVE_DIAGNOSTICS: usize = 4' src/connection_diagnostics.rs
rg -q 'MAX_QUEUED_DIAGNOSTICS: usize = 64' src/connection_diagnostics.rs
rg -q 'HostKeyPolicy::RequireExisting' src/ssh.rs
rg -q 'verify_existing' src/storage.rs src/ssh.rs
rg -q 'hosts-bulk-diagnose' src/ui/app/hosts.rs

cargo fmt --all -- --check
cargo test -p termirust connection_diagnostics::tests --locked -- --test-threads=1
cargo test -p termirust storage::tests::strict_known_host_verification_never_mutates_trust \
  --locked -- --exact --test-threads=1
cargo test -p termirust ssh::tests::connection_diagnostic_times_out_and_cancels_a_stalled_transport \
  --locked -- --exact --test-threads=1
cargo test -p termirust ssh::tests::docker_ssh_connection_diagnostic_is_strict_read_only_and_recovers \
  --locked -- --exact --test-threads=1
cargo test -p termirust \
  ui::app::tests::e2e_hosts_diagnose_button_reports_unknown_trust_without_opening_terminal \
  --locked -- --exact --test-threads=1
git diff --check

echo 'bounded read-only connection diagnostics verified'
