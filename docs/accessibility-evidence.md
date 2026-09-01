# Desktop accessibility and localization audit

This document records the frozen Goal 21.1.6 audit of TermiRust desktop application chrome on one
Apple-silicon macOS development machine. It is engineering evidence, not a VPAT, legal
certification, supported-platform declaration, or claim of complete WCAG conformance.

## Frozen scope

- Application baseline: `9f8924f927752f34db10acc9994344798d0a5fb0`
- Inventory: `tests/ui/audit-cases.toml`, version 1
- Inventory SHA-256: `215d8674447fa8773b93f7073ad5c42e6123b697ece40c7dc741755f40a2d532`
- Explicit cases: 157; deterministic pairwise/full variants: 12,348
- Locales: en-US, en-XA, ar-XB, plus a synthetic CJK fixture
- Themes: light, dark, high contrast, recording friendly
- Scale/reflow dimensions: 100%, 200%, 400%; reduced and full motion
- Input dimensions: keyboard, VoiceOver route, pointer
- Data: synthetic state, secret, path and bidi canaries only
- Machine: Apple-silicon (`arm64`), macOS 26.5.2 build 25F84, VoiceOver bundle version 10
- Accessibility automation permission: enabled; VoiceOver was not running during the automated
  audit, and no human-audible traversal is claimed

The inventory covers first run, shell/navigation/overlays/palette, Projects, Sessions, presets and
runtimes, worktrees and artifacts, Hosts and Connections, SFTP, vaults/keys/snippets, Settings,
Agent Canvas, terminal chrome, and destructive confirmations. Standard N/A states have rationale in
the frozen inventory; terminal, security and destructive cases use full matrix expansion.

## Passing evidence

- Every migrated surface's generated token, localization, semantic, state, scale, locale, theme,
  privacy and action-router verifier passes.
- The macOS accessibility harness passes deterministic semantic snapshots, focus restoration,
  stale action rejection, secret-canary exclusion, AppKit adapter tests and GPUI compilation.
- Fifty specified WCAG contrast token/state pairs pass their 4.5:1 text or 3:1 non-text threshold.
- The inventory hash check passes after execution, proving that no case was added or removed during
  the audit.
- Existing global token and localization baselines remain immutable; no new exception was added.

## Open findings

| Finding | Severity | Result | Owner goal | Release impact |
|---|---:|---|---|---|
| `AUD-AX-MANUAL-001` | A1 | Human-audible keyboard-only and VoiceOver traversal is not recorded for every frozen route. Automated semantics are not substituted for this evidence. | 21.1.7 | Blocks whole-product WCAG/VoiceOver claims. |
| `AUD-VISUAL-001` | A1 | No deterministic GPUI screenshot baseline/capture driver exists for all 100/200/400% theme and reflow cases. | 21.1.8 | Blocks whole-product visual reflow/rendered-contrast claims. |
| `AUD-L10N-LEGACY-001` | A1 | Whole-UI zero-copy audit reports 345 legacy copy findings. Catalog validation still passes. | 21.1.9 | Blocks complete whole-product localization claims. |
| `AUD-TOKEN-LEGACY-001` | A1 | Whole-UI zero-token audit reports 135 visual literals after named exceptions. | 21.1.10 | Blocks complete whole-product token consistency claims. |

The machine-readable register is `tests/ui/audit-results.json`. No application defect was fixed and
no frozen case was changed during this audit.

## Claim boundary

The evidence supports only this statement: on the recorded macOS development environment, the
migrated semantic contracts, generated English and pseudo-locales, privacy projections, and listed
token contrast pairs pass automated checks for the frozen application baseline. TermiRust must not
claim whole-product WCAG 2.2 AA, complete VoiceOver support, complete localization, or complete
200/400% visual reflow until Goals 21.1.7 through 21.1.10 are completed and their findings close.

Terminal output remains a bounded review projection rather than cell-by-cell semantics. No claim is
made for Narrator, Orca, TalkBack, iOS VoiceOver, additional human languages, or legal compliance.
