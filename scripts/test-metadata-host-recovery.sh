#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--fixtures" || -z "${2:-}" || "${3:-}" != "--crash-matrix" || $# -ne 3 ]]; then
  echo "Usage: $0 --fixtures tests/fixtures/recovery --crash-matrix" >&2
  exit 64
fi

fixture_root="$2"
policy="$fixture_root/recovery-policy.json"
[[ -f "$policy" ]] || { echo "missing recovery policy fixture" >&2; exit 1; }

python3 - "$policy" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    policy = json.load(handle)

assert policy["version"] == 1
assert policy["recovery_kinds"] == [
    "restore_last_good_metadata",
    "reconcile_host_leases",
]
assert set(policy["results"]) == {
    "restored", "reconciled", "no_change", "ambiguous", "rolled_back", "recovery_required"
}
assert policy["metadata_files"] == ["projects.json", "sessions.json", "presets.json"]
assert "process_signal" in policy["forbidden_capabilities"]
assert "session_data_delete" in policy["forbidden_capabilities"]
PY

cargo test -p termirust-store --test metadata_restore_crash_matrix
cargo test -p termirust-session-host --test lease_reconciliation
cargo test -p termirust-session-host --test no_unproven_signal
cargo test -p termirust -- ui::recovery

if rg -n 'libc::kill|std::process|Command::new|process\.kill|factory.reset|remove_dir_all' \
  crates/termirust-store/src/recovery.rs \
  crates/termirust-client/src/host_recovery.rs; then
  echo "recovery implementation contains a forbidden destructive or process-control API" >&2
  exit 1
fi

for selector in \
  session-host-recovery \
  session-host-recovery-prepare \
  session-host-recovery-confirm \
  session-host-recovery-cancel; do
  rg -q "\"$selector\"" src/ui/app/session_sidebar.rs || {
    echo "missing guarded Host recovery UI selector: $selector" >&2
    exit 1
  }
done

echo "metadata and Host recovery acceptance passed"
