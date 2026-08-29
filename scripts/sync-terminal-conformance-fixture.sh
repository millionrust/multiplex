#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SOURCE="$ROOT/tests/fixtures/terminal/terminal-conformance-v1.json"
SWIFT="$ROOT/../terminal_app/terminal_swift/TermiRustMobileTests/Fixtures/terminal-conformance-v1.json"
KOTLIN="$ROOT/../terminal_app/terminal_kotlin/app/src/testController/resources/terminal-conformance-v1.json"

case "${1:-}" in
  --check)
    cmp -s "$SOURCE" "$SWIFT" || {
      echo "Swift terminal conformance fixture differs from the canonical Rust fixture." >&2
      exit 1
    }
    cmp -s "$SOURCE" "$KOTLIN" || {
      echo "Kotlin terminal conformance fixture differs from the canonical Rust fixture." >&2
      exit 1
    }
    echo "Terminal conformance fixtures match."
    ;;
  "")
    cp "$SOURCE" "$SWIFT"
    cp "$SOURCE" "$KOTLIN"
    echo "Terminal conformance fixtures synchronized."
    ;;
  *)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac
