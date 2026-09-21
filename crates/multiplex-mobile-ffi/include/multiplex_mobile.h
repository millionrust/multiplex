#ifndef MULTIPLEX_MOBILE_H
#define MULTIPLEX_MOBILE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MultiplexMobileByteBuffer {
  uint8_t *ptr;
  size_t len;
} MultiplexMobileByteBuffer;

typedef struct MultiplexMobileResult {
  bool ok;
  MultiplexMobileByteBuffer data;
  MultiplexMobileByteBuffer error;
} MultiplexMobileResult;

typedef struct MultiplexMobileTerminal MultiplexMobileTerminal;

MultiplexMobileResult multiplex_mobile_decrypt_vault_json(
    const uint8_t *encrypted_json_ptr,
    size_t encrypted_json_len,
    const uint8_t *passphrase_ptr,
    size_t passphrase_len);

MultiplexMobileResult multiplex_mobile_render_terminal_utf8(
    const uint8_t *input_ptr,
    size_t input_len,
    uint16_t columns,
    uint16_t rows,
    size_t scrollback_rows);

MultiplexMobileResult multiplex_mobile_relay_client_hello(
    const uint8_t *route_id_ptr,
    size_t route_id_len);

MultiplexMobileResult multiplex_mobile_relay_admission_proof(
    const uint8_t *route_id_ptr,
    size_t route_id_len,
    const uint8_t *credential_ptr,
    size_t credential_len,
    uint64_t revocation_epoch,
    uint64_t now_unix_seconds,
    const uint8_t *challenge_ptr,
    size_t challenge_len);

MultiplexMobileResult multiplex_mobile_relay_admission_connection_id(
    const uint8_t *result_ptr,
    size_t result_len);

MultiplexMobileResult multiplex_mobile_relay_encode_envelope(
    const uint8_t *route_id_ptr,
    size_t route_id_len,
    uint64_t sequence,
    const uint8_t *payload_ptr,
    size_t payload_len);

MultiplexMobileResult multiplex_mobile_relay_decode_envelope(
    const uint8_t *route_id_ptr,
    size_t route_id_len,
    uint64_t expected_sequence,
    const uint8_t *envelope_ptr,
    size_t envelope_len);

MultiplexMobileTerminal *multiplex_mobile_terminal_create(
    uint16_t columns,
    uint16_t rows,
    size_t scrollback_rows);

MultiplexMobileResult multiplex_mobile_terminal_process(
    MultiplexMobileTerminal *terminal,
    const uint8_t *input_ptr,
    size_t input_len);

bool multiplex_mobile_terminal_feed(
    MultiplexMobileTerminal *terminal,
    const uint8_t *input_ptr,
    size_t input_len);

MultiplexMobileResult multiplex_mobile_terminal_resize(
    MultiplexMobileTerminal *terminal,
    uint16_t columns,
    uint16_t rows);

MultiplexMobileResult multiplex_mobile_terminal_snapshot(
    MultiplexMobileTerminal *terminal);

void multiplex_mobile_terminal_destroy(MultiplexMobileTerminal *terminal);

void multiplex_mobile_free_result(MultiplexMobileResult result);

void multiplex_mobile_free_buffer(MultiplexMobileByteBuffer buffer);

#ifdef __cplusplus
}
#endif

#endif
