use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;
use std::str;
use termirust_protocol::decrypt_mobile_vault_export_to_json;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TermiRustMobileByteBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TermiRustMobileResult {
    pub ok: bool,
    pub data: TermiRustMobileByteBuffer,
    pub error: TermiRustMobileByteBuffer,
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_decrypt_vault_json(
    encrypted_json_ptr: *const u8,
    encrypted_json_len: usize,
    passphrase_ptr: *const u8,
    passphrase_len: usize,
) -> TermiRustMobileResult {
    match catch_unwind(AssertUnwindSafe(|| {
        decrypt_vault_json(
            encrypted_json_ptr,
            encrypted_json_len,
            passphrase_ptr,
            passphrase_len,
        )
    })) {
        Ok(result) => result,
        Err(_) => error_result("TermiRust mobile vault decryptor panicked."),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_free_result(result: TermiRustMobileResult) {
    termirust_mobile_free_buffer(result.data);
    termirust_mobile_free_buffer(result.error);
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_free_buffer(buffer: TermiRustMobileByteBuffer) {
    if buffer.ptr.is_null() || buffer.len == 0 {
        return;
    }

    unsafe {
        drop(Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.len));
    }
}

fn decrypt_vault_json(
    encrypted_json_ptr: *const u8,
    encrypted_json_len: usize,
    passphrase_ptr: *const u8,
    passphrase_len: usize,
) -> TermiRustMobileResult {
    let encrypted_json = match read_utf8(encrypted_json_ptr, encrypted_json_len, "vault JSON") {
        Ok(value) => value,
        Err(error) => return error_result(&error),
    };
    let passphrase = match read_utf8(passphrase_ptr, passphrase_len, "passphrase") {
        Ok(value) => value,
        Err(error) => return error_result(&error),
    };

    match decrypt_mobile_vault_export_to_json(encrypted_json, passphrase) {
        Ok(json) => success_result(json.into_bytes()),
        Err(error) => error_result(&error.to_string()),
    }
}

fn read_utf8<'a>(ptr: *const u8, len: usize, label: &str) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err(format!("TermiRust mobile {label} pointer was null."));
    }

    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    str::from_utf8(bytes).map_err(|_| format!("TermiRust mobile {label} was not valid UTF-8."))
}

fn success_result(bytes: Vec<u8>) -> TermiRustMobileResult {
    TermiRustMobileResult {
        ok: true,
        data: into_buffer(bytes),
        error: empty_buffer(),
    }
}

fn error_result(message: &str) -> TermiRustMobileResult {
    TermiRustMobileResult {
        ok: false,
        data: empty_buffer(),
        error: into_buffer(message.as_bytes().to_vec()),
    }
}

fn into_buffer(bytes: Vec<u8>) -> TermiRustMobileByteBuffer {
    if bytes.is_empty() {
        return empty_buffer();
    }

    let len = bytes.len();
    let mut boxed = bytes.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    std::mem::forget(boxed);
    TermiRustMobileByteBuffer { ptr, len }
}

fn empty_buffer() -> TermiRustMobileByteBuffer {
    TermiRustMobileByteBuffer {
        ptr: std::ptr::null_mut(),
        len: 0,
    }
}

#[cfg(target_os = "android")]
mod android_jni {
    use jni::JNIEnv;
    use jni::objects::{JByteArray, JClass};
    use jni::sys::jbyteArray;
    use std::ptr;
    use std::str;
    use termirust_protocol::decrypt_mobile_vault_export_to_json;

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_data_NativeMobileVaultCrypto_decryptVaultJson(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        encrypted_json: JByteArray<'_>,
        passphrase: JByteArray<'_>,
    ) -> jbyteArray {
        match decrypt(&mut env, encrypted_json, passphrase) {
            Ok(bytes) => match env.byte_array_from_slice(bytes.as_bytes()) {
                Ok(array) => array.into_raw(),
                Err(error) => {
                    throw(
                        &mut env,
                        "java/lang/IllegalStateException",
                        &error.to_string(),
                    );
                    ptr::null_mut()
                }
            },
            Err(error) => {
                throw(&mut env, "java/lang/IllegalArgumentException", &error);
                ptr::null_mut()
            }
        }
    }

    fn decrypt(
        env: &mut JNIEnv<'_>,
        encrypted_json: JByteArray<'_>,
        passphrase: JByteArray<'_>,
    ) -> Result<String, String> {
        let encrypted_json = env
            .convert_byte_array(encrypted_json)
            .map_err(|error| error.to_string())?;
        let passphrase = env
            .convert_byte_array(passphrase)
            .map_err(|error| error.to_string())?;

        let encrypted_json = str::from_utf8(&encrypted_json)
            .map_err(|_| "Encrypted mobile vault JSON was not valid UTF-8.".to_string())?;
        let passphrase = str::from_utf8(&passphrase)
            .map_err(|_| "Mobile vault passphrase was not valid UTF-8.".to_string())?;

        decrypt_mobile_vault_export_to_json(encrypted_json, passphrase)
            .map_err(|error| error.to_string())
    }

    fn throw(env: &mut JNIEnv<'_>, class: &str, message: &str) {
        let _ = env.throw_new(class, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::slice;
    use std::str;
    use termirust_protocol::{MobileVaultExport, encrypt_mobile_vault_export_json};

    #[test]
    fn ffi_decrypts_mobile_vault_json() {
        let export = MobileVaultExport::empty("export-1", "desktop-1");
        let encrypted =
            encrypt_mobile_vault_export_json(&export, "hunter2").expect("encrypt test vault");
        let passphrase = "hunter2";

        let result = termirust_mobile_decrypt_vault_json(
            encrypted.as_ptr(),
            encrypted.len(),
            passphrase.as_ptr(),
            passphrase.len(),
        );

        assert!(result.ok);
        let decrypted = buffer_to_str(result.data);
        assert!(decrypted.contains("\"export_id\": \"export-1\""));
        assert!(decrypted.contains("\"source_device_id\": \"desktop-1\""));

        termirust_mobile_free_result(result);
    }

    #[test]
    fn ffi_reports_decrypt_errors() {
        let encrypted = "{}";
        let passphrase = "hunter2";

        let result = termirust_mobile_decrypt_vault_json(
            encrypted.as_ptr(),
            encrypted.len(),
            passphrase.as_ptr(),
            passphrase.len(),
        );

        assert!(!result.ok);
        let error = buffer_to_str(result.error);
        assert!(error.contains("missing field"));

        termirust_mobile_free_result(result);
    }

    #[test]
    fn ffi_reports_null_pointer_errors() {
        let passphrase = "hunter2";

        let result = termirust_mobile_decrypt_vault_json(
            std::ptr::null(),
            0,
            passphrase.as_ptr(),
            passphrase.len(),
        );

        assert!(!result.ok);
        assert_eq!(
            buffer_to_str(result.error),
            "TermiRust mobile vault JSON pointer was null."
        );

        termirust_mobile_free_result(result);
    }

    fn buffer_to_str(buffer: TermiRustMobileByteBuffer) -> &'static str {
        let bytes = unsafe { slice::from_raw_parts(buffer.ptr, buffer.len) };
        str::from_utf8(bytes).expect("valid UTF-8")
    }
}
