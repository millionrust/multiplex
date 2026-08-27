use core::fmt;

use clatter::cipherstate::CipherState;
use clatter::crypto::cipher::ChaChaPoly;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::authorization::AuthorizationPolicy;
use crate::error::{ErrorCode, Result};
use crate::types::{
    CONTROLLER_V1, ControllerCapability, ControllerFrame, ControllerFrameKind,
    MAX_CONTROL_PAYLOAD_BYTES, MAX_TERMINAL_FRAME_BYTES, RevocationEpoch, SealedControllerFrame,
};

const FRAME_MAGIC: [u8; 4] = *b"TCF1";
const FRAME_HEADER_BYTES: usize = 32;
const AEAD_TAG_BYTES: usize = 16;
const REKEY_INTERVAL: u64 = 1 << 20;
pub const MAX_SEQUENCE: u64 = u64::MAX - 2;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ControllerTransport {
    send: CipherState<ChaChaPoly>,
    receive: CipherState<ChaChaPoly>,
    #[zeroize(skip)]
    policy: AuthorizationPolicy,
    #[zeroize(skip)]
    failed: bool,
}

impl fmt::Debug for ControllerTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ControllerTransport([REDACTED])")
    }
}

impl ControllerTransport {
    pub(crate) fn new(
        send: CipherState<ChaChaPoly>,
        receive: CipherState<ChaChaPoly>,
        policy: AuthorizationPolicy,
    ) -> Self {
        Self {
            send,
            receive,
            policy,
            failed: false,
        }
    }

    pub fn seal(
        &mut self,
        kind: ControllerFrameKind,
        capability: ControllerCapability,
        revocation_epoch: RevocationEpoch,
        payload: &[u8],
    ) -> Result<SealedControllerFrame> {
        self.ensure_active()?;
        match self.seal_inner(kind, capability, revocation_epoch, payload) {
            Ok(frame) => Ok(frame),
            Err(error) => self.fail(error),
        }
    }

    fn seal_inner(
        &mut self,
        kind: ControllerFrameKind,
        capability: ControllerCapability,
        revocation_epoch: RevocationEpoch,
        payload: &[u8],
    ) -> Result<SealedControllerFrame> {
        self.policy.require(capability, revocation_epoch)?;
        validate_payload_limit(kind, payload.len())?;
        let sequence = self.send.get_nonce();
        if sequence > MAX_SEQUENCE {
            return Err(ErrorCode::SequenceExhausted.into());
        }
        if sequence != 0 && sequence.is_multiple_of(REKEY_INTERVAL) {
            self.send.rekey().map_err(|_| ErrorCode::CryptoFailure)?;
        }
        let ciphertext_len = payload
            .len()
            .checked_add(AEAD_TAG_BYTES)
            .ok_or(ErrorCode::FrameTooLarge)?;
        let frame_len = FRAME_HEADER_BYTES
            .checked_add(ciphertext_len)
            .ok_or(ErrorCode::FrameTooLarge)?;
        if kind == ControllerFrameKind::Terminal && frame_len > MAX_TERMINAL_FRAME_BYTES {
            return Err(ErrorCode::FrameTooLarge.into());
        }
        let mut frame = vec![0_u8; frame_len];
        encode_header(
            &mut frame[..FRAME_HEADER_BYTES],
            kind,
            capability,
            revocation_epoch,
            sequence,
            ciphertext_len,
        )?;
        let (header, ciphertext) = frame.split_at_mut(FRAME_HEADER_BYTES);
        self.send
            .encrypt_with_ad(header, payload, ciphertext)
            .map_err(|_| ErrorCode::CryptoFailure)?;
        Ok(SealedControllerFrame::new(frame))
    }

    pub fn open(&mut self, bytes: &[u8]) -> Result<ControllerFrame> {
        self.ensure_active()?;
        match self.open_inner(bytes) {
            Ok(frame) => Ok(frame),
            Err(error) => self.fail(error),
        }
    }

