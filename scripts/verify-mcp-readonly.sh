#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo fmt --all -- --check
cargo test -p termirust-mcp --all-targets --locked -- --test-threads=1
cargo clippy -p termirust-mcp --all-targets --locked -- -D warnings

if rg -n 'read_payload\(' crates/termirust-mcp/src; then
  echo "MCP must not expose artifact payload reads." >&2
  exit 1
fi

if ! rg -q 'CapabilitySet::default\(\)' crates/termirust-mcp/src/protocol.rs; then
  echo "MCP must retain an explicit default read-only capability set." >&2
  exit 1
fi

git diff --check
printf '%s\n' "PASS: bounded capability-scoped read-only MCP"
