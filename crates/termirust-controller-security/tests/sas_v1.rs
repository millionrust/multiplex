use termirust_controller_security::{
    CONTROLLER_V1, ControllerProtocolVersion, DeviceStaticPublicKey, ErrorCode, HandshakeHash,
    HostStaticPublicKey, PairingNonce, derive_sas_v1,
};

fn bytes(start: u8) -> [u8; 32] {
    core::array::from_fn(|index| start.wrapping_add(index as u8))
}

#[test]
fn normative_sas_v1_anchor_matches_every_byte() {
    let nonce = PairingNonce(bytes(0x00));
    let hash = HandshakeHash(bytes(0x20));
    let host = HostStaticPublicKey(bytes(0x40));
    let device = DeviceStaticPublicKey(bytes(0x60));
    let sas = derive_sas_v1(&nonce, &hash, CONTROLLER_V1, host, device)
        .unwrap_or_else(|error| panic!("SAS anchor failed: {error}"));
    assert_eq!(sas.as_str(), "YKHM-ZHBT");
    assert_eq!(sas.accessibility_symbols().len(), 8);
}

#[test]
fn any_single_bit_change_to_a_bound_256_bit_field_changes_anchor() {
    let nonce = bytes(0x00);
    let hash = bytes(0x20);
    let host = bytes(0x40);
    let device = bytes(0x60);
    let expected = "YKHM-ZHBT";
    for field in 0..4 {
        for bit in 0..256 {
            let mut values = [nonce, hash, host, device];
            values[field][bit / 8] ^= 1 << (bit % 8);
            let changed = derive_sas_v1(
                &PairingNonce(values[0]),
                &HandshakeHash(values[1]),
                CONTROLLER_V1,
                HostStaticPublicKey(values[2]),
                DeviceStaticPublicKey(values[3]),
            )
            .unwrap_or_else(|error| panic!("mutated SAS failed: {error}"));
            assert_ne!(changed.as_str(), expected, "field={field}, bit={bit}");
        }
    }
}

#[test]
fn unsupported_version_never_derives_a_code() {
    let result = derive_sas_v1(
        &PairingNonce([0; 32]),
        &HandshakeHash([0; 32]),
        ControllerProtocolVersion { major: 2, minor: 0 },
        HostStaticPublicKey([0; 32]),
        DeviceStaticPublicKey([0; 32]),
    );
    assert_eq!(
        result.map_err(|error| error.code()),
        Err(ErrorCode::IncompatibleVersion)
    );
}
