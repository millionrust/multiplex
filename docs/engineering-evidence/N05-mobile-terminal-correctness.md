# N05 Native Mobile Terminal Correctness

**Evidence date:** 2026-09-03
**Status:** Complete for the native engine and current mobile acceptance gate

## Scope

N05 replaces transcript-style mobile rendering with one stateful `vt100` engine
exported from Rust and consumed by native SwiftUI and Compose presentation/input
layers. Direct SSH and paired Controller Sessions now share the same bounded VT
model on each platform while retaining separate transports and native UI.

The engine covers primary and alternate screens, DEC cursor/keypad modes, cursor
visibility, character and line insertion/deletion, styles and colors, Unicode
width/graphemes, bracketed paste, mouse modes/encoding, scrollback, resize, and
bounded high-output processing. Network reads feed parser state without encoding
a full snapshot for every packet; Swift and Kotlin request compact snapshots only
when their coalesced UI publishers render.

## Shared Corpus

`tests/fixtures/terminal/terminal-interactive-v1.json` contains deterministic
tmux-, vim-, and htop-shaped full-screen streams. Rust, the shipped iOS
XCFramework, and all four shipped Android JNI ABIs consume synchronized copies.
Every production-native test feeds each stream at every possible two-packet
boundary and compares lines, cursor, alternate-screen state, DEC modes,
bracketed-paste state, mouse mode/encoding, and scrollback.

## Results

| Verification | Result |
|---|---|
| `cargo test -p termirust-mobile-ffi` | PASS, 10/10 |
| Rust interactive corpus at every packet split | PASS, 3 cases |
| Rust 8,000-line bounded-output/compact-snapshot test | PASS |
| Android JVM tests and instrumentation APK assembly | PASS |
| Android production JNI tests on Pixel 9 emulator | PASS, 5 executed; 1 unrelated live-host test explicitly skipped |
| iOS production XCFramework interactive corpus test | PASS on iPhone 17 Pro simulator |
| iOS native bounded-terminal regression class | PASS except the first resource-copy attempt; project regeneration fixed the test bundle and the focused rerun passed |
| Fixture synchronization and `git diff --check` | PASS |

The final focused iOS result is under:

```text
/tmp/termirust-ios-derived/Logs/Test/Test-TermiRustMobile-2026.09.03_13-18-04-+0530.xcresult
```

## User Acceptance Already Completed

The N04 physical iPhone run exercised the same production Controller terminal
surface and passed visible cursor rendering, exact selection/copy, output follow
without blank overscroll, software-keyboard show/hide, portrait and landscape
layout, background/foreground recovery, writer release/reacquisition, and
bidirectional terminal input/output. Direct SSH now uses this same terminal model.

## Resource Bounds

- One network frame is capped at 1 MiB by the native FFI.
- Native terminal dimensions are capped at 1,000 by 1,000 cells.
- Native scrollback is capped at 50,000 rows and further restricted by each
  platform's retained-cell and model-byte budgets.
- Mobile scrollback sizing conservatively budgets 64 bytes per decoded cell.
- Snapshot wire rows omit trailing default blank cells; Swift and Kotlin validate
  dimensions and reconstruct padded rows for native rendering.
- High-rate output mutates parser state immediately but coalesces expensive JSON
  snapshot publication to the UI frame cadence.

## Known Limits

This evidence uses deterministic recordings shaped like representative tmux,
vim, and htop traffic rather than automating those third-party applications inside
the mobile UI. Physical Android OEM background behavior, hardware-keyboard
variants, and prolonged accessibility/IME soak remain qualification-matrix work
under N15. The iOS XCTest host continues to emit known SwiftNIO duplicate-class
warnings; the tests and production generic build remain successful.

Free repository-volume space after verification remained above the required
15 GiB threshold.
