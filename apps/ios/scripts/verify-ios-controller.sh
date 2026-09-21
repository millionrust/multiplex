#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAGE=""
REQUIRE_RUNTIME="${REQUIRE_IOS_RUNTIME:-0}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --stage)
      [[ $# -ge 2 ]] || { printf 'Missing value for --stage.\n' >&2; exit 2; }
      STAGE="$2"
      shift 2
      ;;
    --require-runtime)
      REQUIRE_RUNTIME=1
      shift
      ;;
    *)
      printf 'Usage: %s --stage pairing-fleet|readonly-terminal|writer-controls|terminal-conformance|terminal-interaction|terminal-acceptance|route-contract|universal-session [--require-runtime]\n' "$0" >&2
      exit 2
      ;;
  esac
done

[[ "$STAGE" == "pairing-fleet" || "$STAGE" == "readonly-terminal" || "$STAGE" == "writer-controls" || "$STAGE" == "terminal-conformance" || "$STAGE" == "terminal-interaction" || "$STAGE" == "terminal-acceptance" || "$STAGE" == "route-contract" || "$STAGE" == "universal-session" ]] || {
  printf 'Stage must be pairing-fleet, readonly-terminal, writer-controls, terminal-conformance, terminal-interaction, terminal-acceptance, route-contract, or universal-session.\n' >&2
  exit 2
}

cd "$ROOT_DIR"
[[ -d Frameworks/MultiplexControllerSecurity.xcframework ]] || {
  printf 'Controller security XCFramework is missing.\n' >&2
  exit 1
}
[[ -f MultiplexMobile/Generated/MultiplexControllerSecurity.swift ]] || {
  printf 'Generated Controller Swift binding is missing.\n' >&2
  exit 1
}
[[ -f MultiplexMobile/Localizable.xcstrings ]] || {
  printf 'Controller string catalog is missing.\n' >&2
  exit 1
}
command -v xcodegen >/dev/null || {
  printf 'xcodegen is required.\n' >&2
  exit 1
}

xcodegen generate --spec project.yml >/dev/null

SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
FRAMEWORKS="Frameworks/MultiplexControllerSecurity.xcframework/ios-arm64"
MOBILE_FRAMEWORKS="Frameworks/MultiplexMobileCrypto.xcframework/ios-arm64"
PLATFORM="/Applications/Xcode.app/Contents/Developer/Platforms/iPhoneOS.platform/Developer"
TEMP_MODULE="$(mktemp -d "${TMPDIR:-/tmp}/multiplex-ios-controller.XXXXXX")"
trap 'find "$TEMP_MODULE" -depth -delete 2>/dev/null || true' EXIT
TRANSPORT_STUBS="$TEMP_MODULE/ControllerTransportTypecheckStubs.swift"
cat >"$TRANSPORT_STUBS" <<'EOF'
import Foundation

private enum ControllerTransportTypecheckError: Error {
  case unavailable
}

enum SSHControllerTransport {
  static let remoteCommand = "multiplex controller-bridge --stdio"

  static func factory(
    hostID: String,
    configuration: ControllerRemoteRouteConfiguration,
    credentials: any ControllerRouteCredentialStoring
  ) throws -> ControllerTransportFactory {
    throw ControllerTransportTypecheckError.unavailable
  }
}

enum RelayControllerTransport {
  static func factory(
    hostID: String,
    configuration: ControllerRemoteRouteConfiguration,
    credentials: any ControllerRouteCredentialStoring
  ) throws -> ControllerTransportFactory {
    throw ControllerTransportTypecheckError.unavailable
  }
}
EOF
xcrun xcstringstool compile \
  MultiplexMobile/Localizable.xcstrings \
  --output-directory "$TEMP_MODULE/localization" \
  --dry-run >/dev/null

