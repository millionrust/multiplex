#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_DIR="${TERMIRUST_IOS_DIR:-$ROOT_DIR/mobile/ios}"
ANDROID_DIR="${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/mobile/android}"
MODE="${1:---check}"
SOURCE="$ROOT_DIR/tests/fixtures/universal-session-v1/golden.json"
IOS_FIXTURE="$IOS_DIR/TermiRustMobileTests/Fixtures/universal-session-v1.json"
ANDROID_FIXTURE="$ANDROID_DIR/app/src/test/resources/universal-session-v1.json"

if [[ "$MODE" != "--check" && "$MODE" != "--write" ]]; then
  printf 'Usage: scripts/sync-universal-session-fixture.sh [--check|--write]\n' >&2
  exit 2
fi

if [[ "$MODE" == "--write" ]]; then
  mkdir -p "$(dirname "$IOS_FIXTURE")" "$(dirname "$ANDROID_FIXTURE")"
  cp "$SOURCE" "$IOS_FIXTURE"
  cp "$SOURCE" "$ANDROID_FIXTURE"
  printf 'Universal Session fixture synced to Swift and Kotlin repositories.\n'
  exit 0
fi

cmp "$SOURCE" "$IOS_FIXTURE"
cmp "$SOURCE" "$ANDROID_FIXTURE"
printf 'Rust, Swift, and Kotlin Universal Session fixtures match.\n'
