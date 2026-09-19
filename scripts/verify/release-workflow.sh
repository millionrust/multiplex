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
# Windows compiles and tests every feature across every target. The job runs that as two commands,
# because nextest cannot run a bench whose harness prints its own report: nextest takes the
# libraries, binaries, integration tests and examples, and cargo test takes the benches. Together
# they are `cargo test --all-targets`, so the contract holds both, each with every feature.
grep -F 'cargo nextest run --workspace --lib --bins --tests --examples --all-features --locked' "$ci" >/dev/null
grep -F "cargo test --workspace --bench '*' --all-features --locked" "$ci" >/dev/null
grep -F 'apps/android/scripts/verify-android-unified-routes.sh' "$ci" >/dev/null
grep -F 'apps/android/gradlew -p apps/android lintDebug' "$ci" >/dev/null
grep -F 'apps/ios/scripts/verify-ios-unified-routes.sh' "$ci" >/dev/null

printf '%s\n' 'PASS: release workflow fails closed and CI covers Windows, Android, and iOS builds'
