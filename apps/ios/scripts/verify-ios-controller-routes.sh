#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="${CONTROLLER_ROUTE_FIXTURE:-$ROOT_DIR/../../tests/fixtures/controller-routes/route-selection-v1.json}"
ACCEPTANCE_FIXTURE="${CONTROLLER_ROUTE_ACCEPTANCE_FIXTURE:-$ROOT_DIR/TermiRustMobileTests/Fixtures/remote-route-acceptance-v1.json}"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/termirust-ios-routes.XXXXXX")"
trap 'find "$TEMP_DIR" -depth -delete 2>/dev/null || true' EXIT

[[ -f "$FIXTURE" ]] || { printf 'Controller route fixture is missing: %s\n' "$FIXTURE" >&2; exit 1; }
[[ -f "$ACCEPTANCE_FIXTURE" ]] || { printf 'Controller route acceptance fixture is missing: %s\n' "$ACCEPTANCE_FIXTURE" >&2; exit 1; }

xcrun swiftc \
  -parse-as-library \
  -swift-version 6 \
  -strict-concurrency=complete \
  "$ROOT_DIR/TermiRustMobile/Models/ControllerRemoteRoute.swift" \
  "$ROOT_DIR/TermiRustMobile/Controller/AppleControllerRouteCoordinator.swift" \
  "$ROOT_DIR/scripts/controller-remote-route.swift" \
  -o "$TEMP_DIR/controller-remote-route"

"$TEMP_DIR/controller-remote-route" "$FIXTURE"

xcrun swiftc \
  -parse-as-library \
  -swift-version 6 \
  -strict-concurrency=complete \
  "$ROOT_DIR/TermiRustMobile/Models/ControllerRemoteRoute.swift" \
  "$ROOT_DIR/TermiRustMobile/Controller/AppleControllerRouteCoordinator.swift" \
  "$ROOT_DIR/scripts/controller-remote-route-acceptance.swift" \
  -o "$TEMP_DIR/controller-remote-route-acceptance"

"$TEMP_DIR/controller-remote-route-acceptance" "$ACCEPTANCE_FIXTURE"
git -C "$ROOT_DIR" diff --check
