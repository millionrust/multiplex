#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Required tool '$1' is missing. $2" >&2
    exit 127
  fi
}

require_tool cargo "Install Rust with rustup; rust-toolchain.toml pins the required toolchain."
require_tool rustc "Install Rust with rustup; rust-toolchain.toml pins the required toolchain."
require_tool python3 "Install Python 3 to run the changed-line Clippy policy."

verify_versions() {
  local rust_version deny_version
  rust_version=$(rustc --version | awk '{print $2}')
  if [[ $rust_version != "1.98.1" ]]; then
    echo "Expected rustc 1.98.1 from rust-toolchain.toml; found $rust_version." >&2
    exit 1
  fi
  require_tool cargo-deny "Install the pinned policy runner with: cargo install cargo-deny --version 0.19.8 --locked"
  deny_version=$(cargo deny --version | awk '{print $2}')
  if [[ $deny_version != "0.19.8" ]]; then
    echo "Expected cargo-deny 0.19.8; found $deny_version." >&2
    exit 1
  fi
}

focused() {
  cargo fmt --all -- --check
  python3 scripts/verify/gpui-boundaries.py
  ./scripts/verify/mcp-readonly.sh
  ./scripts/verify/mcp-actions.sh
  ./scripts/verify/browser-capability.sh
  cargo check -p multiplex --all-targets --all-features --locked
  python3 scripts/dev/clippy-changed.py
  cargo test -p multiplex local::tests::local_tmux_session_survives_disconnect_and_reattaches -- --exact --nocapture
  cargo test -p multiplex ui::app::tests::canvas_persistent_local_terminal_opens_or_explains_missing_tmux -- --exact --nocapture
}

policy() {
  verify_versions
  python3 - <<'PY'
from datetime import date
import json
from pathlib import Path
import re
import subprocess
import sys

metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
    text=True,
))
versions = {package["name"]: package.get("rust_version") for package in metadata["packages"]}
# Every package declares the pinned toolchain; there is no separate minimum.
unexpected = {name: version for name, version in versions.items() if version != "1.98"}
if unexpected:
    print(f"Workspace packages must declare rust-version 1.98, the pinned toolchain: {unexpected}", file=sys.stderr)
    raise SystemExit(1)

policy = Path("deny.toml").read_text()
ignore_block = policy.split("ignore = [", 1)[1].split("\n]", 1)[0]
exceptions = [line for line in ignore_block.splitlines() if "{ " in line]
for exception in exceptions:
    if "Owner:" not in exception:
        print(f"Advisory exception has no owner: {exception.strip()}", file=sys.stderr)
        raise SystemExit(1)
    match = re.search(r"expires: (\d{4}-\d{2}-\d{2})", exception)
    if match is None:
        print(f"Advisory exception has no expiry: {exception.strip()}", file=sys.stderr)
        raise SystemExit(1)
    if date.fromisoformat(match.group(1)) < date.today():
        print(f"Advisory exception expired: {exception.strip()}", file=sys.stderr)
        raise SystemExit(1)
PY
  cargo deny check
}

workspace() {
  cargo fmt --all -- --check
  python3 scripts/verify/gpui-boundaries.py
  ./scripts/verify/mcp-readonly.sh
  ./scripts/verify/mcp-actions.sh
  ./scripts/verify/browser-capability.sh
  cargo check --workspace --all-targets --all-features --locked
  cargo clippy --workspace --all-targets --all-features
  python3 scripts/dev/clippy-changed.py
  # Runs every test binary even after one fails, so a red run reports all of its failures
  # instead of the first one and hides the rest behind it.
  cargo test --workspace --all-targets --locked --no-fail-fast
  cargo doc --workspace --no-deps
  policy
  git diff --check
}

case "${1:-}" in
  focused) focused ;;
  workspace) workspace ;;
  policy) policy ;;
  *)
    echo "Usage: $0 {focused|workspace|policy}" >&2
    exit 2
    ;;
esac