    fn open_inner(&mut self, bytes: &[u8]) -> Result<ControllerFrame> {
        let header = decode_header(bytes)?;
        self.policy
            .require(header.capability, header.revocation_epoch)?;
        let expected = self.receive.get_nonce();
        if header.sequence < expected {
            return Err(ErrorCode::DuplicateFrame.into());
        }
        if header.sequence > expected {
            return Err(ErrorCode::OutOfOrderFrame.into());
        }
        if header.sequence > MAX_SEQUENCE {
            return Err(ErrorCode::SequenceExhausted.into());
        }
        if header.sequence != 0 && header.sequence.is_multiple_of(REKEY_INTERVAL) {
            self.receive.rekey().map_err(|_| ErrorCode::CryptoFailure)?;
        }
        let payload_len = header
            .ciphertext_len
            .checked_sub(AEAD_TAG_BYTES)
            .ok_or(ErrorCode::InvalidEncoding)?;
        validate_payload_limit(header.kind, payload_len)?;
        let mut payload = vec![0_u8; payload_len];
        self.receive
            .decrypt_with_ad(
                &bytes[..FRAME_HEADER_BYTES],
                &bytes[FRAME_HEADER_BYTES..],
                &mut payload,
            )
            .map_err(|_| ErrorCode::AuthenticationFailed)?;
        Ok(ControllerFrame {
            kind: header.kind,
            capability: header.capability,
            revocation_epoch: header.revocation_epoch,
            sequence: header.sequence,
            payload,
        })
    }

    fn ensure_active(&self) -> Result<()> {
        if self.failed {
            Err(ErrorCode::WrongState.into())
        } else {
            Ok(())
        }
    }

    fn fail<T>(&mut self, error: crate::error::ControllerSecurityError) -> Result<T> {
        self.send.zeroize();
        self.receive.zeroize();
        self.failed = true;
        Err(error)
    }

    #[cfg(test)]
    pub(crate) fn from_test_keys(
        send_key: &[u8; 32],
        receive_key: &[u8; 32],
        policy: AuthorizationPolicy,
        sequence: u64,
    ) -> Self {
        Self::new(
            CipherState::new(send_key, sequence),
            CipherState::new(receive_key, sequence),
            policy,
        )
    }
}

struct Header {
    kind: ControllerFrameKind,
    capability: ControllerCapability,
    revocation_epoch: RevocationEpoch,
    sequence: u64,
    ciphertext_len: usize,
}

