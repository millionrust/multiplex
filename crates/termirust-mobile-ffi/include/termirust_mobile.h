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

typedef struct TermiRustMobileTerminal TermiRustMobileTerminal;

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

TermiRustMobileResult termirust_mobile_relay_client_hello(
    const uint8_t *route_id_ptr,
    size_t route_id_len);

TermiRustMobileResult termirust_mobile_relay_admission_proof(
    const uint8_t *route_id_ptr,
    size_t route_id_len,
    const uint8_t *credential_ptr,
    size_t credential_len,
    uint64_t revocation_epoch,
    uint64_t now_unix_seconds,
    const uint8_t *challenge_ptr,
    size_t challenge_len);

TermiRustMobileResult termirust_mobile_relay_admission_connection_id(
    const uint8_t *result_ptr,
    size_t result_len);

TermiRustMobileResult termirust_mobile_relay_encode_envelope(
    const uint8_t *route_id_ptr,
    size_t route_id_len,
    uint64_t sequence,
    const uint8_t *payload_ptr,
    size_t payload_len);

TermiRustMobileResult termirust_mobile_relay_decode_envelope(
    const uint8_t *route_id_ptr,
    size_t route_id_len,
    uint64_t expected_sequence,
    const uint8_t *envelope_ptr,
    size_t envelope_len);

TermiRustMobileTerminal *termirust_mobile_terminal_create(
    uint16_t columns,
    uint16_t rows,
    size_t scrollback_rows);

TermiRustMobileResult termirust_mobile_terminal_process(
    TermiRustMobileTerminal *terminal,
    const uint8_t *input_ptr,
    size_t input_len);

bool termirust_mobile_terminal_feed(
    TermiRustMobileTerminal *terminal,
    const uint8_t *input_ptr,
    size_t input_len);

TermiRustMobileResult termirust_mobile_terminal_resize(
    TermiRustMobileTerminal *terminal,
    uint16_t columns,
    uint16_t rows);

TermiRustMobileResult termirust_mobile_terminal_snapshot(
    TermiRustMobileTerminal *terminal);

void termirust_mobile_terminal_destroy(TermiRustMobileTerminal *terminal);

void termirust_mobile_free_result(TermiRustMobileResult result);

void termirust_mobile_free_buffer(TermiRustMobileByteBuffer buffer);

#ifdef __cplusplus
}
#endif

#endif
