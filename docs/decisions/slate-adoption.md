# Slate Adoption Status

Status: Tokens and status semantics adopted on every surface; Slate components blocked on the desktop GPUI migration

Reviewed: 2026-09-15

## Context

Slate has two layers. `design/tokens.toml` is the source of truth for color, spacing, type,
radii, motion, and the eight status kinds; `termirust-ui-contract` generates it into Rust,
Swift, and Kotlin. `crates/termirust-slate` is the styled component library built on those
tokens. An audit found the token layer only partly applied and the component layer unused.

## What is applied

| Surface | Tokens | Themes | Status shapes |
| ------- | ------ | ------ | ------------- |
| Desktop chrome, panels, dialogs | colors, spacing, type, radii, motion from `ui/theme.rs` | System, Dark, Light, High contrast, Recording; gpui-component controls recolored per theme | glyph and color in sessions, activity, overlays, artifacts, library |
| Desktop terminal | background, foreground, cursor, ANSI 0-15 | follows the app theme | n/a |
| iOS | generated `SlateTokens.swift`, dynamic `Color.slate` | light/dark/high contrast from the system | status colors |
| Android | generated `SlateTokens.kt`, Material color schemes built from it | light/dark | status colors |
| TUI | the user's terminal palette in Slate roles | the terminal's own | glyph per status, readable under `NO_COLOR` |

The visual-literal lint baseline shrank from 136 to 47 entries, and no entries were added.

## What is not applied

The desktop app still draws with `gpui` 0.2.2 and `gpui-component` 0.5.1. `termirust-slate`
is built on `gpui-pre` 0.3 and `gpui-base` 0.6, and its elements cannot be placed in a
`gpui` 0.2 window. So Slate's buttons, inputs, segmented controls, menus, palette, dialogs,
toasts, shell, and split panes are not used by the app, even though the app's own widgets
now read the same tokens.

The migration touches 35 files (about 81k lines) that import `gpui`. 28 of them import
`gpui-component`, mostly `button`, `input`, `scroll`, `Icon`/`IconName`, `Root`, and `Theme`.
`Input`/`InputState` and the scroll areas have no drop-in Slate replacement on `gpui` 0.2, so
this is one cut-over rather than an incremental swap.

## Known gaps

- The busy glyph (`ring_spinner`) is drawn static; the desktop does not animate it.
- The last test-module literals and the `TERMINAL_LINE_HEIGHT` terminal-grid exception remain
  outside the tokens.
- Host color chips stay user-chosen colors, not tokens.
- Some library strings, for example "No hosts pinned yet" and "Connected", are hard-coded
  English.
- The iOS and Android builds were not run for the token change; the Swift sources type-check
  against a macOS analog and the Kotlin sources were not compiled.

## Decision

Keep the token layer as the single source for every surface now. Adopt Slate components only
as part of a dedicated desktop migration from `gpui` 0.2 / `gpui-component` to `gpui-pre` /
`gpui-base`, planned as its own milestone.
