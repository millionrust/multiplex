//! Six-digit code pairing key exchange.
//!
//! CPace (draft-irtf-cfrg-cpace-14) over Ristretto255 with SHA-512 turns the code the desktop
//! shows into a uniformly random key both sides share only if they used the same code. An
//! active attacker learns nothing that lets them test other codes offline, so each pairing
//! attempt is one guess; the Host limits attempts per code. The resulting [`CodeBinding`] is
//! bound into the Noise XX pairing prologue, which authenticates the Host static key to the
//! phone and the device static key to the Host without a SAS comparison.

use core::fmt;

use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity as _;
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest as _, Sha512};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::codec::{PAIRING_OFFER_BYTES, encode_offer};
use crate::error::{ErrorCode, Result};
use crate::types::{PairingNonce, PairingOfferCore, PairingRole};

pub const PAIRING_CODE_DIGITS: usize = 6;
pub const CODE_PAIRING_SHARE_BYTES: usize = 32;

const DSI: &[u8] = b"CPaceRistretto255";
const DSI_ISK: &[u8] = b"CPaceRistretto255_ISK";
const HASH_BLOCK_BYTES: usize = 128;
const CHANNEL_IDENTIFIER: &[u8] = b"multiplex-controller-code-v2";
const DEVICE_ASSOCIATED_DATA: &[u8] = b"multiplex-controller-device";
const BINDING_LABEL: &[u8] = b"multiplex-controller-code-binding-v2\0";
const CODE_SPACE: u32 = 1_000_000;

/// The six-digit code shown on the desktop and typed on the phone.
#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct PairingCode([u8; PAIRING_CODE_DIGITS]);

impl PairingCode {
    /// Accepts exactly six ASCII digits.
    pub fn parse(value: &str) -> Result<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != PAIRING_CODE_DIGITS || !bytes.iter().all(u8::is_ascii_digit) {
            return Err(ErrorCode::InvalidEncoding.into());
        }
        let mut code = [0_u8; PAIRING_CODE_DIGITS];
        code.copy_from_slice(bytes);
        Ok(Self(code))
    }

    /// A uniformly random code, using rejection sampling so no code is more likely.
    pub fn generate(rng: &mut (impl RngCore + CryptoRng)) -> Self {
        let limit = u32::MAX - (u32::MAX % CODE_SPACE);
        loop {
            let value = rng.next_u32();
            if value < limit {
                return Self::from_number(value % CODE_SPACE);
            }
        }
    }

    fn from_number(mut value: u32) -> Self {
        let mut code = [b'0'; PAIRING_CODE_DIGITS];
        for digit in code.iter_mut().rev() {
            *digit = b'0' + (value % 10) as u8;
            value /= 10;
        }
        Self(code)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.0).unwrap_or_default()
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingCode([REDACTED])")
    }
}

/// The key both sides derive from a matching code, bound into the pairing prologue.
#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct CodeBinding([u8; 32]);

impl CodeBinding {
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for CodeBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CodeBinding([REDACTED])")
    }
}

/// One side of the code key exchange. The device is CPace party A, the Host party B.
pub struct CodeKeyExchange {
    role: PairingRole,
    scalar: Scalar,
    share: [u8; CODE_PAIRING_SHARE_BYTES],
    sid: [u8; 64],
    offer: [u8; PAIRING_OFFER_BYTES],
}

impl fmt::Debug for CodeKeyExchange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodeKeyExchange")
            .field("role", &self.role)
            .finish()
    }
}

impl Drop for CodeKeyExchange {
    fn drop(&mut self) {
        self.scalar.zeroize();
    }
}

impl CodeKeyExchange {
    /// Starts the exchange for `offer`, keyed by `code`. `device_nonce` is the phone's fresh
    /// nonce and `scalar_entropy` 64 fresh random bytes.
    pub fn new(
        role: PairingRole,
        code: &PairingCode,
        offer: &PairingOfferCore,
        device_nonce: &PairingNonce,
        mut scalar_entropy: [u8; 64],
    ) -> Result<Self> {
        let offer = encode_offer(offer)?;
        let mut sid = [0_u8; 64];
        sid[..32].copy_from_slice(&device_nonce.0);
        sid[32..].copy_from_slice(&offer[18..50]);
        let scalar = Scalar::from_bytes_mod_order_wide(&scalar_entropy);
        scalar_entropy.zeroize();
        if scalar == Scalar::ZERO {
            return Err(ErrorCode::CryptoFailure.into());
        }
        let generator = calculate_generator(&code.0, &channel_identifier(&offer), &sid);
        let share = (scalar * generator).compress().to_bytes();
        Ok(Self {
            role,
            scalar,
            share,
            sid,
            offer,
        })
    }

