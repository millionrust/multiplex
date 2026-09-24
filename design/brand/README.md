# The Multiplex mark

A keycap with a shell prompt on it: a key you press to get somewhere else.

Everything here is the source. The PNGs in `crates/multiplex-desktop/assets/icons/`, both mobile
apps, and anywhere else an icon ships are rendered from these files by
`scripts/build/brand-icons.sh`, which needs Chrome (`MULTIPLEX_CHROME` points at another binary).
Change the artwork here and run the script; never edit a generated PNG.

| File | What it is |
| --- | --- |
| `mark.svg` | The cap on its own, in the app's blue. Websites, documents, the lockup. |
| `mark-mono.svg` | One shape with the glyph knocked out, filled with `currentColor`: menu-bar template images, favicons, stamps, anywhere one colour is all there is. |
| `icon-macos.svg` | The macOS app icon: the rounded body inside the 824/1024 content box Apple's own icons keep to. |
| `icon-ios.svg` | The iOS app icon: square and opaque, because iOS rounds the corners itself and refuses transparency. |
| `icon-rounded.svg` | A rounded square carrying its own corners, for Android's legacy launcher icon. |
| `icon-round.svg` | The circular launcher icon Android asks for beside the square one. |
| `icon-android-foreground.svg` | The adaptive icon's foreground layer, inside the 264-unit safe circle. Its background is `@color/ic_launcher_background`, which matches the field's middle stop. |

## Drawing it

A 64-unit grid. The cap is 46 × 40 on an 8-unit margin, corner radius 11, front lip 6 units. The
chevron sits one unit above the geometric centre, because the lip carries weight below it and a
centred glyph reads low. The glyph spans 62 % of the cap's width; narrower and it closes up at
18 px. The front lip is lighter than the top's shadow so the cap keeps its edge on black.

Colours are the app's own: `#74A7F2` accent over the `#2E3A50 → #11151C` field, glyph in
`#0E1622`. Keep clear space of one cap corner radius on every side, and do not put the coloured
cap on a mid-blue background — use `mark-mono.svg` there.

## What is not built yet

A Windows `.ico` and an embedded executable icon: the Windows builds still carry the default
Rust executable icon. The `.icns` is produced by cargo-bundle from `app.png` and `app@2x.png`.
