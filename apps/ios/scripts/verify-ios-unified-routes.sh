#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REQUIRE_RUNTIME="${REQUIRE_IOS_RUNTIME:-0}"
CONFIGURATION="Debug"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --require-runtime)
      REQUIRE_RUNTIME=1
      shift
      ;;
    --configuration)
      [[ $# -ge 2 ]] || { printf 'Missing configuration value.\n' >&2; exit 2; }
      CONFIGURATION="$2"
      shift 2
      ;;
    *)
      printf 'Usage: %s [--require-runtime]\n' "$0" >&2
      exit 2
      ;;
  esac
done

cd "$ROOT_DIR"

command -v xcodegen >/dev/null || { printf 'xcodegen is required.\n' >&2; exit 1; }
[[ -d Frameworks/MultiplexMobileCrypto.xcframework ]] || {
  printf 'Direct SSH crypto XCFramework is missing.\n' >&2
  exit 1
}
[[ -d Frameworks/MultiplexControllerSecurity.xcframework ]] || {
  printf 'Controller security XCFramework is missing.\n' >&2
  exit 1
}

required_sources=(
  MultiplexMobile/SSH/MobileSSHSession.swift
  MultiplexMobile/SSH/TmuxBootstrap.swift
  MultiplexMobile/ViewModels/HostListViewModel.swift
  MultiplexMobile/Controller/AppleControllerRouteCoordinator.swift
  MultiplexMobile/Models/ControllerRemoteRoute.swift
  MultiplexMobile/Views/ContentView.swift
  MultiplexMobile/Views/ControllerRootView.swift
)
for path in "${required_sources[@]}"; do
  [[ -f "$path" ]] || { printf 'Unified route source is missing: %s\n' "$path" >&2; exit 1; }
done

grep -Eq '^[[:space:]]+UNIFIED_MOBILE_ROUTES:[[:space:]]+1$' project.yml || {
  printf 'The product target must enable unified mobile routes.\n' >&2
  exit 1
}
grep -Eq 'ContentView\(viewModel: connectionViewModel\)' MultiplexMobile/App/MultiplexMobileApp.swift || {
  printf 'Unified navigation does not expose saved Connections.\n' >&2
  exit 1
}
grep -Eq 'ControllerRootView\(viewModel: controllerViewModel\)' MultiplexMobile/App/MultiplexMobileApp.swift || {
  printf 'Unified navigation does not expose paired Devices.\n' >&2
  exit 1
}
grep -Eq 'TabView\(selection: \$destination\)' MultiplexMobile/App/MultiplexMobileApp.swift || {
  printf 'Unified navigation is not bound to the canonical root destination.\n' >&2
  exit 1
}
grep -q 'Section("Connection Route")' MultiplexMobile/Views/ControllerRootView.swift || {
  printf 'Devices does not expose explicit Controller route selection.\n' >&2
  exit 1
}

xcodegen generate --spec project.yml >/dev/null
project_file=MultiplexMobile.xcodeproj/project.pbxproj
for marker in MobileSSHSession TmuxBootstrap HostListViewModel ControllerRootView MultiplexMobileCrypto MultiplexControllerSecurity NIOSSH; do
  grep -q "$marker" "$project_file" || {
    printf 'Generated unified target is missing %s.\n' "$marker" >&2
    exit 1
  }
done

TEMP_MODULE="$(mktemp -d "${TMPDIR:-/tmp}/multiplex-ios-unified.XXXXXX")"
trap 'find "$TEMP_MODULE" -depth -delete 2>/dev/null || true' EXIT

xcrun xcstringstool compile \
  MultiplexMobile/Localizable.xcstrings \
  --output-directory "$TEMP_MODULE/localization" \
  --dry-run >/dev/null

xcrun swiftc -frontend -parse $(find MultiplexMobile MultiplexMobileTests -name '*.swift' -print)

BUILD_LOG="$TEMP_MODULE/xcodebuild.log"
set +e
xcodebuild build \
  -project MultiplexMobile.xcodeproj \
  -scheme MultiplexMobile \
  -configuration "$CONFIGURATION" \
  -destination 'generic/platform=iOS' \
  CODE_SIGNING_ALLOWED=NO >"$BUILD_LOG" 2>&1
BUILD_STATUS=$?
set -e
if [[ "$BUILD_STATUS" == "0" ]]; then
  printf 'Unified iOS device build passed.\n'
elif grep -Eq 'Unable to find a destination|Found no destinations' "$BUILD_LOG" \
  && [[ "$REQUIRE_RUNTIME" != "1" ]]; then
  printf 'Unified source and lifecycle tests type-checked; this Xcode has no eligible iOS destination.\n'
elif grep -Eq 'Unable to find a destination|Found no destinations' "$BUILD_LOG"; then
  printf 'No eligible iOS destination is installed; runtime verification is required.\n' >&2
  exit 1
else
  cat "$BUILD_LOG" >&2
  exit "$BUILD_STATUS"
fi

IOS_DESTINATION="${MULTIPLEX_IOS_DESTINATION:-${TERMIRUST_IOS_DESTINATION:-}}"
if [[ -z "$IOS_DESTINATION" ]]; then
  simulator_id="$(
    xcrun simctl list devices available 2>/dev/null \
      | awk -F '[()]' '/iPhone/ { print $2; exit }'
  )"
  if [[ -n "$simulator_id" ]]; then
    IOS_DESTINATION="platform=iOS Simulator,id=$simulator_id"
  fi
fi
if [[ -n "$IOS_DESTINATION" ]]; then
  xcodebuild test -quiet \
    -project MultiplexMobile.xcodeproj \
    -scheme MultiplexMobile \
    -configuration "$CONFIGURATION" \
    -destination "$IOS_DESTINATION" \
    -only-testing:MultiplexMobileTests/UnifiedRouteLifecycleTests \
    -only-testing:MultiplexMobileTests/AppleControllerRouteTests \
    -only-testing:MultiplexMobileTests/AppleControllerRouteViewModelTests
  printf 'Unified production lifecycle tests passed on %s.\n' "$IOS_DESTINATION"
elif [[ "$REQUIRE_RUNTIME" == "1" ]]; then
  printf 'No eligible iOS destination is installed; runtime verification is required.\n' >&2
  exit 1
else
  printf 'Unified production target compiled; runtime lifecycle tests were SKIPPED.\n'
fi

git diff --check
printf 'Unified iOS target includes direct SSH Connections and explicit paired Device routes.\n'
