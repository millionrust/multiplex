use hkdf::Hkdf;
use sha2::{Digest, Sha256};

use crate::error::{ErrorCode, Result};
use crate::types::{
    ControllerProtocolVersion, DeviceStaticPublicKey, HandshakeHash, HostStaticPublicKey,
    PairingNonce, SasCode,
};

const SAS_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn derive_sas_v1(
    nonce: &PairingNonce,
    handshake_hash: &HandshakeHash,
    version: ControllerProtocolVersion,
    host_key: HostStaticPublicKey,
    device_key: DeviceStaticPublicKey,
) -> Result<SasCode> {
    version.require_v1()?;

    let mut salt_input = Vec::with_capacity(60);
    salt_input.extend_from_slice(b"termirust-controller-sas-v1\0");
    salt_input.extend_from_slice(&nonce.0);
    let salt = Sha256::digest(&salt_input);

    let mut info = Vec::with_capacity(104);
    info.extend_from_slice(b"sas\0");
    info.extend_from_slice(&version.major.to_be_bytes());
    info.extend_from_slice(&version.minor.to_be_bytes());
    info.extend_from_slice(&host_key.0);
    info.extend_from_slice(&device_key.0);

    let hkdf = Hkdf::<Sha256>::new(Some(&salt), &handshake_hash.0);
    let mut output = [0_u8; 5];
    hkdf.expand(&info, &mut output)
        .map_err(|_| ErrorCode::CryptoFailure)?;

    let mut display = [0_u8; 9];
    let bits = u64::from_be_bytes([
        0, 0, 0, output[0], output[1], output[2], output[3], output[4],
    ]);
    for index in 0..8 {
        let shift = 35 - index * 5;
        display[if index < 4 { index } else { index + 1 }] =
            SAS_ALPHABET[((bits >> shift) & 0x1f) as usize];
    }
    display[4] = b'-';
    Ok(SasCode::new(display))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CONTROLLER_V1;

    fn sequential(start: u8) -> [u8; 32] {
        core::array::from_fn(|index| start.wrapping_add(index as u8))
    }

    #[test]
    fn normative_anchor_is_exact() {
        let nonce = PairingNonce(sequential(0x00));
        let hash = HandshakeHash(sequential(0x20));
        let host = HostStaticPublicKey(sequential(0x40));
        let device = DeviceStaticPublicKey(sequential(0x60));
        let sas = derive_sas_v1(&nonce, &hash, CONTROLLER_V1, host, device)
            .unwrap_or_else(|error| panic!("anchor failed: {error}"));
        assert_eq!(sas.as_str(), "YKHM-ZHBT");
    }
}
