#ifndef TERMIRUST_MOBILE_H
#define TERMIRUST_MOBILE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TermiRustMobileByteBuffer {
  uint8_t *ptr;
  size_t len;
} TermiRustMobileByteBuffer;

typedef struct TermiRustMobileResult {
  bool ok;
  TermiRustMobileByteBuffer data;
  TermiRustMobileByteBuffer error;
} TermiRustMobileResult;

TermiRustMobileResult termirust_mobile_decrypt_vault_json(
    const uint8_t *encrypted_json_ptr,
    size_t encrypted_json_len,
    const uint8_t *passphrase_ptr,
    size_t passphrase_len);

TermiRustMobileResult termirust_mobile_render_terminal_utf8(
    const uint8_t *input_ptr,
    size_t input_len,
    uint16_t columns,
    uint16_t rows,
    size_t scrollback_rows);

void termirust_mobile_free_result(TermiRustMobileResult result);

void termirust_mobile_free_buffer(TermiRustMobileByteBuffer buffer);

#ifdef __cplusplus
}
#endif

#endif
