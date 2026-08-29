# E04 native mobile terminal correctness

Date: 2026-08-29
Status: complete

## Outcome

The Rust desktop, native Swift, and native Kotlin terminal implementations now share
versioned fixtures for parser cells/modes/scrollback, styled Unicode width and resize,
selection/clipboard/keyboard/IME interaction, and adaptive accessibility/lifecycle
acceptance. Both mobile clients retain native UI ownership and bounded input/output.

## Completed Children

- E04.1 core cross-platform terminal conformance:
  `completion-evidence/e04.1-cross-platform-terminal-conformance-v1.md`
- E04.2 style, Unicode width, and resize conformance:
  `completion-evidence/e04.2-native-style-unicode-resize-conformance.md`
- E04.3 selection, clipboard, keyboard, and IME conformance:
  `completion-evidence/e04.3-native-selection-clipboard-keyboard-ime-conformance.md`
- E04.4 adaptive layout, accessibility, performance, and lifecycle acceptance:
  `completion-evidence/e04.4-adaptive-accessibility-performance-lifecycle.md`

## Exit Gate

All canonical fixture suites and native acceptance gates pass. Runtime/device limitations
are recorded in E04.4 evidence; release-platform certification remains outside E04.
