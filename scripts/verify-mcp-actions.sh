#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo fmt --all -- --check
cargo test -p termirust-mcp --all-targets --locked -- --test-threads=1
cargo test -p termirust-cli --test session_input --locked
cargo clippy -p termirust-mcp --all-targets --locked -- -D warnings
cargo clippy -p termirust-cli --lib --locked -- -D warnings

if rg -n 'std::process::Command|Command::new\(|read_payload\(' crates/termirust-mcp/src; then
  echo "MCP actions must use typed facades and must not spawn commands or read artifact payloads." >&2
  exit 1
fi

for marker in ActionPolicyStore command_id expected_revision writer_lease; do
  if ! rg -q "$marker" crates/termirust-mcp/src crates/termirust-cli/src; then
    echo "MCP action contract marker is missing: $marker" >&2
    exit 1
  fi
done

git diff --check
printf '%s\n' "PASS: scoped approved idempotent MCP actions"
