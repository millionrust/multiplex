#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 docs/decisions/update-trust.md tests/fixtures/update-tuf" >&2
  exit 2
fi

adr=$1
fixtures=$2
manifest="$fixtures/MANIFEST.sha256"

[[ -f "$adr" && -d "$fixtures" && -f "$manifest" ]] || {
  echo "update-trust ADR or fixture corpus is missing" >&2
  exit 1
}

for marker in \
  "Status: Accepted for offline metadata verification only" \
  "Decision date: 2026-08-28" \
  'Selected implementation: `tough 0.24.0`' \
  "35b378d98765c2ae9cdc3e9963ea7e670da8cdd9ee39611b8d722083c7f1ac11" \
  "98d8eb8b2ce63515d9b4981c938ef6453c5b5771" \
  "No platform updater is authorized by this ADR alone."; do
  rg -Fq -- "$marker" "$adr" || {
    echo "update-trust ADR is missing required marker: $marker" >&2
    exit 1
  }
done

rg -Fq 'tough = { version = "=0.24.0", default-features = false }' \
  crates/termirust-update-trust/Cargo.toml || {
  echo "tough dependency is not exactly pinned with default features disabled" >&2
  exit 1
}

python3 - <<'PY'
from pathlib import Path
import tomllib

lock = tomllib.loads(Path("Cargo.lock").read_text())
matches = [package for package in lock["package"] if package["name"] == "tough"]
if len(matches) != 1:
    raise SystemExit("locked graph does not contain exactly one tough package")
package = matches[0]
if package["version"] != "0.24.0" or package.get("checksum") != \
        "35b378d98765c2ae9cdc3e9963ea7e670da8cdd9ee39611b8d722083c7f1ac11":
    raise SystemExit("locked tough version or checksum does not match the ADR")
PY

if cargo tree -p termirust-update-trust -e features --locked | \
  rg -q 'tough feature "http"|reqwest'; then
  echo "tough HTTP dependencies entered the resolved verifier graph" >&2
  exit 1
fi

shasum -a 256 -c "$manifest"

for path in \
  "$fixtures/valid-v1/metadata/timestamp.json" \
  "$fixtures/valid-v2/metadata/2.targets.json" \
  "$fixtures/valid-delegated/metadata/1.stable-macos-aarch64.json" \
  "$fixtures/root-rotation/2.root.json" \
  "$fixtures/hostile/duplicate-signatures-root.json" \
  "$fixtures/source/SYNTHETIC-TEST-KEY.pem"; do
  [[ -f "$path" ]] || {
    echo "required update-trust fixture is missing: $path" >&2
    exit 1
  }
done

if rg -n -i \
  'reqwest|hyper::|TcpStream|UdpSocket|std::net|tokio::net|Command::new|std::process::Command|open::that' \
  crates/termirust-update-trust/src; then
  echo "update-trust production source contains a network, process, or installer surface" >&2
  exit 1
fi

cargo test -p termirust-update-trust --test tuf_attack_matrix --locked
cargo test -p termirust-update-trust --test root_rotation_and_atomic_state --locked

state_dir=$(mktemp -d /tmp/termirust-update-trust.XXXXXX)
trap 'find "$state_dir" -depth -delete' EXIT
cargo run -q -p termirust-update-trust --bin verify_update_repository --locked -- \
  "$fixtures/valid-v1" "$state_dir/state.json"
cargo run -q -p termirust-update-trust --bin verify_update_repository --locked -- \
  "$fixtures/valid-v2" "$state_dir/state.json"

python3 - "$state_dir/state.json" <<'PY'
import json
from pathlib import Path
import stat
import sys

path = Path(sys.argv[1])
state = json.loads(path.read_text())
expected = {
    "schema_version": 1,
    "root_version": 1,
    "timestamp_version": 2,
    "snapshot_version": 2,
    "targets_version": 2,
}
for key, value in expected.items():
    if state.get(key) != value:
        raise SystemExit(f"unexpected committed trust state: {key}")
if stat.S_IMODE(path.stat().st_mode) != 0o600:
    raise SystemExit("trust state is not private mode 0600")
PY

echo "update-trust ADR, fixtures, attack matrix, and atomic CLI state verified"
