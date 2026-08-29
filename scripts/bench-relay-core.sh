#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ ${1:-} != "--loopback-only" ]]; then
  echo "Refusing to run without --loopback-only." >&2
  exit 2
fi
shift

pairs=""
runs=""
output="target/relay-core/relay-core-report.json"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --pairs) pairs=${2:?missing pair counts}; shift 2 ;;
    --runs) runs=${2:?missing run count}; shift 2 ;;
    --output) output=${2:?missing output path}; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ $pairs != "1,10,100" || $runs != "10" ]]; then
  echo "Canonical evidence requires --pairs 1,10,100 --runs 10." >&2
  exit 2
fi

cargo run -q -p termirust-relay-server --example relay_bench -- \
  --pairs "$pairs" --runs "$runs" --output "$output"

python3 - "$output" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1]))
assert report["schema"] == "termirust-relay-core-benchmark"
assert report["loopback_only"] is True
assert report["runs_per_scenario"] == 10
assert [scenario["pairs"] for scenario in report["scenarios"]] == [1, 10, 100]
for scenario in report["scenarios"]:
    assert len(scenario["runs"]) == 10
    assert scenario["real_tcp_sockets"] == scenario["pairs"] * 2
    assert scenario["max_queue_drops"] == 0
    assert scenario["persistent_ciphertext_bytes"] == 0
    assert scenario["per_route_log_bytes"] == 0
    assert scenario["connect_p99_micros"] < 2_000_000
    assert scenario["round_trip_p99_micros"] < 500_000
PY

echo "relay core loopback benchmark passed: $output"
