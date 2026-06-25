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

## iOS Integration Path

The Swift app has a `MobileVaultDecrypting` protocol. Implement it with a thin Swift wrapper over `termirust_mobile_decrypt_vault_json`.

Build the XCFramework:

```bash
scripts/build-mobile-ffi-ios.sh
```

The script installs missing Rust iOS targets, builds the device and simulator static libraries, and writes:

```text
dist/mobile/ios/TermiRustMobileCrypto.xcframework
```

Link that XCFramework into `terminal_swift`.

## Android Integration Path

The Android app has a `MobileVaultDecryptor` interface. Implement it through JNI or UniFFI and delegate to the same Rust function.

Build JNI libraries:

```bash
scripts/build-mobile-ffi-android.sh
```

The script installs missing Rust Android targets, finds the local Android NDK, and writes:

```text
dist/mobile/android/jniLibs/
```

Copy or sync those ABI folders into `terminal_kotlin/app/src/main/jniLibs/`.

## Verification

Rust-side checks:

```bash
cargo test -p termirust-mobile-ffi
cargo build -p termirust-mobile-ffi
```

After mobile linking:

- Import an encrypted vault exported by desktop TermiRust.
- Confirm mobile lists the same hosts.
- Confirm persistent tmux settings are present on the imported host.
- Confirm a wrong passphrase returns an error and no host data.
