# E05 mobile route convergence

Date: 2026-08-29
Status: complete

## Outcome

The native iOS/iPadOS and Android products now expose saved direct-SSH Connections and
paired Devices/Device Sessions in one coherent application per platform. Route identity,
credentials, continuity, capabilities, lifecycle, and terminal ownership remain explicit:
tmux continuity is never presented as Host replay, and reconnect never resends input.

## Completed Children

- E05.1 route identity, capability, and navigation contract:
  `completion-evidence/e05.1-mobile-route-contract.md`
- E05.2 unified iOS and iPadOS application:
  `completion-evidence/e05.2-unified-ios-application.md`
- E05.3 unified Android application:
  `completion-evidence/e05.3-unified-android-application.md`
- E05.4 cross-route lifecycle and acceptance:
  `completion-evidence/e05.4-cross-route-acceptance.md`

## Exit Gate

Each native target can open direct SSH Connections and Host-owned Device Sessions without
a separate installation or shared secret namespace. Permanent route labels and canonical
capability projections prevent ownership confusion. The synchronized 15-case lifecycle
corpus passes on Swift and Kotlin, all prior E04 fixture and terminal gates remain green,
the unified Android APK builds, and the unified iOS target passes strict compilation under
the recorded Xcode runtime limitation.

This evidence does not claim App Store/Play release certification, a public operated relay,
or that direct SSH has durable Host-service replay semantics.