    #[must_use]
    pub fn share(&self) -> [u8; CODE_PAIRING_SHARE_BYTES] {
        self.share
    }

    /// Completes the exchange with the peer's share. Fails on an invalid or neutral point,
    /// which is how a hostile share is rejected.
    pub fn finish(self, peer_share: &[u8]) -> Result<CodeBinding> {
        let peer: [u8; CODE_PAIRING_SHARE_BYTES] = peer_share
            .try_into()
            .map_err(|_| ErrorCode::InvalidEncoding)?;
        let mut secret = scalar_mult_vfy(&self.scalar, &peer).ok_or(ErrorCode::WrongKey)?;
        let (device_share, host_share) = match self.role {
            PairingRole::DeviceInitiator => (&self.share, &peer),
            PairingRole::HostResponder => (&peer, &self.share),
        };
        let mut isk = intermediate_session_key(
            &self.sid,
            &secret,
            device_share,
            DEVICE_ASSOCIATED_DATA,
            host_share,
            &self.offer,
        );
        secret.zeroize();
        let mut hash = Sha512::new();
        hash.update(BINDING_LABEL);
        hash.update(isk);
        isk.zeroize();
        let mut digest: [u8; 64] = hash.finalize().into();
        let mut binding = [0_u8; 32];
        binding.copy_from_slice(&digest[..32]);
        digest.zeroize();
        Ok(CodeBinding(binding))
    }
}

/// The channel identifier names the protocol and the exact offer, so a code cannot be used
/// against another Host key, nonce, expiry, or capability set.
fn channel_identifier(offer: &[u8; PAIRING_OFFER_BYTES]) -> Vec<u8> {
    let mut identifier = Vec::with_capacity(CHANNEL_IDENTIFIER.len() + PAIRING_OFFER_BYTES);
    identifier.extend_from_slice(CHANNEL_IDENTIFIER);
    identifier.extend_from_slice(offer);
    identifier
}

fn prepend_len(output: &mut Vec<u8>, data: &[u8]) {
    let mut length = data.len();
    loop {
        let byte = (length & 0x7f) as u8;
        length >>= 7;
        if length == 0 {
            output.push(byte);
            break;
        }
        output.push(byte | 0x80);
    }
    output.extend_from_slice(data);
}

fn lv_cat(parts: &[&[u8]]) -> Vec<u8> {
    let mut output = Vec::new();
    for part in parts {
        prepend_len(&mut output, part);
    }
    output
}

pub(crate) fn generator_string(prs: &[u8], ci: &[u8], sid: &[u8]) -> Vec<u8> {
    let mut prefix = Vec::new();
    prepend_len(&mut prefix, prs);
    let prs_length = prefix.len();
    prefix.clear();
    prepend_len(&mut prefix, DSI);
    let pad = HASH_BLOCK_BYTES.saturating_sub(1 + prs_length + prefix.len());
    lv_cat(&[DSI, prs, &vec![0_u8; pad], ci, sid])
}

pub(crate) fn calculate_generator(prs: &[u8], ci: &[u8], sid: &[u8]) -> RistrettoPoint {
    let mut input = generator_string(prs, ci, sid);
    let mut digest: [u8; 64] = Sha512::digest(&input).into();
    input.zeroize();
    let generator = RistrettoPoint::from_uniform_bytes(&digest);
    digest.zeroize();
    generator
}

/// Decodes `point` and multiplies it by `scalar`, returning `None` for an invalid encoding or
/// the neutral element.
pub(crate) fn scalar_mult_vfy(scalar: &Scalar, point: &[u8; 32]) -> Option<[u8; 32]> {
    let decoded = CompressedRistretto(*point).decompress()?;
    let product = scalar * decoded;
    (product != RistrettoPoint::identity()).then(|| product.compress().to_bytes())
}

