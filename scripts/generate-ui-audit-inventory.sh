#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

baseline=""
output=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --baseline) baseline="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ ! "$baseline" =~ ^[0-9a-f]{40}$ || -z "$output" ]]; then
  echo "Usage: $0 --baseline FULL_GIT_HASH --output tests/ui/audit-cases.toml" >&2
  exit 2
fi

mkdir -p "$(dirname "$output")"
temporary="${output}.tmp.$$"
trap 'rm -f "$temporary"' EXIT

cat >"$temporary" <<EOF
version = 1
baseline_commit = "$baseline"
synthetic_canaries = [
  "TERMIRUST_AUDIT_SECRET_CANARY_7bd50a",
  "TERMIRUST_AUDIT_PATH_CANARY_32e146",
  "TERMIRUST_AUDIT_BIDI_CANARY_9ac274",
]
EOF

themes=(light dark high-contrast recording-friendly)
locales=(en-US en-XA ar-XB cjk-fixture)
scales=(100 200 400)
motions=(full reduced)
inputs=(keyboard voiceover pointer)
viewports=(960x640 1440x900@2x 720x900)
standard_states=(normal loading empty filter-empty partial offline permission-denied error cancelled recovery)
case_number=0

contains_state() {
  local wanted="$1"
  local csv="$2"
  [[ ",$csv," == *",$wanted,"* ]]
}

emit_case() {
  local screen="$1"
  local surface="$2"
  local state="$3"
  local coverage="$4"
  local rationale="${5:-}"
  case_number=$((case_number + 1))
  local index=$((case_number - 1))
  local theme="${themes[$((index % ${#themes[@]}))]}"
  local locale="${locales[$((index % ${#locales[@]}))]}"
  local scale="${scales[$((index % ${#scales[@]}))]}"
  local motion="${motions[$((index % ${#motions[@]}))]}"
  local input="${inputs[$((index % ${#inputs[@]}))]}"
  local viewport="${viewports[$((index % ${#viewports[@]}))]}"
  local privacy="synthetic"
  if [[ "$theme" == "recording-friendly" || "$screen" == "terminal-chrome" || "$screen" == "vault-keys-snippets" ]]; then
    privacy="synthetic-secret"
  fi
  printf '\n[[cases]]\n' >>"$temporary"
  printf 'id = "UA-%04d"\n' "$case_number" >>"$temporary"
  printf 'screen_id = "%s"\n' "$screen" >>"$temporary"
  printf 'surface = "%s"\n' "$surface" >>"$temporary"
  if [[ -n "$rationale" ]]; then
    printf 'route_fixture = "n-a"\n' >>"$temporary"
  else
    printf 'route_fixture = "synthetic-%s-%s"\n' "$screen" "$state" >>"$temporary"
  fi
  printf 'state = "%s"\n' "$state" >>"$temporary"
  printf 'viewport = "%s"\n' "$viewport" >>"$temporary"
  printf 'scale = %s\n' "$scale" >>"$temporary"
  printf 'theme = "%s"\n' "$theme" >>"$temporary"
  printf 'locale = "%s"\n' "$locale" >>"$temporary"
  printf 'motion = "%s"\n' "$motion" >>"$temporary"
  printf 'input_mode = "%s"\n' "$input" >>"$temporary"
  printf 'reader_steps = ["enter route", "read title and state", "traverse controls", "activate recovery or cancel when present", "escape and verify focus return"]\n' >>"$temporary"
  printf 'expected_semantics = ["named root", "text state independent of color", "bounded focus route", "no keyboard trap", "privacy projection"]\n' >>"$temporary"
  printf 'privacy_class = "%s"\n' "$privacy" >>"$temporary"
  printf 'coverage = "%s"\n' "$coverage" >>"$temporary"
  if [[ -n "$rationale" ]]; then
    printf 'n_a_reason = "%s"\n' "$rationale" >>"$temporary"
  fi
}

emit_screen() {
  local screen="$1"
  local surface="$2"
  local coverage="$3"
  local applicable="$4"
  local state
  for state in "${standard_states[@]}"; do
    if contains_state "$state" "$applicable"; then
      emit_case "$screen" "$surface" "$state" "$coverage"
    else
      emit_case "$screen" "$surface" "$state" "$coverage" "State is not part of this route's frozen product lifecycle; the owning surface state contract covers it elsewhere."
    fi
  done
}

emit_screen first-run shell-overlays-palette pairwise "normal,loading,empty,permission-denied,error,cancelled,recovery"
emit_screen shell-navigation shell-overlays-palette pairwise "normal,loading,empty,filter-empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen projects projects-groups-sessions pairwise "normal,loading,empty,filter-empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen sessions projects-groups-sessions pairwise "normal,loading,empty,filter-empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen presets-runtimes presets-runtimes pairwise "normal,loading,empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen worktrees-artifacts worktrees-artifacts pairwise "normal,loading,empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen hosts-connections hosts-connections pairwise "normal,loading,empty,filter-empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen sftp sftp pairwise "normal,loading,empty,filter-empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen vault-keys-snippets vault-keys-snippets full "normal,loading,empty,filter-empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen settings settings pairwise "normal,loading,empty,filter-empty,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen agent-canvas agent-canvas pairwise "normal,loading,empty,partial,offline,permission-denied,error,recovery"
emit_screen terminal-chrome terminal-chrome full "normal,loading,partial,offline,permission-denied,error,cancelled,recovery"
emit_screen destructive-confirmation cross-screen full "normal,permission-denied,error,cancelled,recovery"

for state in replaying gap backpressured detached exited malformed-unicode overflow-truncated; do
  emit_case terminal-chrome terminal-chrome "$state" full
done
for state in locked unlocking wrong-secret keyring-denied corrupt import-malformed oversize generating reviewing storage-failure; do
  emit_case vault-keys-snippets vault-keys-snippets "$state" full
done
for state in host-key-unknown host-key-mismatch authentication-denied invalid-target credential-store-unavailable; do
  emit_case hosts-connections hosts-connections "$state" pairwise
done
for state in conflict transferring disk-full resource-limit timeout; do
  emit_case sftp sftp "$state" pairwise
done

mv "$temporary" "$output"
trap - EXIT
echo "generated $case_number frozen UI audit cases at $output"
