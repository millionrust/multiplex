#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

fixtures=""
crash_matrix=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --fixtures)
      [[ $# -ge 2 ]] || { echo "--fixtures requires a path" >&2; exit 2; }
      fixtures=$2
      shift 2
      ;;
    --crash-matrix)
      crash_matrix=true
      shift
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

[[ -n "$fixtures" ]] || { echo "--fixtures is required" >&2; exit 2; }
[[ -f "$fixtures/repair-policy.json" ]] || {
  echo "Missing repair policy: $fixtures/repair-policy.json" >&2
  exit 1
}
[[ "$crash_matrix" == true ]] || {
  echo "--crash-matrix is required for the release verifier" >&2
  exit 2
}

python3 - "$fixtures/repair-policy.json" <<'PY'
import json
import sys

policy = json.load(open(sys.argv[1], encoding="utf-8"))
assert policy["version"] == 1
assert policy["authoritative_files"] == [
    "format.json", "projects.json", "sessions.json", "presets.json"
]
assert policy["derived_indexes"] == [
    "project-session-v1.json", "palette-v1.json"
]
assert policy["maximum_sessions_per_project"] == 10_000
assert len(policy["crash_points"]) == 5
PY

if rg -n 'std::process|Command::new|TcpStream|UdpSocket|reqwest|russh|HostCommand' \
  crates/termirust-store/src/health.rs crates/termirust-domain/src/indexes.rs; then
  echo "Health/index implementation contains a forbidden network or process capability." >&2
  exit 1
fi

cargo test -p termirust-store --test health_scan
cargo test -p termirust-store --test derived_index_rebuild_crash_matrix
cargo test -p termirust-domain --test index_determinism

echo "Index repair verification passed."