pub(crate) fn intermediate_session_key(
    sid: &[u8],
    secret: &[u8; 32],
    share_a: &[u8],
    associated_a: &[u8],
    share_b: &[u8],
    associated_b: &[u8],
) -> [u8; 64] {
    let mut input = lv_cat(&[DSI_ISK, sid, secret]);
    input.extend_from_slice(&lv_cat(&[share_a, associated_a]));
    input.extend_from_slice(&lv_cat(&[share_b, associated_b]));
    let isk = Sha512::digest(&input).into();
    input.zeroize();
    isk
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CONTROLLER_V2, CapabilitySet, ControllerCapability, HostStaticPublicKey};

    fn hex(value: &str) -> Vec<u8> {
        let value: String = value.split_whitespace().collect();
        (0..value.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
            .collect()
    }

    fn array<const N: usize>(value: &str) -> [u8; N] {
        hex(value).try_into().unwrap()
    }

    // draft-irtf-cfrg-cpace-14, appendix B.3.
    const SID: &str = "7e4b4791d6a8ef019b936c79fb7f2c57";
    const CI: &str = "6f630b425f726573706f6e6465720b415f696e69746961746f72";
    const YA_SCALAR: &str = "da3d23700a9e5699258aef94dc060dfda5ebb61f02a5ea77fad53f4ff0976d08";
    const YB_SCALAR: &str = "d2316b454718c35362d83d69df6320f38578ed5984651435e2949762d900b80d";
    const YA: &str = "d40fb265a7abeaee7939d91a585fe59f7053f982c296ec413c624c669308f87a";
    const YB: &str = "08bcf6e9777a9c313a3db6daa510f2d398403319c2341bd506a92e672eb7e307";
    const K: &str = "e22b1ef7788f661478f3cddd4c600774fc0f41e6b711569190ff88fa0e607e09";

    #[test]
    fn generator_matches_the_cpace_ristretto255_vector() {
        let string = generator_string(b"Password", &hex(CI), &hex(SID));
        assert_eq!(
            string,
            hex("11435061636552697374726574746f3235350850617373776f726464
                 00000000000000000000000000000000000000000000000000000000
                 00000000000000000000000000000000000000000000000000000000
                 00000000000000000000000000000000000000000000000000000000
                 000000000000000000000000000000001a6f630b425f726573706f6e
                 6465720b415f696e69746961746f72107e4b4791d6a8ef019b936c79
                 fb7f2c57")
        );
        assert_eq!(
            calculate_generator(b"Password", &hex(CI), &hex(SID))
                .compress()
                .to_bytes()
                .to_vec(),
            hex("a6fc82c3b8968fbb2e06fee81ca858586dea50d248f0c7ca6a18b0902a30b36b")
        );
    }

    #[test]
    fn shares_secret_and_isk_match_the_cpace_ristretto255_vectors() {
        let generator = calculate_generator(b"Password", &hex(CI), &hex(SID));
        let ya = Scalar::from_bytes_mod_order(array(YA_SCALAR));
        let yb = Scalar::from_bytes_mod_order(array(YB_SCALAR));
        assert_eq!((ya * generator).compress().to_bytes(), array::<32>(YA));
        assert_eq!((yb * generator).compress().to_bytes(), array::<32>(YB));
        assert_eq!(scalar_mult_vfy(&ya, &array(YB)), Some(array(K)));
        assert_eq!(scalar_mult_vfy(&yb, &array(YA)), Some(array(K)));
        assert_eq!(
            intermediate_session_key(&hex(SID), &array(K), &hex(YA), b"ADa", &hex(YB), b"ADb")
                .to_vec(),
            hex("4c5469a16b2364c4b944ebc1a79e51d1674ad47db26e8718154f59fa
                 ebfaa52d8346f30aa58377117eb20d527f2cbc5c76381f7fd372e89d
                 f8239f87f2e02ed1")
        );
    }

    #[test]
    fn scalar_mult_vfy_matches_the_valid_vector_and_rejects_the_invalid_ones() {
        let scalar = Scalar::from_bytes_mod_order(array(
            "7cd0e075fa7955ba52c02759a6c90dbbfc10e6d40aea8d283e407d88cf538a05",
        ));
        assert_eq!(
            scalar_mult_vfy(
                &scalar,
                &array("2c3c6b8c4f3800e7aef6864025b4ed79bd599117e427c41bd47d93d654b4a51c")
            ),
            Some(array(
                "7c13645fe790a468f62c39beb7388e541d8405d1ade69d1778c5fe3e7f6b600e"
            ))
        );
        assert_eq!(
            scalar_mult_vfy(
                &scalar,
                &array("2b3c6b8c4f3800e7aef6864025b4ed79bd599117e427c41bd47d93d654b4a51c")
            ),
            None
        );
        assert_eq!(scalar_mult_vfy(&scalar, &[0; 32]), None);
    }

    fn offer() -> PairingOfferCore {
        PairingOfferCore {
            version: CONTROLLER_V2,
            expires_at_unix_seconds: 1_000,
            nonce: PairingNonce([5; 32]),
            host_static_public_key: HostStaticPublicKey([6; 32]),
            capabilities: CapabilitySet::default().with(ControllerCapability::ObserveSessions),
        }
    }

    fn exchange(
        role: PairingRole,
        code: &str,
        offer: &PairingOfferCore,
        seed: u8,
    ) -> CodeKeyExchange {
        CodeKeyExchange::new(
            role,
            &PairingCode::parse(code).unwrap(),
            offer,
            &PairingNonce([9; 32]),
            [seed; 64],
        )
        .unwrap()
    }

    #[test]
    fn matching_codes_agree_and_any_difference_disagrees() {
        let offer = offer();
        let device = exchange(PairingRole::DeviceInitiator, "042917", &offer, 1);
        let host = exchange(PairingRole::HostResponder, "042917", &offer, 2);
        let (device_share, host_share) = (device.share(), host.share());
        assert_eq!(
            device.finish(&host_share).unwrap(),
            host.finish(&device_share).unwrap()
        );

        let device = exchange(PairingRole::DeviceInitiator, "042918", &offer, 1);
        let host = exchange(PairingRole::HostResponder, "042917", &offer, 2);
        let (device_share, host_share) = (device.share(), host.share());
        assert_ne!(
            device.finish(&host_share).unwrap(),
            host.finish(&device_share).unwrap()
        );

        // The same code against a different offer (here, another Host key) disagrees too.
        let mut other = offer.clone();
        other.host_static_public_key = HostStaticPublicKey([7; 32]);
        let device = exchange(PairingRole::DeviceInitiator, "042917", &other, 1);
        let host = exchange(PairingRole::HostResponder, "042917", &offer, 2);
        let (device_share, host_share) = (device.share(), host.share());
        assert_ne!(
            device.finish(&host_share).unwrap(),
            host.finish(&device_share).unwrap()
        );
    }

    #[test]
    fn hostile_shares_are_rejected() {
        let offer = offer();
        for share in [[0_u8; 32].to_vec(), [0xff; 32].to_vec(), vec![1; 31]] {
            let host = exchange(PairingRole::HostResponder, "000000", &offer, 3);
            assert!(host.finish(&share).is_err());
        }
    }

    #[test]
    fn codes_are_exactly_six_digits_uniform_and_redacted() {
        assert!(PairingCode::parse("123456").is_ok());
        for invalid in ["12345", "1234567", "12345a", "123 45", "١٢٣٤٥٦", ""] {
            assert!(PairingCode::parse(invalid).is_err(), "accepted {invalid:?}");
        }
        assert_eq!(PairingCode::from_number(7).as_str(), "000007");
        assert_eq!(PairingCode::from_number(999_999).as_str(), "999999");
        assert_eq!(
            format!("{:?}", PairingCode::parse("123456").unwrap()),
            "PairingCode([REDACTED])"
        );

        struct Fixed(Vec<u32>);
        impl RngCore for Fixed {
            fn next_u32(&mut self) -> u32 {
                self.0.remove(0)
            }
            fn next_u64(&mut self) -> u64 {
                u64::from(self.next_u32())
            }
            fn fill_bytes(&mut self, dest: &mut [u8]) {
                dest.fill(0);
            }
            fn try_fill_bytes(
                &mut self,
                dest: &mut [u8],
            ) -> core::result::Result<(), rand_core::Error> {
                self.fill_bytes(dest);
                Ok(())
            }
        }
        impl CryptoRng for Fixed {}
        // Values in the biased tail above the last whole block of a million are redrawn.
        let mut rng = Fixed(vec![u32::MAX, 4_294_000_000, 1_000_042]);
        assert_eq!(PairingCode::generate(&mut rng).as_str(), "000042");
    }
}
