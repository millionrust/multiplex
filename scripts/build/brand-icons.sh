#!/usr/bin/env bash
# Renders every app icon from the SVG sources in design/brand/.
#
# The PNGs in the desktop bundle and both mobile apps are build output that happens to be
# committed, because the packagers need them: change the artwork in design/brand/ and run this,
# rather than editing a PNG. Chrome does the rasterising, since it is the one renderer this
# workspace can rely on having on every developer machine.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BRAND="$ROOT_DIR/design/brand"
DESKTOP_ICONS="$ROOT_DIR/crates/multiplex-desktop/assets/icons"
ANDROID_RES="${MULTIPLEX_ANDROID_DIR:-${TERMIRUST_ANDROID_DIR:-$ROOT_DIR/apps/android}}/app/src/main/res"
IOS_ICONSET="${MULTIPLEX_IOS_DIR:-${TERMIRUST_IOS_DIR:-$ROOT_DIR/apps/ios}}/MultiplexMobile/Assets.xcassets/AppIcon.appiconset"

CHROME="${MULTIPLEX_CHROME:-${TERMIRUST_CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}}"
if [[ ! -x "$CHROME" ]]; then
  CHROME="$(command -v google-chrome || command -v chromium || command -v chromium-browser || true)"
fi
[[ -x "$CHROME" ]] || {
  printf 'No Chrome found. Set MULTIPLEX_CHROME to a Chrome or Chromium binary.\n' >&2
  exit 1
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/multiplex-brand.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# One SVG, one size, one PNG. The page is exactly the icon, on nothing.
render() {
  local source=$1 size=$2 target=$3
  local page="$WORK/page.html"
  {
    printf '<!doctype html><meta charset="utf-8"><style>'
    printf 'html,body{margin:0;padding:0;background:transparent}'
    printf 'img{display:block;width:%spx;height:%spx}</style>' "$size" "$size"
    printf '<img src="%s" alt="">' "file://$source"
  } > "$page"
  "$CHROME" --headless --disable-gpu --hide-scrollbars \
    --default-background-color=00000000 \
    --force-device-scale-factor=1 \
    --window-size="$size,$size" \
    --virtual-time-budget=1500 \
    --screenshot="$target" "file://$page" >/dev/null 2>&1
  [[ -s "$target" ]] || { printf 'failed to render %s at %s\n' "$source" "$size" >&2; exit 1; }
  printf '  %4s  %s\n' "$size" "${target#"$ROOT_DIR"/}"
}

printf 'desktop\n'
render "$BRAND/icon-macos.svg" 512 "$DESKTOP_ICONS/app.png"
render "$BRAND/icon-macos.svg" 1024 "$DESKTOP_ICONS/app@2x.png"
cp "$BRAND/icon-macos.svg" "$DESKTOP_ICONS/app.svg"
printf '        %s\n' "${DESKTOP_ICONS#"$ROOT_DIR"/}/app.svg"

printf 'iOS\n'
render "$BRAND/icon-ios.svg" 1024 "$IOS_ICONSET/AppIcon.png"

printf 'Android\n'
render "$BRAND/icon-android-foreground.svg" 432 "$ANDROID_RES/drawable-nodpi/ic_launcher_foreground.png"
for density in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:144 xxxhdpi:192; do
  render "$BRAND/icon-rounded.svg" "${density#*:}" "$ANDROID_RES/mipmap-${density%%:*}/ic_launcher.png"
  render "$BRAND/icon-round.svg" "${density#*:}" "$ANDROID_RES/mipmap-${density%%:*}/ic_launcher_round.png"
done

printf 'done\n'
