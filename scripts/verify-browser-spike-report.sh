#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: ./scripts/verify-browser-spike-report.sh <report.json> <decision.md>" >&2
  exit 2
fi

report=$1
decision=$2
python3 - "$report" "$decision" <<'PY'
import json
from pathlib import Path
import sys

report_path = Path(sys.argv[1])
decision_path = Path(sys.argv[2])
if not report_path.is_file() or report_path.stat().st_size > 256 * 1024:
    raise SystemExit("report must be a regular JSON file no larger than 256 KiB")
if not decision_path.is_file():
    raise SystemExit("decision record is missing")

data = json.loads(report_path.read_text())
decision = decision_path.read_text()

required_fixtures = {
    "redirect",
    "rebinding",
    "iframe",
    "popup",
    "websocket",
    "service_worker",
    "download",
    "huge_dom",
    "stalled_response",
    "crash",
    "stale_element",
}
required_gates = {
    "os_user_profile_isolation",
    "owned_process_termination",
    "navigation_interception",
    "subresource_interception",
    "redirect_interception",
    "iframe_interception",
    "popup_interception",
    "websocket_interception",
    "service_worker_interception",
    "download_interception",
    "stale_document_detection",
    "cancellation_within_30s",
    "compatible_license",
    "maintained_release_and_security_route",
    "reproducible_packaging_path",
}
expected_candidates = {
    "chromiumoxide": ("0.9.1", "a7e2bb835b9643410f9e3dc044f0d947e96cbfa4"),
    "headless_chrome": ("1.0.22", "0a5c307a85debc450378a1f19e4dac1838d7b22d"),
    "webdriver_chromedriver": ("WebDriver 2 July 2026 WD + ChromeDriver 152.0.7977.64", None),
}

if data.get("schema_version") != 1 or data.get("mode") != "fixture_only":
    raise SystemExit("unexpected report schema or mode")
if data.get("fixture_seed") != 0x19012026 or not (10 <= data.get("runs", 0) <= 100):
    raise SystemExit("fixture seed or run count is not frozen")

harness = data.get("harness", {})
for key in (
    "loopback_only",
    "empty_child_environment",
    "owned_process_group_verified",
    "unrelated_process_survived",
    "descendant_terminated",
    "temporary_profile_removed",
):
    if harness.get(key) is not True:
        raise SystemExit(f"harness safety proof failed: {key}")
results = harness.get("fixture_results", [])
if {result.get("id") for result in results} != required_fixtures:
    raise SystemExit("hostile fixture result set is incomplete")
if not all(result.get("passed") is True for result in results):
    raise SystemExit("a hostile fixture harness probe failed")
if len(harness.get("warm_run_ms", [])) != data["runs"]:
    raise SystemExit("warm-run evidence count does not match requested runs")

candidates = {candidate.get("id"): candidate for candidate in data.get("candidates", [])}
if set(candidates) != set(expected_candidates):
    raise SystemExit("candidate set is not the fixed three-route comparison")
all_gates_pass = {}
for candidate_id, (version, commit) in expected_candidates.items():
    candidate = candidates[candidate_id]
    if candidate.get("controller_version") != version or candidate.get("controller_commit") != commit:
        raise SystemExit(f"candidate pin mismatch: {candidate_id}")
    browser = candidate.get("browser", {})
    if browser.get("version") != "152.0.7977.64":
        raise SystemExit(f"browser pin mismatch: {candidate_id}")
    if browser.get("archive_sha256") != "10033804338bd0a5aa098149a8dd64f3f2e0e8b201bf3d400d7c17d067ff696f":
        raise SystemExit(f"browser checksum mismatch: {candidate_id}")
    gates = candidate.get("mandatory_gates", {})
    if set(gates) != required_gates:
        raise SystemExit(f"mandatory gate set is incomplete: {candidate_id}")
    statuses = [gate.get("status") for gate in gates.values()]
    if any(status not in {"pass", "fail", "unknown"} for status in statuses):
        raise SystemExit(f"invalid gate status: {candidate_id}")
    all_gates_pass[candidate_id] = all(status == "pass" for status in statuses)

result = data.get("decision", {})
kind = result.get("kind")
selected = result.get("selected_candidate")
if kind == "go":
    if selected not in candidates or not all_gates_pass[selected]:
        raise SystemExit("Go is forbidden unless every mandatory candidate gate passes")
elif kind == "conditional_go":
    if selected not in candidates or not result.get("blockers"):
        raise SystemExit("Conditional Go requires a fixed candidate and blockers")
elif kind == "no_go":
    if selected is not None or not result.get("blockers"):
        raise SystemExit("No-Go requires no selected candidate and explicit reasons")
else:
    raise SystemExit("decision is not binary")

required_decision_text = (
    "Decision: **No-Go**",
    "chromiumoxide 0.9.1",
    "headless_chrome 1.0.22",
    "Chrome for Testing 152.0.7977.64",
    "Goal 19.2 remains frozen",
)
for marker in required_decision_text:
    if marker not in decision:
        raise SystemExit(f"decision record is missing: {marker}")

serialized = json.dumps(data, sort_keys=True)
for prohibited in ("/Users/", "/home/", "cookie", "password", "authorization"):
    if prohibited.lower() in serialized.lower():
        raise SystemExit(f"report contains prohibited user/credential material: {prohibited}")

print("browser spike report and No-Go decision are consistent")
PY
