#!/usr/bin/env sh
set -eu

workflow=.github/workflows/release.yml
ci=.github/workflows/ci.yml

test -f "$workflow"
test -f "$ci"

if grep -F '|| true' "$workflow" >/dev/null; then
  printf '%s\n' 'release workflow must not suppress packaging failures' >&2
  exit 1
fi

for name in termirust termirust-cli termirust-session-host termirust-mcp termirust-mcp-authorize termirust-relay; do
  count=$(grep -o "$name" "$workflow" | wc -l | tr -d ' ')
  if [ "$count" -lt 2 ]; then
    printf 'release workflow does not stage required executable: %s\n' "$name" >&2
    exit 1
  fi
done

grep -F 'if-no-files-found: error' "$workflow" >/dev/null
grep -F 'sha256' "$workflow" >/dev/null
grep -F 'macos-15-intel' "$workflow" >/dev/null
grep -F 'output-file: dist/TermiRust-${{ matrix.target.name }}.spdx.json' "$workflow" >/dev/null
grep -F 'uses: actions/attest@v4' "$workflow" >/dev/null
grep -F 'draft: true' "$workflow" >/dev/null
if grep -F 'macos-13' "$workflow" >/dev/null; then
  printf '%s\n' 'release workflow uses the retired macos-13 runner' >&2
  exit 1
fi
grep -F 'windows-2022' "$ci" >/dev/null
grep -F 'branches: [main, dev, test]' "$ci" >/dev/null
grep -F 'cargo test --workspace --all-targets --all-features --locked' "$ci" >/dev/null
grep -F 'mobile/android/scripts/verify-android-unified-routes.sh' "$ci" >/dev/null
grep -F 'mobile/android/gradlew -p mobile/android lintDebug' "$ci" >/dev/null
grep -F 'mobile/ios/scripts/verify-ios-unified-routes.sh' "$ci" >/dev/null

printf '%s\n' 'PASS: release workflow fails closed and CI covers Windows, Android, and iOS builds'
