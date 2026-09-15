mod common;

use proptest::prelude::*;
use serde::Deserialize;
use termirust_controller_security::{
    ControllerCapability, ControllerFrame, ErrorCode, HandshakeHash, HandshakeMessage,
    PairingNonce, SasCode, SealedControllerFrame, StaticPrivateKey, decode_offer,
};
use zeroize::ZeroizeOnDrop;

proptest! {
    #[test]
    fn arbitrary_offer_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = decode_offer(&bytes);
    }
}

#[test]
fn malformed_handshake_corpus_fails_closed_without_secret_diagnostics() {
    for length in [0, 1, 31, 32, 141, 143, 206, 207] {
        let (_, mut host) = common::machines();
        let bytes = vec![0xa5; length];
        let result = host.read_next(&bytes, common::NOW_MILLIS + 1);
        assert!(result.is_err(), "length {length} unexpectedly succeeded");
    }
    let canary = [0x5a; 32];
    let diagnostics = format!(
        "{:?} {:?} {:?} {:?} {:?}",
        StaticPrivateKey::from_fixture_bytes(canary),
        PairingNonce(canary),
        HandshakeHash(canary),
        common::machines().0.write_next(common::NOW_MILLIS + 1),
        ControllerFrame {
            kind: termirust_controller_security::ControllerFrameKind::Control,
            capability: ControllerCapability::ObserveSessions,
            revocation_epoch: termirust_controller_security::RevocationEpoch(0),
            sequence: 0,
            payload: canary.to_vec(),
        }
    );
    assert!(!diagnostics.contains("5a5a"));
    assert!(!diagnostics.contains("ZZZZ"));
    assert!(diagnostics.contains("REDACTED"));
}

#[derive(Deserialize)]
struct OfferCorpusCase {
    id: String,
    hex: Option<String>,
    repeat_hex: Option<String>,
    count: Option<usize>,
    error: String,
}

#[test]
fn committed_malformed_offer_corpus_fails_with_frozen_errors() {
    let cases: Vec<OfferCorpusCase> =
        serde_json::from_str(include_str!("malformed/offer-corpus.json"))
            .unwrap_or_else(|error| panic!("malformed corpus JSON failed: {error}"));
    for case in cases {
        let bytes = if let Some(hex) = case.hex {
            hex::decode(hex).unwrap_or_else(|error| panic!("{} hex failed: {error}", case.id))
        } else {
            let repeated = hex::decode(case.repeat_hex.unwrap_or_default())
                .unwrap_or_else(|error| panic!("{} repeated hex failed: {error}", case.id));
            repeated.repeat(case.count.unwrap_or_default())
        };
        let actual = decode_offer(&bytes)
            .map(|_| "Ok".to_owned())
            .unwrap_or_else(|error| format!("{:?}", error.code()));
        assert_eq!(actual, case.error, "corpus case {}", case.id);
    }
}

#[test]
fn all_live_secret_wrappers_are_zeroize_on_drop() {
    fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}
    assert_zeroize_on_drop::<StaticPrivateKey>();
    assert_zeroize_on_drop::<PairingNonce>();
    assert_zeroize_on_drop::<HandshakeHash>();
    assert_zeroize_on_drop::<SasCode>();
    assert_zeroize_on_drop::<HandshakeMessage>();
    assert_zeroize_on_drop::<SealedControllerFrame>();
    assert_zeroize_on_drop::<ControllerFrame>();
}

#[test]
fn unknown_capability_bits_fail_closed() {
    assert_eq!(
        termirust_controller_security::CapabilitySet::from_bits(0x20).map_err(|error| error.code()),
        Err(ErrorCode::UnknownCapability)
    );
}