fn encode_header(
    bytes: &mut [u8],
    kind: ControllerFrameKind,
    capability: ControllerCapability,
    epoch: RevocationEpoch,
    sequence: u64,
    ciphertext_len: usize,
) -> Result<()> {
    if bytes.len() != FRAME_HEADER_BYTES {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    let ciphertext_len = u32::try_from(ciphertext_len).map_err(|_| ErrorCode::FrameTooLarge)?;
    bytes[..4].copy_from_slice(&FRAME_MAGIC);
    bytes[4..6].copy_from_slice(&CONTROLLER_V1.major.to_be_bytes());
    bytes[6..8].copy_from_slice(&CONTROLLER_V1.minor.to_be_bytes());
    bytes[8] = kind as u8;
    bytes[9] = capability as u8;
    bytes[10..12].copy_from_slice(&0_u16.to_be_bytes());
    bytes[12..20].copy_from_slice(&epoch.0.to_be_bytes());
    bytes[20..28].copy_from_slice(&sequence.to_be_bytes());
    bytes[28..32].copy_from_slice(&ciphertext_len.to_be_bytes());
    Ok(())
}

fn decode_header(bytes: &[u8]) -> Result<Header> {
    if bytes.len() < FRAME_HEADER_BYTES {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    if bytes[..4] != FRAME_MAGIC {
        return Err(ErrorCode::InvalidMagic.into());
    }
    let version = crate::types::ControllerProtocolVersion {
        major: read_u16(bytes, 4)?,
        minor: read_u16(bytes, 6)?,
    };
    version.require_v1()?;
    if bytes[10..12] != [0, 0] {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    let kind = ControllerFrameKind::from_wire(bytes[8])?;
    let ciphertext_len = read_u32(bytes, 28)? as usize;
    let expected_len = FRAME_HEADER_BYTES
        .checked_add(ciphertext_len)
        .ok_or(ErrorCode::FrameTooLarge)?;
    if bytes.len() != expected_len {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    if kind == ControllerFrameKind::Terminal && expected_len > MAX_TERMINAL_FRAME_BYTES {
        return Err(ErrorCode::FrameTooLarge.into());
    }
    Ok(Header {
        kind,
        capability: ControllerCapability::from_wire(bytes[9])?,
        revocation_epoch: RevocationEpoch(read_u64(bytes, 12)?),
        sequence: read_u64(bytes, 20)?,
        ciphertext_len,
    })
}

fn validate_payload_limit(kind: ControllerFrameKind, payload_len: usize) -> Result<()> {
    match kind {
        ControllerFrameKind::Control if payload_len > MAX_CONTROL_PAYLOAD_BYTES => {
            Err(ErrorCode::FrameTooLarge.into())
        }
        ControllerFrameKind::Terminal
            if payload_len
                .checked_add(FRAME_HEADER_BYTES + AEAD_TAG_BYTES)
                .is_none_or(|length| length > MAX_TERMINAL_FRAME_BYTES) =>
        {
            Err(ErrorCode::FrameTooLarge.into())
        }
        _ => Ok(()),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(read_array(bytes, offset)?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(read_array(bytes, offset)?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(read_array(bytes, offset)?))
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
    use crate::types::CapabilitySet;

    fn transports(sequence: u64) -> (ControllerTransport, ControllerTransport) {
        let capabilities = CapabilitySet::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput);
        let policy = AuthorizationPolicy::new(capabilities, RevocationEpoch(7));
        (
            ControllerTransport::from_test_keys(&[1; 32], &[2; 32], policy, sequence),
            ControllerTransport::from_test_keys(&[2; 32], &[1; 32], policy, sequence),
        )
    }

    #[test]
    fn boundary_and_rekey_sequences_round_trip() {
        for sequence in [0, REKEY_INTERVAL, MAX_SEQUENCE] {
            let (mut sender, mut receiver) = transports(sequence);
            let sealed = sender
                .seal(
                    ControllerFrameKind::Control,
                    ControllerCapability::ObserveSessions,
                    RevocationEpoch(7),
                    b"bounded",
                )
                .unwrap_or_else(|error| panic!("seal failed: {error}"));
            let opened = receiver
                .open(sealed.as_bytes())
                .unwrap_or_else(|error| panic!("open failed: {error}"));
            assert_eq!(opened.sequence, sequence);
            assert_eq!(opened.payload, b"bounded");
        }
    }

    #[test]
    fn tamper_replay_gap_and_revocation_fail_closed() {
        let (mut sender, mut receiver) = transports(0);
        let sealed = sender
            .seal(
                ControllerFrameKind::Control,
                ControllerCapability::ObserveSessions,
                RevocationEpoch(7),
                b"secret",
            )
            .unwrap_or_else(|error| panic!("seal failed: {error}"));
        let mut tampered = sealed.as_bytes().to_vec();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert_eq!(
            receiver.open(&tampered).map_err(|error| error.code()),
            Err(ErrorCode::AuthenticationFailed)
        );
        assert_eq!(
            receiver
                .open(sealed.as_bytes())
                .map_err(|error| error.code()),
            Err(ErrorCode::WrongState)
        );

        let (mut sender, mut receiver) = transports(0);
        let sealed = sender
            .seal(
                ControllerFrameKind::Control,
                ControllerCapability::ObserveSessions,
                RevocationEpoch(7),
                b"replay",
            )
            .unwrap_or_else(|error| panic!("seal failed: {error}"));
        assert!(receiver.open(sealed.as_bytes()).is_ok());
        assert_eq!(
            receiver
                .open(sealed.as_bytes())
                .map_err(|error| error.code()),
            Err(ErrorCode::DuplicateFrame)
        );
        assert_eq!(
            receiver
                .open(sealed.as_bytes())
                .map_err(|error| error.code()),
            Err(ErrorCode::WrongState)
        );

        let (mut sender, mut receiver) = transports(0);
        let first = sender
            .seal(
                ControllerFrameKind::Control,
                ControllerCapability::ObserveSessions,
                RevocationEpoch(7),
                b"one",
            )
            .unwrap_or_else(|error| panic!("first seal failed: {error}"));
        let second = sender
            .seal(
                ControllerFrameKind::Control,
                ControllerCapability::ObserveSessions,
                RevocationEpoch(7),
                b"two",
            )
            .unwrap_or_else(|error| panic!("second seal failed: {error}"));
        assert_eq!(
            receiver
                .open(second.as_bytes())
                .map_err(|error| error.code()),
            Err(ErrorCode::OutOfOrderFrame)
        );
        assert_eq!(
            receiver
                .open(first.as_bytes())
                .map_err(|error| error.code()),
            Err(ErrorCode::WrongState)
        );

        assert_eq!(
            sender
                .seal(
                    ControllerFrameKind::Control,
                    ControllerCapability::ObserveSessions,
                    RevocationEpoch(8),
                    b"revoked",
                )
                .map_err(|error| error.code()),
            Err(ErrorCode::CapabilityDenied)
        );
        assert_eq!(
            sender
                .seal(
                    ControllerFrameKind::Control,
                    ControllerCapability::ObserveSessions,
                    RevocationEpoch(7),
                    b"after failure",
                )
                .map_err(|error| error.code()),
            Err(ErrorCode::WrongState)
        );
    }

    #[test]
    fn payload_and_sequence_limits_are_exact_and_fail_closed() {
        for (kind, max_payload) in [
            (ControllerFrameKind::Control, MAX_CONTROL_PAYLOAD_BYTES),
            (
                ControllerFrameKind::Terminal,
                MAX_TERMINAL_FRAME_BYTES - FRAME_HEADER_BYTES - AEAD_TAG_BYTES,
            ),
        ] {
            let (mut sender, mut receiver) = transports(0);
            let payload = vec![0x5a; max_payload];
            let sealed = sender
                .seal(
                    kind,
                    ControllerCapability::AttachOutput,
                    RevocationEpoch(7),
                    &payload,
                )
                .unwrap_or_else(|error| panic!("boundary seal failed: {error}"));
            assert_eq!(
                receiver
                    .open(sealed.as_bytes())
                    .unwrap_or_else(|error| panic!("boundary open failed: {error}"))
                    .payload,
                payload
            );

            let (mut sender, _) = transports(0);
            assert_eq!(
                sender
                    .seal(
                        kind,
                        ControllerCapability::AttachOutput,
                        RevocationEpoch(7),
                        &vec![0; max_payload + 1],
                    )
                    .map_err(|error| error.code()),
                Err(ErrorCode::FrameTooLarge)
            );
            assert_eq!(
                sender
                    .seal(
                        kind,
                        ControllerCapability::AttachOutput,
                        RevocationEpoch(7),
                        b"after failure",
                    )
                    .map_err(|error| error.code()),
                Err(ErrorCode::WrongState)
            );
        }

        let (mut sender, _) = transports(MAX_SEQUENCE);
        sender
            .seal(
                ControllerFrameKind::Control,
                ControllerCapability::ObserveSessions,
                RevocationEpoch(7),
                b"last",
            )
            .unwrap_or_else(|error| panic!("last sequence failed: {error}"));
        assert_eq!(
            sender
                .seal(
                    ControllerFrameKind::Control,
                    ControllerCapability::ObserveSessions,
                    RevocationEpoch(7),
                    b"exhausted",
                )
                .map_err(|error| error.code()),
            Err(ErrorCode::SequenceExhausted)
        );
        assert_eq!(
            sender
                .seal(
                    ControllerFrameKind::Control,
                    ControllerCapability::ObserveSessions,
                    RevocationEpoch(7),
                    b"after failure",
                )
                .map_err(|error| error.code()),
            Err(ErrorCode::WrongState)
        );
    }
}
