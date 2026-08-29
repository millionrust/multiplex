#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SWIFT_ROOT="$ROOT/../terminal_app/terminal_swift"
KOTLIN_ROOT="$ROOT/../terminal_app/terminal_kotlin"

FILES="
tests/fixtures/terminal/terminal-conformance-v1.json|TermiRustMobileTests/Fixtures/terminal-conformance-v1.json|app/src/testController/resources/terminal-conformance-v1.json
tests/fixtures/terminal/terminal-conformance-v2.json|TermiRustMobileTests/Fixtures/terminal-conformance-v2.json|app/src/testController/resources/terminal-conformance-v2.json
tests/fixtures/terminal/terminal-interaction-v1.json|TermiRustMobileTests/Fixtures/terminal-interaction-v1.json|app/src/testController/resources/terminal-interaction-v1.json
tests/fixtures/terminal/generated/GeneratedTerminalCellWidth.swift|TermiRustMobile/Terminal/GeneratedTerminalCellWidth.swift|-
tests/fixtures/terminal/generated/GeneratedTerminalCellWidth.kt|-|app/src/main/java/com/termirust/mobile/controller/GeneratedTerminalCellWidth.kt
"

case "${1:-}" in
  --check)
    printf '%s' "$FILES" | while IFS='|' read -r source swift kotlin; do
      [ -n "$source" ] || continue
      if [ "$swift" != "-" ]; then
        cmp -s "$ROOT/$source" "$SWIFT_ROOT/$swift" || {
          echo "Swift terminal conformance asset differs: $swift" >&2
          exit 1
        }
      fi
      if [ "$kotlin" != "-" ]; then
        cmp -s "$ROOT/$source" "$KOTLIN_ROOT/$kotlin" || {
          echo "Kotlin terminal conformance asset differs: $kotlin" >&2
          exit 1
        }
      fi
    done
    echo "Terminal conformance fixtures match."
    ;;
  "")
    printf '%s' "$FILES" | while IFS='|' read -r source swift kotlin; do
      [ -n "$source" ] || continue
      if [ "$swift" != "-" ]; then
        mkdir -p "$(dirname "$SWIFT_ROOT/$swift")"
        cp "$ROOT/$source" "$SWIFT_ROOT/$swift"
      fi
      if [ "$kotlin" != "-" ]; then
        mkdir -p "$(dirname "$KOTLIN_ROOT/$kotlin")"
        cp "$ROOT/$source" "$KOTLIN_ROOT/$kotlin"
      fi
    done
    echo "Terminal conformance fixtures synchronized."
    ;;
  *)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac
