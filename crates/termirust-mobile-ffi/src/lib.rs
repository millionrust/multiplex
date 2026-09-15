use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;
use std::str;
use termirust_protocol::decrypt_mobile_vault_export_to_json;
use vt100::Parser;

mod relay;
mod terminal;

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
pub extern "C" fn termirust_mobile_render_terminal_utf8(
    input_ptr: *const u8,
    input_len: usize,
    columns: u16,
    rows: u16,
    scrollback_rows: usize,
) -> TermiRustMobileResult {
    match catch_unwind(AssertUnwindSafe(|| {
        render_terminal_utf8(input_ptr, input_len, columns, rows, scrollback_rows)
    })) {
        Ok(result) => result,
        Err(_) => error_result("TermiRust mobile terminal renderer panicked."),
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

fn render_terminal_utf8(
    input_ptr: *const u8,
    input_len: usize,
    columns: u16,
    rows: u16,
    scrollback_rows: usize,
) -> TermiRustMobileResult {
    let input = match read_bytes(input_ptr, input_len, "terminal input") {
        Ok(value) => value,
        Err(error) => return error_result(&error),
    };

    let columns = columns.max(1);
    let rows = rows.max(1);
    let mut parser = Parser::new(rows, columns, scrollback_rows);
    parser.process(input);

    let mut lines = terminal_rows_text(&parser);
    if lines.len() > scrollback_rows {
        lines = lines.split_off(lines.len() - scrollback_rows);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) && lines.len() > 1 {
        lines.pop();
    }

    success_result(lines.join("\n").into_bytes())
}

fn terminal_rows_text(parser: &Parser) -> Vec<String> {
    let screen = parser.screen().clone();
    let (rows, cols) = screen.size();
    let viewport_rows = usize::from(rows.max(1));
    let max_scrollback = {
        let mut top = screen.clone();
        top.set_scrollback(usize::MAX);
        top.scrollback()
    };
    let mut all_rows = Vec::with_capacity(max_scrollback + viewport_rows);

    let full_pages = max_scrollback / viewport_rows;
    let remainder = max_scrollback % viewport_rows;

    for page in 0..full_pages {
        let mut view = screen.clone();
        view.set_scrollback(max_scrollback - page * viewport_rows);
        all_rows.extend(view.rows(0, cols));
    }

    if remainder > 0 {
        let mut view = screen.clone();
        view.set_scrollback(remainder);
        all_rows.extend(view.rows(0, cols).take(remainder));
    }

    let mut view = screen;
    view.set_scrollback(0);
    all_rows.extend(view.rows(0, cols));
    all_rows
}

fn read_utf8<'a>(ptr: *const u8, len: usize, label: &str) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err(format!("TermiRust mobile {label} pointer was null."));
    }

    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    str::from_utf8(bytes).map_err(|_| format!("TermiRust mobile {label} was not valid UTF-8."))
}

pub(crate) fn read_bytes<'a>(ptr: *const u8, len: usize, label: &str) -> Result<&'a [u8], String> {
    if ptr.is_null() {
        if len == 0 {
            return Ok(&[]);
        }
        return Err(format!("TermiRust mobile {label} pointer was null."));
    }

    Ok(unsafe { slice::from_raw_parts(ptr, len) })
}

pub(crate) fn success_result(bytes: Vec<u8>) -> TermiRustMobileResult {
    TermiRustMobileResult {
        ok: true,
        data: into_buffer(bytes),
        error: empty_buffer(),
    }
}

