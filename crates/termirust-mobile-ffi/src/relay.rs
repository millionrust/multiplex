use crate::{TermiRustMobileResult, error_result, read_bytes, success_result};
use std::panic::{AssertUnwindSafe, catch_unwind};
use termirust_relay_protocol::{
    ADMISSION_LIFETIME_SECONDS, RelayAdmissionChallenge, RelayAdmissionCredential,
    RelayAdmissionResult, RelayClientHello, RelayConnectionSequence, RelayDiagnosticCode,
    RelayDirection, RelayEndpointRole, RelayEnvelopeV1, RelayRevocationEpoch, RelayRouteId,
};

const ROUTE_BYTES: usize = 32;
const CREDENTIAL_BYTES: usize = 32;

pub(crate) fn ffi_result(
    operation: impl FnOnce() -> Result<Vec<u8>, String>,
) -> TermiRustMobileResult {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(bytes)) => success_result(bytes),
        Ok(Err(error)) => error_result(&error),
        Err(_) => error_result("TermiRust mobile relay protocol panicked."),
    }
}

pub(crate) fn client_hello(
    route_id_ptr: *const u8,
    route_id_len: usize,
) -> Result<Vec<u8>, String> {
    Ok(RelayClientHello {
        route_id: route_id(route_id_ptr, route_id_len)?,
        role: RelayEndpointRole::Controller,
    }
    .encode()
    .to_vec())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn admission_proof(
    route_id_ptr: *const u8,
    route_id_len: usize,
    credential_ptr: *const u8,
    credential_len: usize,
    revocation_epoch: u64,
    now_unix_seconds: u64,
    challenge_ptr: *const u8,
    challenge_len: usize,
) -> Result<Vec<u8>, String> {
    let route_id = route_id(route_id_ptr, route_id_len)?;
    let secret =
        fixed_bytes::<CREDENTIAL_BYTES>(credential_ptr, credential_len, "relay credential")?;
    let credential = RelayAdmissionCredential::from_secret_bytes(secret);
    let challenge = RelayAdmissionChallenge::decode(read_bytes(
        challenge_ptr,
        challenge_len,
        "relay challenge",
    )?)
    .map_err(|_| "TermiRust mobile relay challenge was malformed.".to_owned())?;
    if challenge.route_id != route_id
        || challenge.role != RelayEndpointRole::Controller
        || challenge.verifier != credential.verifier()
        || challenge.revocation_epoch != RelayRevocationEpoch(revocation_epoch)
        || challenge.expires_at_unix_seconds < now_unix_seconds
        || challenge
            .expires_at_unix_seconds
            .saturating_sub(now_unix_seconds)
            > ADMISSION_LIFETIME_SECONDS
    {
        return Err("TermiRust mobile relay challenge did not match this route.".to_owned());
    }
    Ok(credential.prove(&challenge).encode().to_vec())
}

pub(crate) fn admission_connection_id(
    result_ptr: *const u8,
    result_len: usize,
) -> Result<Vec<u8>, String> {
    let result = RelayAdmissionResult::decode(read_bytes(
        result_ptr,
        result_len,
        "relay admission result",
    )?)
    .map_err(|_| "TermiRust mobile relay admission result was malformed.".to_owned())?;
    if result.diagnostic != RelayDiagnosticCode::Ready {
        return Err(format!(
            "TermiRust mobile relay admission failed: {}.",
            result.diagnostic.as_str()
        ));
    }
    let connection_id = result
        .connection_id
        .ok_or_else(|| "TermiRust mobile relay admission omitted its connection ID.".to_owned())?;
    Ok(connection_id.0.to_be_bytes().to_vec())
}

pub(crate) fn encode_envelope(
    route_id_ptr: *const u8,
    route_id_len: usize,
    sequence: u64,
    payload_ptr: *const u8,
    payload_len: usize,
) -> Result<Vec<u8>, String> {
    let payload = read_bytes(payload_ptr, payload_len, "relay payload")?.to_vec();
    RelayEnvelopeV1::new(
        route_id(route_id_ptr, route_id_len)?,
        RelayDirection::ControllerToHost,
        RelayConnectionSequence(sequence),
        payload,
    )
    .map(|envelope| envelope.encode())
    .map_err(|_| "TermiRust mobile relay payload exceeded its bound.".to_owned())
}

pub(crate) fn decode_envelope(
    route_id_ptr: *const u8,
    route_id_len: usize,
    expected_sequence: u64,
    envelope_ptr: *const u8,
    envelope_len: usize,
) -> Result<Vec<u8>, String> {
    let route_id = route_id(route_id_ptr, route_id_len)?;
    let envelope =
        RelayEnvelopeV1::decode(read_bytes(envelope_ptr, envelope_len, "relay envelope")?)
            .map_err(|_| "TermiRust mobile relay envelope was malformed.".to_owned())?;
    if envelope.route_id() != route_id
        || envelope.direction() != RelayDirection::HostToController
        || envelope.sequence() != RelayConnectionSequence(expected_sequence)
    {
        return Err("TermiRust mobile relay envelope route or sequence did not match.".to_owned());
    }
    Ok(envelope.ciphertext().to_vec())
}

fn route_id(ptr: *const u8, len: usize) -> Result<RelayRouteId, String> {
    Ok(RelayRouteId(fixed_bytes::<ROUTE_BYTES>(
        ptr,
        len,
        "relay route ID",
    )?))
}

fn fixed_bytes<const N: usize>(ptr: *const u8, len: usize, label: &str) -> Result<[u8; N], String> {
    let bytes = read_bytes(ptr, len, label)?;
    bytes
        .try_into()
        .map_err(|_| format!("TermiRust mobile {label} must contain exactly {N} bytes."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_relay_protocol::{RelayAdmissionChallenge, RelayCredentialVerifier};

    #[test]
    fn canonical_controller_material_and_envelopes_are_strict() {
        let route = [0x22_u8; 32];
        let secret = [0x11_u8; 32];
        let credential = RelayAdmissionCredential::from_secret_bytes(secret);
        let challenge = RelayAdmissionChallenge {
            route_id: RelayRouteId(route),
            role: RelayEndpointRole::Controller,
            verifier: credential.verifier(),
            revocation_epoch: RelayRevocationEpoch(7),
            serial: 9,
            expires_at_unix_seconds: 1_234,
            nonce: [0x33; 32],
        };
        let proof = admission_proof(
            route.as_ptr(),
            route.len(),
            secret.as_ptr(),
            secret.len(),
            7,
            1_230,
            challenge.encode().as_ptr(),
            challenge.encode().len(),
        )
        .unwrap();
        assert_eq!(proof, credential.prove(&challenge).encode());
        assert_eq!(client_hello(route.as_ptr(), route.len()).unwrap()[6], 2);

        let outbound_payload = [0x70, 0x71, 0x72];
        let outbound = encode_envelope(
            route.as_ptr(),
            route.len(),
            4,
            outbound_payload.as_ptr(),
            outbound_payload.len(),
        )
        .unwrap();
        let canonical_outbound = RelayEnvelopeV1::new(
            RelayRouteId(route),
            RelayDirection::ControllerToHost,
            RelayConnectionSequence(4),
            outbound_payload.to_vec(),
        )
        .unwrap()
        .encode();
        assert_eq!(outbound, canonical_outbound);

        let inbound = RelayEnvelopeV1::new(
            RelayRouteId(route),
            RelayDirection::HostToController,
            RelayConnectionSequence(5),
            vec![0x44, 0x55, 0x66],
        )
        .unwrap()
        .encode();
        assert_eq!(
            decode_envelope(
                route.as_ptr(),
                route.len(),
                5,
                inbound.as_ptr(),
                inbound.len()
            )
            .unwrap(),
            [0x44, 0x55, 0x66]
        );
        assert!(
            decode_envelope(
                route.as_ptr(),
                route.len(),
                4,
                inbound.as_ptr(),
                inbound.len()
            )
            .is_err()
        );

        let wrong = RelayAdmissionChallenge {
            verifier: RelayCredentialVerifier([9; 32]),
            ..challenge
        };
        assert!(
            admission_proof(
                route.as_ptr(),
                route.len(),
                secret.as_ptr(),
                secret.len(),
                7,
                1_230,
                wrong.encode().as_ptr(),
                wrong.encode().len()
            )
            .is_err()
        );
    }
}
