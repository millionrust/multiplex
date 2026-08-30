#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
swift_root="$root/../terminal_app/terminal_swift"
kotlin_root="$root/../terminal_app/terminal_kotlin"

case "${1:-}" in
  "") full=1 ;;
  --contract-only) full=0 ;;
  *) echo "usage: $0 [--contract-only]" >&2; exit 2 ;;
esac

cd "$root"
python3 scripts/verify-remote-route-acceptance.py
./scripts/sync-terminal-conformance-fixture.sh --check
cargo test -p termirust-domain controller_route --locked
cargo test -p termirust controller::route_coordinator::tests::shared_acceptance --locked

CONTROLLER_ROUTE_FIXTURE="$root/tests/fixtures/controller-routes/route-selection-v1.json" \
CONTROLLER_ROUTE_ACCEPTANCE_FIXTURE="$root/tests/fixtures/controller-routes/remote-route-acceptance-v1.json" \
  "$swift_root/scripts/verify-ios-controller-routes.sh"
(
  cd "$kotlin_root"
  ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}" \
    ./gradlew testDebugUnitTest --tests '*RemoteRoute*Test' --console=plain
)

if [ "$full" -eq 1 ]; then
  python3 scripts/verify-mobile-route-contract.py
  python3 scripts/verify-mobile-cross-route-acceptance.py
  "$swift_root/scripts/verify-ios-controller.sh" --stage route-contract
  (
    cd "$kotlin_root"
    ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}" \
      ./scripts/verify-android-controller.sh --stage route-contract
  )
  ./scripts/verify-controller-lan.sh
  ./scripts/test-controller-ssh.sh
  ./scripts/test-desktop-relay-route.sh --local-only --fault-matrix
fi

git diff --check
echo "cross-platform remote route acceptance passed"
