#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ $# -ne 2 || $2 != "--no-network" ]]; then
  echo "Usage: $0 tests/fixtures/diagnostics/export-policy.json --no-network" >&2
  exit 2
fi

policy=$1
expected=tests/fixtures/diagnostics/expected-manifest.json
[[ -f "$policy" && -f "$expected" ]] || {
  echo "diagnostic export fixtures are missing" >&2
  exit 1
}

if rg -n \
  'std::net|tokio::net|TcpStream|UdpSocket|reqwest|hyper::|Command::new|std::process::Command' \
  crates/termirust-diagnostics/src; then
  echo "diagnostics production source contains a network or child-process route" >&2
  exit 1
fi

work=$(mktemp -d /tmp/termirust-diagnostic-bundle.XXXXXX)
trap 'find "$work" -depth -delete' EXIT
bundle="$work/bundle.json"
cargo run -q -p termirust-diagnostics --bin diagnostic_fixture_export --locked -- \
  "$policy" "$bundle"

python3 - "$policy" "$expected" "$bundle" <<'PY'
import json
from pathlib import Path
import stat
import sys

policy = json.loads(Path(sys.argv[1]).read_text())
expected = json.loads(Path(sys.argv[2]).read_text())
bundle_path = Path(sys.argv[3])
bundle = json.loads(bundle_path.read_text())
manifest = bundle["manifest"]

if set(manifest) != set(expected["required_manifest_fields"]):
    raise SystemExit("manifest fields do not exactly match the frozen contract")
if bundle["schema_version"] != expected["schema_version"]:
    raise SystemExit("bundle schema version mismatch")
if manifest["included_classes"] != expected["included_classes"]:
    raise SystemExit("included classes mismatch")
if manifest["excluded_classes"] != expected["excluded_classes"]:
    raise SystemExit("excluded classes mismatch")
if any(item["category"] != expected["file_category"] for item in manifest["files"]):
    raise SystemExit("unexpected diagnostic file category")
if manifest["total_entries"] != 1 or manifest["redactions"] != 0:
    raise SystemExit("unexpected synthetic manifest totals")
if len(manifest["snapshot_sha256"]) != 64:
    raise SystemExit("snapshot hash is not SHA-256")

raw = bundle_path.read_bytes()
for canary in policy["canaries"]:
    if canary.encode() in raw:
        raise SystemExit(f"privacy canary leaked: {canary}")
if stat.S_IMODE(bundle_path.stat().st_mode) != 0o600:
    raise SystemExit("exported bundle is not private mode 0600")
PY

cargo test -q -p termirust-diagnostics --test secret_canary_export --locked
echo "diagnostic bundle schema, zero-canary export, private mode, and no-network source verified"
