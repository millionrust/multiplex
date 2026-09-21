#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/build/macos-universal.sh ARM64_ZIP X86_64_ZIP OUTPUT_ZIP

Merges the Apple silicon and Intel builds of Multiplex.app into one universal app: every
executable in Contents/MacOS becomes a fat binary holding both slices, and everything else in the
bundle comes from the Apple silicon build. Fails unless both bundles hold the same executables and
every merged one reports exactly arm64 and x86_64.
USAGE
}

[[ $# -eq 3 ]] || { usage >&2; exit 2; }
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ARM_ZIP="$1"
X86_ZIP="$2"
OUTPUT_ZIP="$3"
[[ -f "$ARM_ZIP" && -f "$X86_ZIP" ]] || { printf 'Both input zips must exist.\n' >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/multiplex-universal.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
ditto -x -k "$ARM_ZIP" "$WORK/arm64"
ditto -x -k "$X86_ZIP" "$WORK/x86_64"
ARM_APP="$WORK/arm64/Multiplex.app"
X86_APP="$WORK/x86_64/Multiplex.app"
[[ -d "$ARM_APP/Contents/MacOS" && -d "$X86_APP/Contents/MacOS" ]] || {
  printf 'Each zip must contain Multiplex.app.\n' >&2
  exit 1
}

# The same executables on both sides: a sidecar missing from one build would otherwise ship as a
# single-architecture file inside an app that claims to be universal.
if ! diff <(cd "$ARM_APP/Contents/MacOS" && ls) <(cd "$X86_APP/Contents/MacOS" && ls) >/dev/null; then
  printf 'The two builds do not contain the same executables:\n' >&2
  diff <(cd "$ARM_APP/Contents/MacOS" && ls) <(cd "$X86_APP/Contents/MacOS" && ls) >&2 || true
  exit 1
fi

UNIVERSAL_APP="$WORK/universal/Multiplex.app"
mkdir -p "$WORK/universal"
ditto "$ARM_APP" "$UNIVERSAL_APP"
for arm_binary in "$ARM_APP"/Contents/MacOS/*; do
  name="$(basename "$arm_binary")"
  lipo -create "$arm_binary" "$X86_APP/Contents/MacOS/$name" \
    -output "$UNIVERSAL_APP/Contents/MacOS/$name"
  archs="$(lipo -archs "$UNIVERSAL_APP/Contents/MacOS/$name" | tr ' ' '\n' | sort | xargs)"
  if [[ "$archs" != "arm64 x86_64" ]]; then
    printf '%s holds %s, not arm64 and x86_64.\n' "$name" "$archs" >&2
    exit 1
  fi
  printf '%s: %s\n' "$name" "$archs"
done

"$ROOT_DIR/scripts/verify/release-package.sh" "$UNIVERSAL_APP/Contents/MacOS"
mkdir -p "$(dirname "$OUTPUT_ZIP")"
rm -f "$OUTPUT_ZIP"
ditto -c -k --keepParent "$UNIVERSAL_APP" "$OUTPUT_ZIP"
printf 'Wrote %s\n' "$OUTPUT_ZIP"
