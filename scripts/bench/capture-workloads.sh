#!/usr/bin/env bash
# Spike 0.2: what capture costs under the three workloads Remote Screens is sized for.
#
# RS2 measured an uncontrolled screen -- "this development session, which updates continuously" --
# which answered the architectural question (macOS attaches no dirty rectangles) but not the sizing
# one. The plan's targets are per workload, so the measurement has to be per workload too.
#
# Each workload is driven in this terminal, so it must be the visible, frontmost window, the screen
# must be awake and unlocked, and Screen Recording must be allowed for this terminal. Nothing is
# recorded to disk: capture_stats prints counts and rates and keeps no pixels.
#
#   scripts/bench/capture-workloads.sh [seconds] [scale] [video-file]
#
# `video-file` is optional. Without it the video row is a synthetic full-screen animation, which
# has the capture characteristic that matters (a large area changing every frame) but is not a real
# decoded video, and the output says so. With it, the file is opened in the default player.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

SECONDS_PER_RUN="${1:-10}"
SCALE="${2:-1.0}"
VIDEO_FILE="${3:-}"

export CARGO_INCREMENTAL=0

# Built once, up front: a build running during a capture is itself a workload, and would land in
# every number below.
echo "building capture_stats..."
cargo build -p multiplex-screen-capture --release --example capture_stats >/dev/null

STATS="$ROOT_DIR/target/release/examples/capture_stats"
[[ -x "$STATS" ]] || { echo "capture_stats was not built at $STATS" >&2; exit 1; }

workload_pid=""
cleanup() {
  [[ -n "$workload_pid" ]] && kill "$workload_pid" 2>/dev/null || true
  workload_pid=""
}
trap cleanup EXIT

# A person typing at a prompt: one character at a time, a newline now and then. The point is that
# each change is tiny, so this is where damage reporting would have paid off most.
typing_workload() {
  local text="the quick brown fox jumps over the lazy dog and keeps on typing "
  while true; do
    for (( i=0; i<${#text}; i++ )); do
      printf '%s' "${text:i:1}"
      sleep 0.08
    done
    printf '\n'
  done
}

# Build output, a log tail, a long file: full lines arriving fast enough that the whole window moves.
scrolling_workload() {
  local n=0
  while true; do
    printf '%6d  %s\n' "$n" "$(head -c 72 /dev/urandom | base64 | head -c 72)"
    n=$((n + 1))
    sleep 0.02
  done
}

# A large area changing every frame. Synthetic: 24-bit colour bands that shift each frame, redrawn
# over the whole terminal. Not a decoded video, and the report says so.
synthetic_video_workload() {
  local rows cols
  rows=$(tput lines 2>/dev/null || echo 40)
  cols=$(tput cols 2>/dev/null || echo 120)
  local phase=0
  while true; do
    printf '\033[H'
    for (( y=0; y<rows-1; y++ )); do
      local r=$(( (y * 7 + phase * 5) % 256 ))
      local g=$(( (y * 3 + phase * 11) % 256 ))
      local b=$(( (y * 13 + phase * 3) % 256 ))
      printf '\033[48;2;%d;%d;%dm%*s\033[0m\n' "$r" "$g" "$b" "$cols" ""
    done
    phase=$((phase + 1))
    sleep 0.033
  done
}

run_case() {
  local name="$1" starter="$2"
  echo
  echo "================================================================"
  echo "workload: $name  (${SECONDS_PER_RUN}s at scale ${SCALE})"
  echo "================================================================"
  # Let the screen settle so the previous workload's last frames are not counted in this one.
  sleep 1
  "$starter" &
  workload_pid=$!
  "$STATS" "$SECONDS_PER_RUN" "$SCALE" || true
  cleanup
  printf '\033[0m'
}

echo "Remote Screens spike 0.2 -- capture cost per workload"
echo "macOS: $(sw_vers -productVersion 2>/dev/null || echo 'n/a')  scale: $SCALE  seconds: $SECONDS_PER_RUN"

# Idle first, as the control. Every other row is only meaningful against it.
echo
echo "================================================================"
echo "workload: idle  (${SECONDS_PER_RUN}s at scale ${SCALE})"
echo "================================================================"
echo "leave the screen alone..."
"$STATS" "$SECONDS_PER_RUN" "$SCALE" || true

run_case "typing" typing_workload
run_case "scrolling" scrolling_workload

if [[ -n "$VIDEO_FILE" ]]; then
  [[ -f "$VIDEO_FILE" ]] || { echo "no such video file: $VIDEO_FILE" >&2; exit 1; }
  echo
  echo "================================================================"
  echo "workload: video (real: $VIDEO_FILE)"
  echo "================================================================"
  open "$VIDEO_FILE"
  sleep 3
  "$STATS" "$SECONDS_PER_RUN" "$SCALE" || true
else
  run_case "video (SYNTHETIC -- colour bands, not a decoded video)" synthetic_video_workload
fi

echo
echo "Done. Put the four rows in docs/engineering-evidence/RS2-screen-capture.md."
[[ -z "$VIDEO_FILE" ]] && echo "The video row is synthetic; re-run with a file argument for a real one."
exit 0
