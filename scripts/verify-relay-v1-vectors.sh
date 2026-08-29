#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ ${1:-} != "--check" || $# -ne 1 ]]; then
  echo "Usage: $0 --check" >&2
  exit 2
fi

cargo run -q -p termirust-relay-protocol --example relay_vectors -- \
  --check tests/fixtures/relay-v1/vectors.json

python3 - <<'PY'
import json
from pathlib import Path

vectors = json.loads(Path("tests/fixtures/relay-v1/vectors.json").read_text())
assert vectors["schema"] == "termirust-relay-v1-vectors"
assert vectors["schema_version"] == 1
assert vectors["protocol_version"] == 1
assert vectors["limits"]["ciphertext_payload_bytes"] == 1_048_576
assert vectors["limits"]["encoded_websocket_message_bytes"] == 1_048_640
assert vectors["limits"]["registered_routes"] == 1_000
assert vectors["limits"]["forwarding_pairs"] == 100
assert vectors["limits"]["queue_messages"] == 64
assert vectors["limits"]["queue_encoded_bytes"] == 4_194_304
codes = vectors["diagnostics"]
assert len(codes) == 35
assert [entry["number"] for entry in codes] == list(range(35))
assert len({entry["code"] for entry in codes}) == len(codes)
PY

echo "relay-v1 canonical vectors are current"
