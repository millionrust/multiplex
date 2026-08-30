# E06 free remote routes

Date: 2026-08-30
Status: complete

## Outcome

Rust, Swift, and Kotlin now share an explicit route model for local IPC, private LAN/VPN,
user-provided SSH, and optional self-hosted relay. Trust requirements, capabilities,
availability, degradation, retry, cancellation, revocation, mutation reconciliation, and
route switching are deterministic and fixture-backed. No account, public service, silent
fallback, or automatic trust downgrade was introduced.

## Completed Children

- E06.1: canonical route trust, capability, state, retry, and cancellation contract.
- E06.2: desktop route coordinator and recovery UX.
- E06.3: unified iOS/iPadOS route selection and ownership integration.
- E06.4: unified Android route selection and ownership integration.
- E06.5: shared cross-platform lifecycle, hostile-case, and continuity acceptance.

## Acceptance

`scripts/verify-remote-route-acceptance.sh` passed all shared lifecycle/switch programs and
the existing mobile, LAN, SSH, and local relay gates. Confirmed route changes clean up only
their source, target start remains explicit, revocation fails closed, reads retry within
bounds, and unknown mutation completion is queried rather than replayed.

Public relay operation remains prohibited by D05. Physical iOS/Android runtime verification
and dedicated native Controller-over-SSH/relay transport installation remain release/runtime
work, as recorded in the child evidence; unavailable routes stay visibly unavailable.

## Evidence

- `completion-evidence/e06.1-remote-route-contract.md`
- `completion-evidence/e06.2-desktop-route-coordinator.md`
- `completion-evidence/e06.3-unified-ios-remote-routes.md`
- `completion-evidence/e06.4-unified-android-remote-routes.md`
- `completion-evidence/e06.5-remote-route-acceptance.md`