pub(crate) fn error_result(message: &str) -> TermiRustMobileResult {
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
    use crate::TermiRustMobileResult;
    use jni::JNIEnv;
    use jni::objects::{JByteArray, JClass};
    use jni::sys::{jboolean, jbyteArray, jlong};
    use std::ptr;
    use std::slice;
    use std::str;
    use termirust_protocol::decrypt_mobile_vault_export_to_json;
    use vt100::Parser;

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

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_terminal_NativeMobileTerminal_renderUtf8(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        input: JByteArray<'_>,
        columns: i32,
        rows: i32,
        scrollback_rows: i32,
    ) -> jbyteArray {
        match render(&mut env, input, columns, rows, scrollback_rows) {
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

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeControllerTerminal_create(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        columns: i32,
        rows: i32,
        scrollback_rows: i32,
    ) -> jlong {
        let dimensions = terminal_dimensions(columns, rows, scrollback_rows);
        let Ok((columns, rows, scrollback_rows)) = dimensions else {
            throw(
                &mut env,
                "java/lang/IllegalArgumentException",
                &dimensions.unwrap_err(),
            );
            return 0;
        };
        super::terminal::termirust_mobile_terminal_create(columns, rows, scrollback_rows) as jlong
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeControllerTerminal_process(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        handle: jlong,
        input: JByteArray<'_>,
    ) -> jbyteArray {
        let input = match env.convert_byte_array(input) {
            Ok(input) => input,
            Err(error) => {
                throw(
                    &mut env,
                    "java/lang/IllegalArgumentException",
                    &error.to_string(),
                );
                return ptr::null_mut();
            }
        };
        let result = super::terminal::termirust_mobile_terminal_process(
            terminal_handle(handle),
            input.as_ptr(),
            input.len(),
        );
        mobile_result_to_java(&mut env, result)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeControllerTerminal_feed(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        handle: jlong,
        input: JByteArray<'_>,
    ) -> jboolean {
        let input = match env.convert_byte_array(input) {
            Ok(input) => input,
            Err(error) => {
                throw(
                    &mut env,
                    "java/lang/IllegalArgumentException",
                    &error.to_string(),
                );
                return 0;
            }
        };
        super::terminal::termirust_mobile_terminal_feed(
            terminal_handle(handle),
            input.as_ptr(),
            input.len(),
        ) as jboolean
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeControllerTerminal_resize(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        handle: jlong,
        columns: i32,
        rows: i32,
    ) -> jbyteArray {
        let Ok(columns) = u16::try_from(columns) else {
            throw(
                &mut env,
                "java/lang/IllegalArgumentException",
                "Invalid terminal columns.",
            );
            return ptr::null_mut();
        };
        let Ok(rows) = u16::try_from(rows) else {
            throw(
                &mut env,
                "java/lang/IllegalArgumentException",
                "Invalid terminal rows.",
            );
            return ptr::null_mut();
        };
        let result = super::terminal::termirust_mobile_terminal_resize(
            terminal_handle(handle),
            columns,
            rows,
        );
        mobile_result_to_java(&mut env, result)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeControllerTerminal_snapshot(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        handle: jlong,
    ) -> jbyteArray {
        let result = super::terminal::termirust_mobile_terminal_snapshot(terminal_handle(handle));
        mobile_result_to_java(&mut env, result)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeControllerTerminal_destroy(
        _env: JNIEnv<'_>,
        _class: JClass<'_>,
        handle: jlong,
    ) {
        super::terminal::termirust_mobile_terminal_destroy(terminal_handle(handle));
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeRelayProtocol_clientHello(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        route_id: JByteArray<'_>,
    ) -> jbyteArray {
        let Ok(route_id) = java_bytes(&mut env, route_id) else {
            return ptr::null_mut();
        };
        mobile_result_to_java(
            &mut env,
            super::relay::ffi_result(|| {
                super::relay::client_hello(route_id.as_ptr(), route_id.len())
            }),
        )
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeRelayProtocol_admissionProof(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        route_id: JByteArray<'_>,
        credential: JByteArray<'_>,
        revocation_epoch: jlong,
        now_unix_seconds: jlong,
        challenge: JByteArray<'_>,
    ) -> jbyteArray {
        let (Ok(route_id), Ok(credential), Ok(challenge)) = (
            java_bytes(&mut env, route_id),
            java_bytes(&mut env, credential),
            java_bytes(&mut env, challenge),
        ) else {
            return ptr::null_mut();
        };
        let (Ok(revocation_epoch), Ok(now_unix_seconds)) = (
            u64::try_from(revocation_epoch),
            u64::try_from(now_unix_seconds),
        ) else {
            throw(
                &mut env,
                "java/lang/IllegalArgumentException",
                "Relay epoch and time must be non-negative.",
            );
            return ptr::null_mut();
        };
        mobile_result_to_java(
            &mut env,
            super::relay::ffi_result(|| {
                super::relay::admission_proof(
                    route_id.as_ptr(),
                    route_id.len(),
                    credential.as_ptr(),
                    credential.len(),
                    revocation_epoch,
                    now_unix_seconds,
                    challenge.as_ptr(),
                    challenge.len(),
                )
            }),
        )
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeRelayProtocol_admissionConnectionId(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        result: JByteArray<'_>,
    ) -> jbyteArray {
        let Ok(result) = java_bytes(&mut env, result) else {
            return ptr::null_mut();
        };
        mobile_result_to_java(
            &mut env,
            super::relay::ffi_result(|| {
                super::relay::admission_connection_id(result.as_ptr(), result.len())
            }),
        )
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeRelayProtocol_encodeEnvelope(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        route_id: JByteArray<'_>,
        sequence: jlong,
        payload: JByteArray<'_>,
    ) -> jbyteArray {
        let (Ok(route_id), Ok(payload), Ok(sequence)) = (
            java_bytes(&mut env, route_id),
            java_bytes(&mut env, payload),
            u64::try_from(sequence),
        ) else {
            throw(
                &mut env,
                "java/lang/IllegalArgumentException",
                "Relay envelope input was invalid.",
            );
            return ptr::null_mut();
        };
        mobile_result_to_java(
            &mut env,
            super::relay::ffi_result(|| {
                super::relay::encode_envelope(
                    route_id.as_ptr(),
                    route_id.len(),
                    sequence,
                    payload.as_ptr(),
                    payload.len(),
                )
            }),
        )
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_termirust_mobile_controller_NativeRelayProtocol_decodeEnvelope(
        mut env: JNIEnv<'_>,
        _class: JClass<'_>,
        route_id: JByteArray<'_>,
        expected_sequence: jlong,
        envelope: JByteArray<'_>,
    ) -> jbyteArray {
        let (Ok(route_id), Ok(envelope), Ok(expected_sequence)) = (
            java_bytes(&mut env, route_id),
            java_bytes(&mut env, envelope),
            u64::try_from(expected_sequence),
        ) else {
            throw(
                &mut env,
                "java/lang/IllegalArgumentException",
                "Relay envelope input was invalid.",
            );
            return ptr::null_mut();
        };
        mobile_result_to_java(
            &mut env,
            super::relay::ffi_result(|| {
                super::relay::decode_envelope(
                    route_id.as_ptr(),
                    route_id.len(),
                    expected_sequence,
                    envelope.as_ptr(),
                    envelope.len(),
                )
            }),
        )
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

    fn render(
        env: &mut JNIEnv<'_>,
        input: JByteArray<'_>,
        columns: i32,
        rows: i32,
        scrollback_rows: i32,
    ) -> Result<String, String> {
        let input = env
            .convert_byte_array(input)
            .map_err(|error| error.to_string())?;
        let columns = u16::try_from(columns.max(1)).unwrap_or(u16::MAX);
        let rows = u16::try_from(rows.max(1)).unwrap_or(u16::MAX);
        let scrollback_rows = usize::try_from(scrollback_rows.max(1)).unwrap_or(2_000);

        let mut parser = Parser::new(rows, columns, scrollback_rows);
        parser.process(&input);
        let mut lines = super::terminal_rows_text(&parser);
        if lines.len() > scrollback_rows {
            lines = lines.split_off(lines.len() - scrollback_rows);
        }
        while lines.last().is_some_and(|line| line.trim().is_empty()) && lines.len() > 1 {
            lines.pop();
        }
        Ok(lines.join("\n"))
    }

    fn terminal_dimensions(
        columns: i32,
        rows: i32,
        scrollback_rows: i32,
    ) -> Result<(u16, u16, usize), String> {
        let columns =
            u16::try_from(columns).map_err(|_| "Invalid terminal columns.".to_string())?;
        let rows = u16::try_from(rows).map_err(|_| "Invalid terminal rows.".to_string())?;
        let scrollback_rows = usize::try_from(scrollback_rows)
            .map_err(|_| "Invalid terminal scrollback.".to_string())?;
        Ok((columns, rows, scrollback_rows))
    }

    fn java_bytes(env: &mut JNIEnv<'_>, value: JByteArray<'_>) -> Result<Vec<u8>, ()> {
        env.convert_byte_array(value).map_err(|error| {
            throw(
                env,
                "java/lang/IllegalArgumentException",
                &error.to_string(),
            );
        })
    }

    fn terminal_handle(handle: jlong) -> *mut super::terminal::TermiRustMobileTerminal {
        handle as usize as *mut super::terminal::TermiRustMobileTerminal
    }

    fn mobile_result_to_java(env: &mut JNIEnv<'_>, result: TermiRustMobileResult) -> jbyteArray {
        let response = if result.ok {
            let bytes = unsafe { slice::from_raw_parts(result.data.ptr, result.data.len) };
            env.byte_array_from_slice(bytes)
                .map_err(|error| error.to_string())
        } else {
            let bytes = unsafe { slice::from_raw_parts(result.error.ptr, result.error.len) };
            let error = String::from_utf8_lossy(bytes).into_owned();
            Err(error)
        };
        super::termirust_mobile_free_result(result);
        match response {
            Ok(array) => array.into_raw(),
            Err(error) => {
                throw(env, "java/lang/IllegalStateException", &error);
                ptr::null_mut()
            }
        }
    }

    fn throw(env: &mut JNIEnv<'_>, class: &str, message: &str) {
        let _ = env.throw_new(class, message);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_relay_client_hello(
    route_id_ptr: *const u8,
    route_id_len: usize,
) -> TermiRustMobileResult {
    relay::ffi_result(|| relay::client_hello(route_id_ptr, route_id_len))
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_relay_admission_proof(
    route_id_ptr: *const u8,
    route_id_len: usize,
    credential_ptr: *const u8,
    credential_len: usize,
    revocation_epoch: u64,
    now_unix_seconds: u64,
    challenge_ptr: *const u8,
    challenge_len: usize,
) -> TermiRustMobileResult {
    relay::ffi_result(|| {
        relay::admission_proof(
            route_id_ptr,
            route_id_len,
            credential_ptr,
            credential_len,
            revocation_epoch,
            now_unix_seconds,
            challenge_ptr,
            challenge_len,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_relay_admission_connection_id(
    result_ptr: *const u8,
    result_len: usize,
) -> TermiRustMobileResult {
    relay::ffi_result(|| relay::admission_connection_id(result_ptr, result_len))
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_relay_encode_envelope(
    route_id_ptr: *const u8,
    route_id_len: usize,
    sequence: u64,
    payload_ptr: *const u8,
    payload_len: usize,
) -> TermiRustMobileResult {
    relay::ffi_result(|| {
        relay::encode_envelope(
            route_id_ptr,
            route_id_len,
            sequence,
            payload_ptr,
            payload_len,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn termirust_mobile_relay_decode_envelope(
    route_id_ptr: *const u8,
    route_id_len: usize,
    expected_sequence: u64,
    envelope_ptr: *const u8,
    envelope_len: usize,
) -> TermiRustMobileResult {
    relay::ffi_result(|| {
        relay::decode_envelope(
            route_id_ptr,
            route_id_len,
            expected_sequence,
            envelope_ptr,
            envelope_len,
        )
    })
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

    #[test]
    fn ffi_renders_terminal_with_vt_sequences() {
        let input = b"progress 1\rprogress 2\r\n\x1b[31mred\x1b[0m\r\nabc\x08Z";

        let result =
            termirust_mobile_render_terminal_utf8(input.as_ptr(), input.len(), 80, 24, 2_000);

        assert!(result.ok);
        assert_eq!(buffer_to_str(result.data), "progress 2\nred\nabZ");

        termirust_mobile_free_result(result);
    }

    fn buffer_to_str(buffer: TermiRustMobileByteBuffer) -> &'static str {
        let bytes = unsafe { slice::from_raw_parts(buffer.ptr, buffer.len) };
        str::from_utf8(bytes).expect("valid UTF-8")
    }
}
