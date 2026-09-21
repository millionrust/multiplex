#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

section() {
  printf '\n==> %s\n' "$1"
}

run() {
  printf '+ %s\n' "$*"
  "$@"
}

section "Rust toolchain"
run cargo --version
run rustc --version

section "Formatting"
run cargo fmt --check

section "GPUI dependency boundaries"
run python3 scripts/verify/gpui-boundaries.py

section "Read-only MCP boundary"
run ./scripts/verify/mcp-readonly.sh
run ./scripts/verify/mcp-actions.sh

section "Isolated browser capability"
run ./scripts/verify/browser-capability.sh

section "Compile"
run cargo check

section "Unit tests"
run cargo test

section "Clippy"
run cargo clippy --all-targets --all-features

section "Diff hygiene"
run git diff --check

# Resolved once, so the rest of the block reads a plain variable rather than repeating which
# spelling of the name it came from.
ssh_host="${MULTIPLEX_TEST_SSH_HOST:-${TERMIRUST_TEST_SSH_HOST:-}}"
ssh_user="${MULTIPLEX_TEST_SSH_USER:-${TERMIRUST_TEST_SSH_USER:-}}"
ssh_key="${MULTIPLEX_TEST_SSH_KEY:-${TERMIRUST_TEST_SSH_KEY:-}}"

if [[ -n "$ssh_host" ]]; then
  section "Optional live SSH smoke"

  user_arg=()
  if [[ -n "$ssh_user" ]]; then
    user_arg=("$ssh_user@")
  fi

  port="${MULTIPLEX_TEST_SSH_PORT:-${TERMIRUST_TEST_SSH_PORT:-22}}"
  identity_args=()
  if [[ -n "$ssh_key" ]]; then
    identity_args=(-i "$ssh_key")
  fi

  target="${user_arg[*]}$ssh_host"
  run ssh \
    -o BatchMode=yes \
    -o ConnectTimeout=8 \
    -o StrictHostKeyChecking=accept-new \
    -p "$port" \
    "${identity_args[@]}" \
    "$target" \
    "printf 'termirust-ssh-smoke-ok\n'; uname -a"
else
  section "Optional live SSH smoke skipped"
  printf '%s\n' "Set MULTIPLEX_TEST_SSH_HOST, MULTIPLEX_TEST_SSH_USER, MULTIPLEX_TEST_SSH_PORT, and optionally MULTIPLEX_TEST_SSH_KEY to test a real SSH target."
fi

section "Done"
