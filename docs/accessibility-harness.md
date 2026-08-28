# Accessibility Laboratory

The laboratory is an isolated development view for validating TermiRust's semantic tree,
keyboard focus contract, and macOS VoiceOver adapter. It does not claim whole-product WCAG
conformance and it does not alter saved application data.

## Automated verification

```bash
./scripts/verify-accessibility-harness.sh --platform macos --locale en-US,en-XA,ar-XB
```

The verifier checks deterministic semantic snapshots, stale action rejection, modal focus
restoration, bounded native action routing, all supported development locales, four token
themes, 100%/200% text scale, reduced motion, secret-canary exclusion, and the complete GPUI
desktop compile.

## Launch

```bash
TERMIRUST_AX_LOCALE=en-US \
TERMIRUST_AX_THEME=dark \
TERMIRUST_AX_SCALE=100 \
TERMIRUST_AX_REDUCED_MOTION=1 \
./target/debug/termirust --accessibility-harness
```

Accepted themes are `light`, `dark`, `high-contrast`, and `recording-friendly`. Accepted
scales are integers from `100` through `200`. Invalid development values fall back safely.

## VoiceOver protocol

1. Start VoiceOver with `Cmd+F5`, then launch the laboratory.
2. Navigate the Reference controls landmark and confirm the heading, list, field, menu,
   progress, status, disabled button and its reason, and destructive action are announced.
3. Activate Skip to reference controls and confirm focus moves to Session label.
4. Enter and erase field text. Confirm the value and linked validation error update.
5. Select both list rows and confirm selected state changes without activation on focus alone.
6. Adjust and cancel Reference operation. Confirm its bounded percentage and cancel action.
7. Open Test destructive confirmation. Confirm focus starts on Keep data, Tab and Shift-Tab
   remain in the dialog, Escape closes it, and focus returns to the opener.
8. Repeat open/close once to verify stable semantic identity and no stale action.
9. Activate Announce current status and confirm a polite announcement. Confirming the
   reference action uses an immediate announcement but changes no saved data.
10. Repeat with `en-XA` at scale `200`, `ar-XB`, and `high-contrast`. Confirm text remains
    visible, focus remains visible, and no control is clipped or unreachable.

Record the macOS version, VoiceOver version, locale/theme/scale, and any failure in the goal
completion evidence. Do not mark the VoiceOver acceptance passed from compile or snapshot
evidence alone.
