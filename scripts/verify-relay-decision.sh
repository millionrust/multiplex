#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
report_dir=${1:?usage: verify-relay-decision.sh REPORT_DIR ADR}
adr=${2:?usage: verify-relay-decision.sh REPORT_DIR ADR}

python3 - "$report_dir" "$adr" <<'PY'
import json
from pathlib import Path
import re
import sys

report_dir = Path(sys.argv[1])
adr = Path(sys.argv[2])
report = json.loads((report_dir / "relay-spike-report.json").read_text())
cost = json.loads((report_dir / "cost-model.json").read_text())
threats = json.loads((report_dir / "threat-model.json").read_text())
decision = adr.read_text()

assert report["schema_version"] == 1
assert report["local_only"] is True
assert report["runs_per_scenario"] >= 10
assert report["machine"]["build_profile"] == "release"
expected = {(pairs, duty) for pairs in (1, 10, 100, 1000) for duty in ("idle", "interactive", "burst")}
actual = {(item["pairs"], item["duty_cycle"]) for item in report["scenarios"]}
assert actual == expected
for item in report["scenarios"]:
    assert len(item["runs"]) == report["runs_per_scenario"]
    assert item["logical_sockets"] == item["pairs"] * 2
    assert item["persistent_storage_bytes"] == 0
    assert item["per_route_log_bytes"] == 0
    assert item["max_queue_drops"] == 0
    assert item["connect_p50_micros"] <= item["connect_p95_micros"] <= item["connect_p99_micros"]
    assert item["forward_p50_micros"] <= item["forward_p95_micros"] <= item["forward_p99_micros"]

assert cost["schema_version"] == 1
assert re.fullmatch(r"\d{4}-\d{2}-\d{2}", cost["accessed"])
assert cost["termirust_license_usd"] == 0
assert len(cost["sources"]) >= 2
assert all(source.get("url", "").startswith("https://") for source in cost["sources"])
assert all(source.get("accessed") == cost["accessed"] for source in cost["sources"])
assert {row["pairs"] for row in cost["aws_websocket_examples"]} == {1, 10, 100, 1000}
assert cost["excluded_costs"]

assert threats["schema_version"] == 1
assert len(threats["threats"]) >= 15
required = {"id", "category", "adversary", "protected", "not_protected", "mitigation", "residual_metadata", "test", "owner", "release_blocker"}
assert all(required <= item.keys() for item in threats["threats"])
assert all(item["protected"] and item["not_protected"] and item["mitigation"] for item in threats["threats"])

required_sections = [
    "## Decision",
    "Conditional Go",
    "## Protocol",
    "## Data flow",
    "## Threat model",
    "## Residual metadata",
    "## Abuse and incident checklist",
    "## Traffic evidence",
    "## Cost model",
    "## Operations",
    "## D05 and D06 boundary",
    "## Sources",
]
for marker in required_sections:
    assert marker in decision, marker
assert "No public relay is authorized" in decision
assert "IP address, timing, and size" in decision
assert "2026-08-29" in decision
print("relay decision evidence is complete and local-only")
PY

if rg -n 'std::net|tokio::net|reqwest|hyper::Server' "$root/tools/relay-spike/src/lib.rs"; then
  echo "Relay spike core unexpectedly contains a network route." >&2
  exit 1
fi
if rg -n 'std::fs|File::|OpenOptions' "$root/tools/relay-spike/src/lib.rs"; then
  echo "Relay spike core unexpectedly persists content or admission state." >&2
  exit 1
fi
echo "relay static no-network/no-storage checks passed"
