# Multiplex Mobile for Android

This folder contains the unified native Multiplex mobile application.

## Architecture

- **Connections** are saved direct-SSH destinations with device-local SSH credentials,
  mandatory known-host pins, and optional remote-tmux continuity.
- **Devices** are paired Multiplex desktops that list durable Device Sessions. The Host
  service owns replay, authoritative activity, and single-writer coordination.
- Device access uses one explicit route contract for private LAN/VPN, Controller over SSH,
  and an optional self-hosted relay. The app never silently falls back to another route.
- Route credentials, capabilities, lifecycle, and continuity remain separate inside one
  application and one APK.

## Current State

Implemented:

- Adaptive Compose shell with a phone navigation bar and tablet navigation rail.
- Responsive Devices route selector with persisted explicit choice, confirmation before
  switching, visible unavailable/degraded/revoked states, and source-owned cancellation.
- Shared Android route trust/capability/state policy with bounded reconnect, writer release,
  pending-input cleanup, command-ID reconciliation, and no mutation replay.
- Host/route/purpose/reference-scoped credential aliases backed by the Android Keystore
  secret abstraction; route configuration contains references, never secret material.
- Permanent Direct SSH and Device Session labels on terminal routes.
- Background privacy covers and pending-input cleanup for both terminal routes.
- Jetpack Compose Connection list and direct terminal detail.
- Versioned mobile vault models using kotlinx.serialization.
- Plaintext fixture import for unit tests, encrypted envelope inspection, and encrypted production vault import through the shared Rust crypto library.
- `NativeMobileVaultDecryptor` JNI adapter with `libmultiplex_mobile_ffi.so` packaged for Android ABIs in `app/src/main/jniLibs/`.
- Android document picker flow for encrypted mobile vault import.
- Android Keystore-backed secret storage.
- Selected-host credential entry that saves password/private-key material into Keystore-backed storage under the exported `secret_ref`, including private-key file import.
- Tmux bootstrap script generation.
- SSHJ-backed direct SSH session wiring with pinned known-host verification, Keystore-backed `secret_ref` lookup, PTY shell startup, tmux bootstrap injection, terminal input, resize, and disconnect.
- Transcript-level terminal buffering for common redraw/control sequences such as carriage return, backspace, ANSI SGR, line erase, cursor movement, and clear screen.
- JVM route, terminal, schema, tmux, lifecycle, and Controller protocol tests.
- Device/emulator instrumentation golden run against the real Rust Session Host,
  including pairing, Keystore storage, capability refresh, terminal control,
  reconnect, resize, revocation, and cleanup.
- Native Controller-over-SSH and self-hosted-relay adapters with pinned trust, fixed Controller
  bridge semantics, strict operator-package import, and no silent route fallback.

External-network relay qualification and signed Play distribution remain release-environment
work; they are not implied by local unit/build coverage.

## Build

Every binding and JNI library the APK packages is generated from the Rust crates, not
vendored, so build and sync them once before opening the project. Without them the Kotlin
sources name types that do not exist yet. From the repository root:

```bash
scripts/build/mobile-controller-bindings.sh --android
scripts/sync/mobile-controller-bindings.sh --write --android
scripts/build/mobile-screen-bindings.sh --android
scripts/sync/mobile-screen-bindings.sh --write --android
scripts/sync/mobile-ffi-artifacts.sh android
python3 scripts/build/mobile-replication-artifacts.py build --android
python3 scripts/build/mobile-replication-artifacts.py sync --android --write
```

The pinned NDK is what makes a released artifact reproducible; a build that only has to
prove the application still compiles can waive it with
`MULTIPLEX_CONTROLLER_BINDINGS_ALLOW_UNPINNED=1` and
`MULTIPLEX_REPLICATION_ARTIFACTS_ALLOW_UNPINNED=1`, which say so on stderr.

Use the checked-in Gradle wrapper:

```bash
cd /Users/jacob/Projects/terminal
ANDROID_HOME="$HOME/Library/Android/sdk" ./apps/android/gradlew \
  -p apps/android testDebugUnitTest assembleDebug
apps/android/scripts/verify-android-unified-routes.sh
```

If Android Studio installed the SDK somewhere else, replace the `sdk.dir` path
or set `ANDROID_HOME` for the Gradle commands.

Run the real Controller/Host golden path from the repository root. The runner
uses the sole connected device, or starts the first installed AVD when no device
is connected:

```bash
cd /Users/jacob/Projects/terminal
./scripts/test/mobile-android-controller-host.sh
```

Use `--serial <adb-serial>` when multiple devices are connected, or
`--avd <name>` to select a specific emulator.
