#!/usr/bin/env bash
# Shape one port's traffic into one of section 7's four network profiles, using the traffic shaper
# macOS already has (dummynet, driven through dnctl and pfctl). No downloads, no GUI.
#
#   sudo scripts/bench/condition-link.sh up 3 --port 51820
#   sudo scripts/bench/condition-link.sh status
#   sudo scripts/bench/condition-link.sh down
#
# Why a port and not the whole machine: Network Link Conditioner shapes every interface, and in this
# project the screen *host* runs on the same Mac as the thing being measured. Throttling the Mac
# throttles the host too, so the picture is degraded at both ends and the number means nothing. Pf
# can match a port, so only the screen session is shaped and everything else -- including whatever
# is on screen being captured -- runs at full speed.
#
# Find the port with the app running:
#   lsof -nP -iTCP -sTCP:LISTEN | grep -i multiplex
#
# SAFETY. These rules outlive the shell that made them. The script therefore:
#   - keeps everything in a named anchor (`termirust-cond`), so teardown cannot touch other rules;
#   - installs an automatic revert after --ttl seconds (default 3600) in case the session ends badly;
#   - refuses to configure a second profile over a live one without an explicit `down` first.
# If anything goes wrong, `sudo scripts/bench/condition-link.sh down` is always safe to run, and
# `sudo pfctl -a termirust-cond -F all && sudo dnctl -q flush` is the manual escape hatch.
set -euo pipefail

ANCHOR="termirust-cond"
PIPE_DOWN=1
PIPE_UP=2
STATE="/tmp/termirust-condition-link.state"

usage() {
  cat <<'USAGE'
usage: sudo condition-link.sh up <profile> --port <n> [--ttl <seconds>]
       sudo condition-link.sh down
       sudo condition-link.sh status
       condition-link.sh profiles

profiles (section 7 of the Remote Screens plan):
  1   20 ms RTT,  0% loss, uncapped
  2  100 ms RTT,  1% loss, 5 Mbit/s
  3  300 ms RTT,  5% loss, 1 Mbit/s
  4  500 ms RTT, 10% loss, 200 Kbit/s
USAGE
}

# RTT is round trip, so each direction gets half of it. A pipe delays one way.
profile_params() {
  case "$1" in
    1) HALF_RTT_MS=10  LOSS=0     BW="0"          NAME="20 ms, 0%, uncapped" ;;
    2) HALF_RTT_MS=50  LOSS=0.01  BW="5Mbit/s"    NAME="100 ms, 1%, 5 Mbit/s" ;;
    3) HALF_RTT_MS=150 LOSS=0.05  BW="1Mbit/s"    NAME="300 ms, 5%, 1 Mbit/s" ;;
    4) HALF_RTT_MS=250 LOSS=0.10  BW="200Kbit/s"  NAME="500 ms, 10%, 200 Kbit/s" ;;
    *) echo "unknown profile: $1 (expected 1-4)" >&2; exit 2 ;;
  esac
}

require_root() {
  [[ "$(id -u)" -eq 0 ]] || { echo "this needs sudo: dnctl and pfctl are root-only" >&2; exit 2; }
}

bring_down() {
  require_root
  pfctl -a "$ANCHOR" -F all 2>/dev/null || true
  dnctl pipe delete "$PIPE_DOWN" 2>/dev/null || true
  dnctl pipe delete "$PIPE_UP" 2>/dev/null || true
  rm -f "$STATE"
  echo "link conditioning removed (pf itself is left enabled; that is its normal state)"
}

case "${1:-}" in
  profiles) usage; exit 0 ;;
  down) bring_down; exit 0 ;;
  status)
    if [[ -f "$STATE" ]]; then
      echo "active: $(cat "$STATE")"
      dnctl list 2>/dev/null | grep -E "^0000[12]" || true
    else
      echo "no conditioning recorded by this script"
    fi
    exit 0
    ;;
  up) ;;
  *) usage; exit 2 ;;
esac

require_root
shift
PROFILE="${1:-}"
[[ -n "$PROFILE" ]] || { usage; exit 2; }
shift
PORT=""
TTL=3600
while [[ $# -gt 0 ]]; do
  case "$1" in
    --port) PORT="${2:-}"; shift 2 ;;
    --ttl)  TTL="${2:-}";  shift 2 ;;
    *) echo "unexpected argument: $1" >&2; usage; exit 2 ;;
  esac
done
[[ -n "$PORT" ]] || { echo "--port is required; find it with: lsof -nP -iTCP -sTCP:LISTEN | grep -i multiplex" >&2; exit 2; }

if [[ -f "$STATE" ]]; then
  echo "conditioning is already active: $(cat "$STATE")" >&2
  echo "run 'sudo $0 down' first, so one profile is never measured through another" >&2
  exit 2
fi

profile_params "$PROFILE"

# Two pipes, because a real link is asymmetric in what it is carrying even when its numbers are
# symmetric: the screen goes one way and acknowledgements come back the other.
dnctl pipe "$PIPE_DOWN" config delay "$HALF_RTT_MS" plr "$LOSS" ${BW:+bw "$BW"}
dnctl pipe "$PIPE_UP"   config delay "$HALF_RTT_MS" plr "$LOSS" ${BW:+bw "$BW"}

# An anchor keeps this separate from any other pf rules on the machine, so teardown is exact.
if ! pfctl -s Anchors 2>/dev/null | grep -qx "$ANCHOR"; then
  # Load the main ruleset with our anchor appended, preserving what was already there.
  (pfctl -s rules 2>/dev/null || true; echo "dummynet-anchor \"$ANCHOR\""; echo "anchor \"$ANCHOR\"") \
    | pfctl -f - 2>/dev/null || true
fi

printf 'dummynet in  quick proto {tcp,udp} from any to any port %s pipe %s\ndummynet out quick proto {tcp,udp} from any to any port %s pipe %s\n' \
  "$PORT" "$PIPE_DOWN" "$PORT" "$PIPE_UP" | pfctl -a "$ANCHOR" -f -

pfctl -E 2>/dev/null || true

echo "$NAME on port $PORT (ttl ${TTL}s)" > "$STATE"
echo "conditioning port $PORT: $NAME"
echo "verify with: ping does not cross this rule -- use the app, or 'nc' against that port"
echo "remove with: sudo $0 down"

# The dead-man's switch. If this terminal dies, the rules still come out on their own.
( sleep "$TTL"; [[ -f "$STATE" ]] && "$0" down >/dev/null 2>&1 ) >/dev/null 2>&1 &
disown 2>/dev/null || true