CONTROLLER_SOURCES=(
  MultiplexMobile/Generated/MultiplexControllerSecurity.swift
  MultiplexMobile/Models/ControllerModels.swift
  MultiplexMobile/Models/ControllerRemoteRoute.swift
  MultiplexMobile/Models/ControllerRemoteRouteConfiguration.swift
  MultiplexMobile/Models/MobileRouteContract.swift
  MultiplexMobile/Models/MobileCrossRouteAcceptance.swift
  MultiplexMobile/Controller/ControllerFleetCache.swift
  MultiplexMobile/Controller/AppleControllerRouteCoordinator.swift
  MultiplexMobile/Controller/PairedHostStore.swift
  MultiplexMobile/Security/ControllerKeychainBlobStore.swift
  MultiplexMobile/Security/ControllerRouteConfigurationStore.swift
  MultiplexMobile/Security/ControllerRouteCredentialStore.swift
  MultiplexMobile/Controller/ControllerRetryPolicy.swift
  MultiplexMobile/Controller/ControllerReadOnlyAttach.swift
  MultiplexMobile/Controller/ControllerWriterControl.swift
  MultiplexMobile/Terminal/BoundedTerminalBuffer.swift
  MultiplexMobile/Terminal/NativeControllerTerminal.swift
  MultiplexMobile/Terminal/GeneratedTerminalCellWidth.swift
  MultiplexMobile/Terminal/TerminalInteraction.swift
  MultiplexMobile/Terminal/TerminalAcceptance.swift
  MultiplexMobile/Controller/ControllerComputerDiscovery.swift
  MultiplexMobile/Controller/ControllerConnectionActor.swift
  MultiplexMobile/ViewModels/ControllerViewModel.swift
  MultiplexMobile/ViewModels/ControllerTerminalViewModel.swift
  MultiplexMobile/Views/SlateTokens.swift
  MultiplexMobile/Views/SlateColors.swift
  MultiplexMobile/Views/ControllerPresentation.swift
  MultiplexMobile/Views/ControllerRootView.swift
  MultiplexMobile/Views/ControllerReadOnlyTerminalView.swift
  MultiplexMobile/Views/ControllerTerminalInputView.swift
  MultiplexMobile/Views/ControllerQRCodeScanner.swift
)
CONTROLLER_SOURCES+=("$TRANSPORT_STUBS")
TEST_SOURCES=(
  MultiplexMobileTests/ControllerFleetCacheTests.swift
  MultiplexMobileTests/ControllerPairingFleetTests.swift
)
RUNTIME_TESTS=(
  -only-testing:MultiplexMobileTests/ControllerPairingFleetTests
  -only-testing:MultiplexMobileTests/ControllerFleetCacheTests
)
if [[ "$STAGE" == "readonly-terminal" || "$STAGE" == "writer-controls" || "$STAGE" == "terminal-conformance" || "$STAGE" == "terminal-interaction" || "$STAGE" == "terminal-acceptance" || "$STAGE" == "route-contract" || "$STAGE" == "universal-session" ]]; then
  TEST_SOURCES+=(MultiplexMobileTests/ControllerReadOnlyTerminalTests.swift)
  TEST_SOURCES+=(MultiplexMobileTests/BoundedTerminalBufferTests.swift)
  TEST_SOURCES+=(MultiplexMobileTests/ControllerTerminalViewModelTests.swift)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/ControllerReadOnlyTerminalTests)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/BoundedTerminalBufferTests)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/ControllerTerminalViewModelTests)
fi
if [[ "$STAGE" == "terminal-conformance" || "$STAGE" == "terminal-interaction" || "$STAGE" == "terminal-acceptance" || "$STAGE" == "route-contract" ]]; then
  TEST_SOURCES+=(MultiplexMobileTests/TerminalConformanceV1Tests.swift)
  TEST_SOURCES+=(MultiplexMobileTests/TerminalConformanceV2Tests.swift)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/TerminalConformanceV1Tests)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/TerminalConformanceV2Tests)
fi

if [[ "$STAGE" == "terminal-interaction" || "$STAGE" == "terminal-acceptance" || "$STAGE" == "route-contract" ]]; then
  xcrun swiftc \
    -swift-version 6 \
    -strict-concurrency=complete \
    -D TERMIRUST_TERMINAL_FALLBACK_ONLY \
    MultiplexMobile/Controller/ControllerReadOnlyAttach.swift \
    MultiplexMobile/Terminal/GeneratedTerminalCellWidth.swift \
    MultiplexMobile/Terminal/BoundedTerminalBuffer.swift \
    MultiplexMobile/Terminal/TerminalInteraction.swift \
    scripts/terminal-interaction.swift \
    -o "$TEMP_MODULE/terminal-interaction"
  "$TEMP_MODULE/terminal-interaction" \
    MultiplexMobileTests/Fixtures/terminal-interaction-v1.json
fi
if [[ "$STAGE" == "terminal-interaction" || "$STAGE" == "terminal-acceptance" || "$STAGE" == "route-contract" ]]; then
  TEST_SOURCES+=(MultiplexMobileTests/TerminalInteractionTests.swift)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/TerminalInteractionTests)
fi
if [[ "$STAGE" == "writer-controls" || "$STAGE" == "terminal-interaction" || "$STAGE" == "terminal-acceptance" || "$STAGE" == "route-contract" || "$STAGE" == "universal-session" ]]; then
  TEST_SOURCES+=(MultiplexMobileTests/ControllerWriterTests.swift)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/ControllerWriterTests)
fi
if [[ "$STAGE" == "terminal-acceptance" || "$STAGE" == "route-contract" ]]; then
  TEST_SOURCES+=(MultiplexMobileTests/TerminalAcceptanceTests.swift)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/TerminalAcceptanceTests)
  xcrun swiftc \
    -swift-version 6 \
    -strict-concurrency=complete \
    -D TERMIRUST_TERMINAL_FALLBACK_ONLY \
    MultiplexMobile/Controller/ControllerReadOnlyAttach.swift \
    MultiplexMobile/Terminal/GeneratedTerminalCellWidth.swift \
    MultiplexMobile/Terminal/BoundedTerminalBuffer.swift \
    MultiplexMobile/Terminal/TerminalAcceptance.swift \
    scripts/terminal-acceptance.swift \
    -o "$TEMP_MODULE/terminal-acceptance"
  "$TEMP_MODULE/terminal-acceptance" \
    MultiplexMobileTests/Fixtures/terminal-acceptance-v1.json
