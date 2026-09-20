use multiplex_relay_protocol::{
    RelayAdmissionChallenge, RelayAdmissionProof, RelayAdmissionResult, RelayClientHello,
    RelayEnvelopeV1,
};
use std::io::{self, Read};

fn main() {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input).unwrap();

    let _ = RelayEnvelopeV1::decode(&input);
    let _ = RelayClientHello::decode(&input);
    let _ = RelayAdmissionChallenge::decode(&input);
    let _ = RelayAdmissionProof::decode(&input);
    let _ = RelayAdmissionResult::decode(&input);
}
