#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/create-ios-static-xcframework.sh \
  <module-name> <device-library> <simulator-library> <headers-dir> <output-xcframework>

Packages device and simulator static archives as a namespaced framework-style
XCFramework. Framework-style slices prevent multiple Rust module maps from
colliding in Xcode's shared include output.
USAGE
}

if [[ $# -ne 5 ]]; then
  usage >&2
  exit 2
fi

MODULE_NAME="$1"
DEVICE_LIBRARY="$2"
SIMULATOR_LIBRARY="$3"
HEADERS_DIR="$4"
OUTPUT="$5"

if [[ ! "$MODULE_NAME" =~ ^[A-Za-z][A-Za-z0-9_]*$ ]]; then
  printf 'Invalid framework module name: %s\n' "$MODULE_NAME" >&2
  exit 2
fi
for path in "$DEVICE_LIBRARY" "$SIMULATOR_LIBRARY" "$HEADERS_DIR/module.modulemap"; do
  if [[ ! -e "$path" ]]; then
    printf 'Static framework input is missing: %s\n' "$path" >&2
    exit 1
  fi
done
if [[ "$OUTPUT" != *.xcframework ]]; then
  printf 'XCFramework output must end in .xcframework: %s\n' "$OUTPUT" >&2
  exit 2
fi

STAGING="$(mktemp -d "${TMPDIR:-/tmp}/termirust-ios-framework.XXXXXX")"
cleanup() {
  find "$STAGING" -depth -delete 2>/dev/null || true
}
trap cleanup EXIT

create_framework() {
  local platform="$1"
  local library="$2"
  local root="$3"
  local framework="$root/$MODULE_NAME.framework"
  local bundle_suffix
  bundle_suffix="$(printf '%s' "$MODULE_NAME" | tr '[:upper:]' '[:lower:]')"

  mkdir -p "$framework/Headers" "$framework/Modules"
  cp "$library" "$framework/$MODULE_NAME"
  find "$HEADERS_DIR" -maxdepth 1 -type f -name '*.h' \
    -exec cp {} "$framework/Headers/" \;
  if ! find "$framework/Headers" -maxdepth 1 -type f -name '*.h' | grep -q .; then
    printf 'No public headers were found in %s.\n' "$HEADERS_DIR" >&2
    exit 1
  fi
  awk '
    !qualified && $1 == "module" {
      sub(/module/, "framework module")
      qualified = 1
    }
    { print }
  ' "$HEADERS_DIR/module.modulemap" > "$framework/Modules/module.modulemap"
  if ! grep -Eq "^[[:space:]]*framework module[[:space:]]+$MODULE_NAME[[:space:]]*\\{" \
    "$framework/Modules/module.modulemap"; then
    printf 'Module map does not declare %s.\n' "$MODULE_NAME" >&2
    exit 1
  fi

  cat > "$framework/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$MODULE_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>com.termirust.$bundle_suffix</string>
    <key>CFBundleName</key>
    <string>$MODULE_NAME</string>
    <key>CFBundlePackageType</key>
    <string>FMWK</string>
    <key>CFBundleSupportedPlatforms</key>
    <array><string>$platform</string></array>
    <key>MinimumOSVersion</key>
    <string>17.0</string>
</dict>
</plist>
PLIST
}

create_framework "iPhoneOS" "$DEVICE_LIBRARY" "$STAGING/device"
create_framework "iPhoneSimulator" "$SIMULATOR_LIBRARY" "$STAGING/simulator"

if [[ -e "$OUTPUT" ]]; then
  find "$OUTPUT" -depth -delete
fi
mkdir -p "$(dirname "$OUTPUT")"
xcodebuild -create-xcframework \
  -framework "$STAGING/device/$MODULE_NAME.framework" \
  -framework "$STAGING/simulator/$MODULE_NAME.framework" \
  -output "$OUTPUT"
