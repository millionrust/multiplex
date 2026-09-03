#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
STAGE=""
AVD=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --stage) STAGE=${2:-}; shift 2 ;;
    --avd) AVD=${2:-}; shift 2 ;;
    *) echo "usage: $0 --stage pairing-fleet|readonly-terminal|writer-controls|terminal-conformance|terminal-interaction|terminal-acceptance|route-contract|universal-session [--avd name]" >&2; exit 2 ;;
  esac
done

case "$STAGE" in
  pairing-fleet) FILTER='*ControllerFleetTests' ;;
  readonly-terminal) FILTER='*ControllerReadOnlyTerminalTest' ;;
  writer-controls) FILTER='*ControllerWriterTest' ;;
  terminal-conformance) FILTER='*TerminalConformanceV*Test' ;;
  terminal-interaction) FILTER='*TerminalInteractionTest' ;;
  terminal-acceptance) FILTER='*TerminalAcceptanceTest' ;;
  route-contract) FILTER='*Route*Test*' ;;
  universal-session) FILTER='*UniversalSessionGoldenPathTest' ;;
  *) echo "a valid --stage is required" >&2; exit 2 ;;
esac

cd "$ROOT"
export ANDROID_HOME=${ANDROID_HOME:-"$HOME/Library/Android/sdk"}
./gradlew testDebugUnitTest --tests "$FILTER" --console=plain
if [ "$STAGE" = "terminal-interaction" ] || [ "$STAGE" = "terminal-acceptance" ] || [ "$STAGE" = "route-contract" ]; then
  ./gradlew testDebugUnitTest \
    --tests '*TerminalConformanceV*Test' \
    --tests '*ControllerReadOnlyTerminalTest' \
    --tests '*ControllerWriterTest' \
    --console=plain
fi
if [ "$STAGE" = "terminal-acceptance" ] || [ "$STAGE" = "route-contract" ]; then
  ./gradlew testDebugUnitTest \
    --tests '*TerminalAcceptanceTest' \
    --tests '*TerminalInteractionTest' \
    --console=plain
fi
./scripts/verify-android-unified-routes.sh --structural

if [ -n "$AVD" ]; then
  if command -v adb >/dev/null 2>&1 && adb get-state >/dev/null 2>&1; then
    echo "connected Android device available for manual $STAGE acceptance"
  else
    echo "no connected Android device; automated JVM/static checks passed, device acceptance remains" >&2
  fi
fi
