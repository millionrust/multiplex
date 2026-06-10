#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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

section "Compile"
run cargo check

section "Unit tests"
run cargo test

section "Clippy"
run cargo clippy --all-targets --all-features

section "Diff hygiene"
run git diff --check

if [[ -n "${TSHELL_TEST_SSH_HOST:-}" ]]; then
  section "Optional live SSH smoke"

  user_arg=()
  if [[ -n "${TSHELL_TEST_SSH_USER:-}" ]]; then
    user_arg=("${TSHELL_TEST_SSH_USER}@")
  fi

  port="${TSHELL_TEST_SSH_PORT:-22}"
  identity_args=()
  if [[ -n "${TSHELL_TEST_SSH_KEY:-}" ]]; then
    identity_args=(-i "$TSHELL_TEST_SSH_KEY")
  fi

  target="${user_arg[*]}${TSHELL_TEST_SSH_HOST}"
  run ssh \
    -o BatchMode=yes \
    -o ConnectTimeout=8 \
    -o StrictHostKeyChecking=accept-new \
    -p "$port" \
    "${identity_args[@]}" \
    "$target" \
    "printf 'tshell-ssh-smoke-ok\n'; uname -a"
else
  section "Optional live SSH smoke skipped"
  printf '%s\n' "Set TSHELL_TEST_SSH_HOST, TSHELL_TEST_SSH_USER, TSHELL_TEST_SSH_PORT, and optionally TSHELL_TEST_SSH_KEY to test a real SSH target."
fi

section "Done"
