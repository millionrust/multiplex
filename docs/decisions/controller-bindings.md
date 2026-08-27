# ADR: generated Controller-security bindings

- Status: Accepted for generated fixture conformance
- Decision date: 2026-08-27
- Decision owner: TermiRust engineering
- Release status: D01 and D06 still prohibit native Controller shipping and network exposure

## Decision

TermiRust pins UniFFI `0.32.0` exactly and uses its proc-macro metadata plus library-mode generator. The pinned Rust toolchain is `1.97.1`; generator and runtime crates come from the same lockfile. UniFFI is MPL-2.0 and the TermiRust wrapper remains MIT OR Apache-2.0. The standard Kotlin backend uses JNA, pinned by the Android application to `5.17.0`; the experimental UniFFI JNI backend is not selected.

The generated boundary lives in `termirust-controller-bindings`, separate from the legacy mobile vault/terminal ABI. It contains only immutable Controller-v1 DTOs, stable closed errors, an opaque stateful pairing/framing object, authorization evaluation, and a bounded `SecureBlobStore` foreign trait. Native Swift actors and Kotlin coroutines continue to own transport, retries, clocks, lifecycle, UI, terminal presentation, Keychain/Keystore policy, and artifact packaging.

The selection follows the official [UniFFI 0.32.0 changelog](https://github.com/mozilla/uniffi-rs/blob/main/CHANGELOG.md), [Swift binding contract](https://mozilla.github.io/uniffi-rs/latest/swift/overview.html), [foreign-trait contract](https://mozilla.github.io/uniffi-rs/latest/foreign_traits.html), and [Kotlin/JNA integration guidance](https://mozilla.github.io/uniffi-rs/latest/kotlin/gradle.html). UniFFI generates bindings but does not package target libraries, so TermiRust owns the deterministic XCFramework/JNI scripts and manifests.

## Toolchain and targets

- UniFFI crates/CLI/runtime: exactly `0.32.0`
- Rust: exactly `1.97.1-aarch64-apple-darwin`, minimal profile with rustfmt and Clippy
- iOS: `aarch64-apple-ios`; simulator: `aarch64-apple-ios-sim`, `x86_64-apple-ios`
- Android: `aarch64-linux-android`, `armv7-linux-androideabi`, `i686-linux-android`, `x86_64-linux-android`; API 26; NDK LLVM; 16 KiB ELF LOAD alignment
- Swift module: `TermiRustControllerSecurity`; C module: `TermiRustControllerSecurityFFI`
- Kotlin package: `com.termirust.controller.security`; library: `termirust_controller_bindings`

## Boundary contract

Every fallible export returns `ControllerBindingError`; no generic Rust error string crosses FFI. Exported vectors are length-checked before core parsing or crypto. Pairing objects serialize mutation with a Rust mutex, are safe to call from independent native threads, and make `finish` idempotent. Calls after cancellation/finish return `Disposed`; generated native object destruction is independently idempotent. Rust catches unexpected panics before UniFFI's generated throwing boundary catches them again.

`SecureBlobStore` accepts opaque IDs of at most 128 restricted ASCII bytes and blobs of at most 4 KiB. Callback exceptions map to `SecureBlobUnavailable`; locked, denied, missing, invalid, and unavailable remain closed typed states. Foreign implementations must be thread-safe and non-reentrant, must not retain callback byte buffers, and must not hold a strong reference back to the Rust object. Rust may invoke callbacks on the calling thread; native code must dispatch UI work separately. Native implementations own Keychain/Keystore access policy and zeroize temporary mutable buffers where their runtime permits.

Private key vectors are conspicuous tests only. Production static key bytes are loaded through the foreign store and copied only as required by UniFFI/JNA/Swift bridging, then zeroized in Rust. Debug output and errors contain no key, nonce, SAS, frame, path, key ID, or callback value.

## Reproducibility and promotion

`build-mobile-controller-bindings.sh` builds every target in a fresh isolated target directory, generates both languages with `--no-format`, verifies Android page alignment, records public ABI symbols and SHA-256 for every mobile release output, and promotes only a complete staged tree. A failed build leaves the prior destination unchanged. `verify-mobile-controller-bindings.sh --rebuild-twice` performs two clean builds and requires byte-identical generated sources, mobile libraries, frameworks, symbols, provenance, and manifests. `sync-mobile-controller-bindings.sh --check` rejects stale native copies.

The macOS dylib under `kotlin-test/` is local test support, not a mobile release artifact. Its Mach-O UUID is linker-generated, so it is rebuilt and executed by native conformance tests but excluded from release hashes and the byte-for-byte mobile artifact comparison.

Any UniFFI, Rust, target, JNA, DTO, enum, method, callback, or generator-option change requires an ADR revision, lockfile review, regenerated artifacts/API/ABI hashes, all Rust/native vector tests, and a two-build comparison. Generated files are never manually edited.

## Residual gates

This decision proves a generated in-memory security boundary, not a production mobile security claim. UniFFI 0.32.0, JNA, the generated code, and this integration have not received an independent security audit. D01/D06, route-specific threat tests, platform secure-store conformance, native lifecycle work, and the later iOS/Android goals remain mandatory before shipping or remote exposure.
