#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

run() {
  printf '\n==> %s\n' "$1"
  shift
  "$@"
}

if [[ "${1:-}" != "--automated" || $# -ne 1 ]]; then
  echo "Usage: $0 --automated" >&2
  exit 2
fi

run "Release workflow and package contract" ./scripts/verify-release-workflow.sh
run "Metadata and Host crash recovery" \
  ./scripts/test-metadata-host-recovery.sh --fixtures tests/fixtures/recovery --crash-matrix
run "Derived-index crash recovery" \
  ./scripts/test-index-repair.sh --fixtures tests/fixtures/health-index --crash-matrix
run "Relay atomic-state crash recovery" \
  cargo test -p termirust-relay-server --test crash_recovery --locked
run "Update trust attack and rollback matrix" \
  ./scripts/verify-update-trust-adr.sh docs/decisions/update-trust.md tests/fixtures/update-tuf
run "Host protocol bounded fuzz smoke" ./scripts/fuzz-host-protocol-smoke.sh
run "Session Host stress and throughput" ./scripts/stress-session-host.sh
run "Desktop terminal performance budgets" ./scripts/bench-desktop-terminal.sh
run "100 MiB terminal accessibility budget" \
  ./scripts/bench-terminal-accessibility.sh --bytes 104857600 --profile release
run "Relay 1/10/100-pair performance budgets" \
  ./scripts/bench-relay-core.sh --loopback-only --pairs 1,10,100 --runs 10
run "Isolated browser containment" ./scripts/verify-browser-capability.sh
run "Controller fixture integrity" ./scripts/verify-controller-security-vectors.sh --check
run "Diff hygiene" git diff --check

printf '\nPASS: automated launch qualification\n'
