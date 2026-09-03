# Shared Mobile Vault Crypto FFI

TermiRust mobile vault encryption lives in Rust so desktop, iOS, and Android use one implementation for Argon2id key derivation and AES-256-GCM-SIV decryption.

## Crate

The mobile-callable ABI is in:

```text
crates/termirust-mobile-ffi
```

Public C header:

```text
crates/termirust-mobile-ffi/include/termirust_mobile.h
```

Primary function:

```c
TermiRustMobileResult termirust_mobile_decrypt_vault_json(
    const uint8_t *encrypted_json_ptr,
    size_t encrypted_json_len,
    const uint8_t *passphrase_ptr,
    size_t passphrase_len);
```

On success, `result.ok` is true and `result.data` contains decrypted `MobileVaultExport` JSON. On failure, `result.ok` is false and `result.error` contains a UTF-8 error message.

Always call:

```c
termirust_mobile_free_result(result);
```

after copying the returned buffer.

## Sync Helper

Build and copy the shared crypto artifacts into the native mobile applications with:

```bash
cd /Users/jacob/Projects/terminal
scripts/sync-mobile-ffi-artifacts.sh all
```

Use `ios` or `android` instead of `all` to refresh one platform.

The helper respects:

```text
TERMIRUST_IOS_DIR
TERMIRUST_ANDROID_DIR
```

when the mobile source directories are not in their default monorepo locations.

## iOS Integration Path

The Swift app has a `MobileVaultDecrypting` protocol. Implement it with a thin Swift wrapper over `termirust_mobile_decrypt_vault_json`.

Build the XCFramework:

```bash
cd /Users/jacob/Projects/terminal
scripts/build-mobile-ffi-ios.sh
```

The script installs missing Rust iOS targets, builds the device and simulator static libraries, and writes:

```text
dist/mobile/ios/TermiRustMobileCrypto.xcframework
```

Copy or sync that generated framework into the iOS application before
opening Xcode. Prefer the sync helper above, or run:

```bash
rm -rf mobile/ios/Frameworks/TermiRustMobileCrypto.xcframework
cp -R \
  dist/mobile/ios/TermiRustMobileCrypto.xcframework \
  mobile/ios/Frameworks/
```

If Xcode reports `There is no XCFramework found`, rerun the build script and
copy command above.

## Android Integration Path

The Android app has a `MobileVaultDecryptor` interface. Implement it through JNI or UniFFI and delegate to the same Rust function.
The Kotlin prototype includes `NativeMobileVaultDecryptor`, which calls the JNI export:

```text
Java_com_termirust_mobile_data_NativeMobileVaultCrypto_decryptVaultJson
```

Build JNI libraries:

```bash
cd /Users/jacob/Projects/terminal
scripts/build-mobile-ffi-android.sh
```

The script installs missing Rust Android targets, finds the local Android NDK, and writes:

```text
dist/mobile/android/jniLibs/
```

Copy or sync those ABI folders into `mobile/android/app/src/main/jniLibs/`.
Prefer the sync helper above, or run:

```bash
rm -rf mobile/android/app/src/main/jniLibs
cp -R \
  dist/mobile/android/jniLibs \
  mobile/android/app/src/main/
```

## Verification

Rust-side checks:

```bash
cargo test -p termirust-mobile-ffi
cargo build -p termirust-mobile-ffi
```

Broader local mobile MVP checks:

```bash
scripts/verify-mobile-mvp.sh
```

Direct SSH/tmux smoke tests for both mobile apps:

```bash
scripts/verify-mobile-mvp.sh --live-ssh
```

The live smoke mode requires Docker Desktop to be running, or `TERMIRUST_MOBILE_TEST_SSH_*` variables pointing at a reachable SSH host.
If `termirust-e2e-sshd:local` already exists locally, the smoke scripts reuse it. Set `TERMIRUST_MOBILE_REBUILD_SSH_IMAGE=1` to force a rebuild.

After mobile linking:

- Import an encrypted vault exported by desktop TermiRust.
- Confirm mobile lists the same hosts.
- Confirm persistent tmux settings are present on the imported host.
- Confirm a wrong passphrase returns an error and no host data.
