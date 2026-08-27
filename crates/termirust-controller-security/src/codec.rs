use crate::error::{ErrorCode, Result};
use crate::types::{
    CapabilitySet, ControllerProtocolVersion, DeviceStaticPublicKey, HostStaticPublicKey,
    PairingNonce, PairingOfferCore, PairingRole, PairingStep,
};

const OFFER_MAGIC: [u8; 4] = *b"TCO1";
const OFFER_SUITE_XX_25519_CHACHAPOLY_BLAKE2S: u8 = 1;
pub const PAIRING_OFFER_BYTES: usize = 84;
pub(crate) const PAIRING_PAYLOAD_BYTES: usize = 110;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PairingPayload {
    pub step: PairingStep,
    pub role: PairingRole,
    pub version: ControllerProtocolVersion,
    pub nonce: PairingNonce,
    pub host_key: HostStaticPublicKey,
    pub device_key: DeviceStaticPublicKey,
    pub capabilities: CapabilitySet,
}

pub fn encode_offer(offer: &PairingOfferCore) -> Result<[u8; PAIRING_OFFER_BYTES]> {
    offer.version.require_v1()?;
    let mut bytes = [0_u8; PAIRING_OFFER_BYTES];
    bytes[..4].copy_from_slice(&OFFER_MAGIC);
    bytes[4..6].copy_from_slice(&offer.version.major.to_be_bytes());
    bytes[6..8].copy_from_slice(&offer.version.minor.to_be_bytes());
    bytes[8] = OFFER_SUITE_XX_25519_CHACHAPOLY_BLAKE2S;
    bytes[9] = 0;
    bytes[10..18].copy_from_slice(&offer.expires_at_unix_seconds.to_be_bytes());
    bytes[18..50].copy_from_slice(&offer.nonce.0);
    bytes[50..82].copy_from_slice(&offer.host_static_public_key.0);
    bytes[82..84].copy_from_slice(&offer.capabilities.bits().to_be_bytes());
    Ok(bytes)
}

pub fn decode_offer(bytes: &[u8]) -> Result<PairingOfferCore> {
    if bytes.len() < 8 {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    if bytes[..4] != OFFER_MAGIC {
        return Err(ErrorCode::InvalidMagic.into());
    }
    let version = ControllerProtocolVersion {
        major: read_u16(bytes, 4)?,
        minor: read_u16(bytes, 6)?,
    };
    version.require_v1()?;
    if bytes.len() != PAIRING_OFFER_BYTES {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    if bytes[8] != OFFER_SUITE_XX_25519_CHACHAPOLY_BLAKE2S {
        return Err(ErrorCode::UnsupportedSuite.into());
    }
    if bytes[9] != 0 {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    Ok(PairingOfferCore {
        version,
        expires_at_unix_seconds: read_u64(bytes, 10)?,
        nonce: PairingNonce(read_array(bytes, 18)?),
        host_static_public_key: HostStaticPublicKey(read_array(bytes, 50)?),
        capabilities: CapabilitySet::from_bits(read_u16(bytes, 82)?)?,
    })
}

pub fn pairing_prologue(offer: &PairingOfferCore) -> Result<Vec<u8>> {
    let encoded = encode_offer(offer)?;
    let mut bytes = Vec::with_capacity(30 + encoded.len());
    bytes.extend_from_slice(b"termirust-controller-v1\0");
    bytes.extend_from_slice(&(PAIRING_OFFER_BYTES as u16).to_be_bytes());
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

pub(crate) fn encode_pairing_payload(
    payload: &PairingPayload,
) -> Result<[u8; PAIRING_PAYLOAD_BYTES]> {
    payload.version.require_v1()?;
    let mut bytes = [0_u8; PAIRING_PAYLOAD_BYTES];
    bytes[..4].copy_from_slice(b"TPS1");
    bytes[4] = payload.step as u8;
    bytes[5] = payload.role as u8;
    bytes[6..8].copy_from_slice(&0_u16.to_be_bytes());
    bytes[8..10].copy_from_slice(&payload.version.major.to_be_bytes());
    bytes[10..12].copy_from_slice(&payload.version.minor.to_be_bytes());
    bytes[12..44].copy_from_slice(&payload.nonce.0);
    bytes[44..76].copy_from_slice(&payload.host_key.0);
    bytes[76..108].copy_from_slice(&payload.device_key.0);
    bytes[108..110].copy_from_slice(&payload.capabilities.bits().to_be_bytes());
    Ok(bytes)
}

pub(crate) fn decode_pairing_payload(bytes: &[u8]) -> Result<PairingPayload> {
    if bytes.len() != PAIRING_PAYLOAD_BYTES {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    if bytes[..4] != *b"TPS1" {
        return Err(ErrorCode::InvalidMagic.into());
    }
    if bytes[6..8] != [0, 0] {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    let version = ControllerProtocolVersion {
        major: read_u16(bytes, 8)?,
        minor: read_u16(bytes, 10)?,
    };
    version.require_v1()?;
    Ok(PairingPayload {
        step: PairingStep::from_wire(bytes[4])?,
        role: PairingRole::from_wire(bytes[5])?,
        version,
        nonce: PairingNonce(read_array(bytes, 12)?),
        host_key: HostStaticPublicKey(read_array(bytes, 44)?),
        device_key: DeviceStaticPublicKey(read_array(bytes, 76)?),
        capabilities: CapabilitySet::from_bits(read_u16(bytes, 108)?)?,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(
        bytes
            .get(offset..offset + 2)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(ErrorCode::InvalidEncoding)?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(
        bytes
            .get(offset..offset + 8)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(ErrorCode::InvalidEncoding)?,
    ))
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    bytes
        .get(offset..offset + N)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| ErrorCode::InvalidEncoding.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CONTROLLER_V1;

    fn offer() -> PairingOfferCore {
        PairingOfferCore {
            version: CONTROLLER_V1,
            expires_at_unix_seconds: 500,
            nonce: PairingNonce([7; 32]),
            host_static_public_key: HostStaticPublicKey([9; 32]),
            capabilities: CapabilitySet::default()
                .with(crate::types::ControllerCapability::ObserveSessions),
        }
    }

    #[test]
    fn offer_round_trip_is_canonical() {
        let expected = offer();
        let encoded =
            encode_offer(&expected).unwrap_or_else(|error| panic!("offer encode failed: {error}"));
        let decoded =
            decode_offer(&encoded).unwrap_or_else(|error| panic!("offer decode failed: {error}"));
        assert_eq!(decoded, expected);
        assert_eq!(encode_offer(&decoded), Ok(encoded));
    }

    #[test]
    fn future_major_fails_before_body_validation() {
        let mut encoded =
            encode_offer(&offer()).unwrap_or_else(|error| panic!("offer encode failed: {error}"));
        encoded[4..6].copy_from_slice(&2_u16.to_be_bytes());
        encoded[8] = 255;
        assert_eq!(
            decode_offer(&encoded).map_err(|error| error.code()),
            Err(ErrorCode::IncompatibleVersion)
        );
    }
}
