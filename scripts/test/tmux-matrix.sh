#!/usr/bin/env bash
# Runs the tmux-behaviour tests against several tmux builds. tmux changes what it accepts and
# what it writes between releases: 3.2 has no copy-mode position counter and rewrites `:` and
# `.` in session names, so a setup that works on the newest tmux can still break the tmux an
# LTS ships. Pass the tmux binaries to test, or none to test the one on PATH.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

if [[ $# -eq 0 ]]; then
  command -v tmux >/dev/null 2>&1 || {
    echo "No tmux on PATH. Pass the tmux binaries to test." >&2
    exit 2
  }
  set -- "$(command -v tmux)"
fi

for binary in "$@"; do
  [[ -x $binary ]] || {
    echo "Not an executable tmux: $binary" >&2
    exit 2
  }
  version=$("$binary" -V)
  printf '[tmux-matrix] %s (%s)\n' "$version" "$binary"
  TERMIRUST_TMUX_PATH="$binary" cargo test --locked -p multiplex-tmux --tests
  TERMIRUST_TMUX_PATH="$binary" cargo test --locked \
    -p multiplex-controller-listener --test tmux_sessions
done

printf '[tmux-matrix] every tmux build passed\n'
