#!/usr/bin/env bash
set -euo pipefail

# Sign the completed bundle, after installing helpers or combining architecture slices.
# Ad-hoc signing binds bundle metadata for unsigned releases. A trusted identity is needed
# for macOS to recognize permission grants across different versions of the app.
[[ $# -eq 1 && -d "$1/Contents/MacOS" ]] || {
  printf 'Usage: scripts/build/sign-macos-app.sh Multiplex.app\n' >&2
  exit 2
}
APP="$1"
IDENTITY="${MULTIPLEX_CODESIGN_IDENTITY:-${TERMIRUST_CODESIGN_IDENTITY:--}}"
BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")"
MAIN="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")"
[[ "$BUNDLE_ID" == 'com.millionrust.multiplex' && -f "$APP/Contents/MacOS/$MAIN" ]] || {
  printf 'Expected the Multiplex bundle identifier and main executable.\n' >&2
  exit 1
}
for binary in "$APP"/Contents/MacOS/*; do
  [[ "$(basename "$binary")" == "$MAIN" ]] && continue
  codesign --force --sign "$IDENTITY" --identifier "$BUNDLE_ID.$(basename "$binary")" "$binary"
done
codesign --force --sign "$IDENTITY" --identifier "$BUNDLE_ID" "$APP"
codesign --verify --deep --strict "$APP"