fi
if [[ "$STAGE" == "route-contract" ]]; then
  TEST_SOURCES+=(MultiplexMobileTests/AppleControllerRouteTests.swift)
  TEST_SOURCES+=(MultiplexMobileTests/AppleControllerRouteViewModelTests.swift)
  TEST_SOURCES+=(MultiplexMobileTests/MobileRouteContractTests.swift)
  TEST_SOURCES+=(MultiplexMobileTests/MobileCrossRouteAcceptanceTests.swift)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/MobileRouteContractTests)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/MobileCrossRouteAcceptanceTests)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/AppleControllerRouteTests)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/AppleControllerRouteViewModelTests)
  xcrun swiftc \
    -swift-version 6 \
    -strict-concurrency=complete \
    MultiplexMobile/Models/MobileRouteContract.swift \
    scripts/mobile-route-contract.swift \
    -o "$TEMP_MODULE/mobile-route-contract"
  "$TEMP_MODULE/mobile-route-contract" \
    MultiplexMobileTests/Fixtures/mobile-route-contract-v1.json
  "$ROOT_DIR/scripts/verify-ios-controller-routes.sh"
fi
if [[ "$STAGE" == "universal-session" ]]; then
  TEST_SOURCES+=(MultiplexMobileTests/UniversalSessionGoldenPathTests.swift)
  RUNTIME_TESTS+=(-only-testing:MultiplexMobileTests/UniversalSessionGoldenPathTests)
fi

xcrun swiftc \
  -emit-module \
  -parse-as-library \
  -enable-testing \
  -module-name MultiplexMobile \
  -swift-version 6 \
  -strict-concurrency=complete \
  -target arm64-apple-ios17.0 \
  -sdk "$SDK" \
  -F "$FRAMEWORKS" \
  -F "$MOBILE_FRAMEWORKS" \
  -emit-module-path "$TEMP_MODULE/MultiplexMobile.swiftmodule" \
  "${CONTROLLER_SOURCES[@]}"

xcrun swiftc \
  -typecheck \
  -swift-version 6 \
  -strict-concurrency=complete \
  -target arm64-apple-ios17.0 \
  -sdk "$SDK" \
  -F "$PLATFORM/Library/Frameworks" \
  -I "$PLATFORM/usr/lib" \
  -I "$TEMP_MODULE" \
  -F "$FRAMEWORKS" \
  -F "$MOBILE_FRAMEWORKS" \
  "${TEST_SOURCES[@]}"

xcrun swiftc -frontend -parse $(find MultiplexMobile MultiplexMobileTests -name '*.swift' -print)
git diff --check

if [[ "$STAGE" == "terminal-conformance" || "$STAGE" == "terminal-interaction" || "$STAGE" == "terminal-acceptance" || "$STAGE" == "route-contract" ]]; then
  xcrun swiftc \
    -swift-version 6 \
    -strict-concurrency=complete \
    -D TERMIRUST_TERMINAL_FALLBACK_ONLY \
    MultiplexMobile/Controller/ControllerReadOnlyAttach.swift \
    MultiplexMobile/Terminal/GeneratedTerminalCellWidth.swift \
    MultiplexMobile/Terminal/BoundedTerminalBuffer.swift \
    scripts/terminal-conformance-v1.swift \
    -o "$TEMP_MODULE/terminal-conformance-v1"
  "$TEMP_MODULE/terminal-conformance-v1" \
    MultiplexMobileTests/Fixtures/terminal-conformance-v1.json

  xcrun swiftc \
    -swift-version 6 \
    -strict-concurrency=complete \
    -D TERMIRUST_TERMINAL_FALLBACK_ONLY \
    MultiplexMobile/Controller/ControllerReadOnlyAttach.swift \
    MultiplexMobile/Terminal/GeneratedTerminalCellWidth.swift \
    MultiplexMobile/Terminal/BoundedTerminalBuffer.swift \
    scripts/terminal-conformance-v2.swift \
    -o "$TEMP_MODULE/terminal-conformance-v2"
  "$TEMP_MODULE/terminal-conformance-v2" \
    MultiplexMobileTests/Fixtures/terminal-conformance-v2.json
fi

IOS_DESTINATION="${TERMIRUST_IOS_DESTINATION:-}"
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
    -destination "$IOS_DESTINATION" \
    "${RUNTIME_TESTS[@]}"
  printf 'Controller iOS runtime tests passed on %s.\n' "$IOS_DESTINATION"
elif [[ "$REQUIRE_RUNTIME" == "1" ]]; then
  printf 'No available iPhone simulator runtime. Runtime verification is required.\n' >&2
  exit 1
else
  printf 'Controller source and test type-checks passed; no iPhone simulator runtime is installed.\n'
fi
