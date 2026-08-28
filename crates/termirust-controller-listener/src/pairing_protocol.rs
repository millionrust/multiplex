use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use termirust_controller_security::{
    PAIRING_OFFER_BYTES, PairingOfferCore, decode_offer, encode_offer,
};
use termirust_domain::{
    AddressFamily, ControllerDeviceId, ControllerNetworkError, ControllerPort, DiscoveryPolicy,
    PairingOfferId, RouteCandidate,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::{ListenerError, ListenerErrorCode, read_bounded_frame, write_bounded_frame};

const CONNECTION_MAGIC: [u8; 4] = *b"TRCN";
const CONNECTION_VERSION: u16 = 1;
const CONNECTION_PREFACE_BYTES: usize = 8;
const PAIRING_ENVELOPE_VERSION: u16 = 1;
const MAX_PAIRING_ENVELOPE_BYTES: usize = 4 * 1024;
const MAX_PAIRING_CONTROL_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ControllerConnectionPurpose {
    Authenticate = 1,
    Pair = 2,
}

impl ControllerConnectionPurpose {
    pub async fn read_from<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Self, ListenerError> {
        let mut bytes = [0; CONNECTION_PREFACE_BYTES];
        reader
            .read_exact(&mut bytes)
            .await
            .map_err(ListenerError::from)?;
        Self::decode(bytes)
    }

    pub async fn write_to<W: AsyncWrite + Unpin>(
        self,
        writer: &mut W,
    ) -> Result<(), ListenerError> {
        writer
            .write_all(&self.encode())
            .await
            .map_err(ListenerError::from)?;
        writer.flush().await.map_err(ListenerError::from)
    }

    fn encode(self) -> [u8; CONNECTION_PREFACE_BYTES] {
        let mut bytes = [0; CONNECTION_PREFACE_BYTES];
        bytes[..4].copy_from_slice(&CONNECTION_MAGIC);
        bytes[4..6].copy_from_slice(&CONNECTION_VERSION.to_be_bytes());
        bytes[6] = self as u8;
        bytes
    }

    fn decode(bytes: [u8; CONNECTION_PREFACE_BYTES]) -> Result<Self, ListenerError> {
        if bytes[..4] != CONNECTION_MAGIC
            || u16::from_be_bytes([bytes[4], bytes[5]]) != CONNECTION_VERSION
            || bytes[7] != 0
        {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        match bytes[6] {
            1 => Ok(Self::Authenticate),
            2 => Ok(Self::Pair),
            _ => Err(ListenerError::new(ListenerErrorCode::MalformedFrame)),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerPairingOffer {
    pub schema_version: u16,
    pub offer_id: PairingOfferId,
    pub identity_generation: u64,
    pub revocation_epoch: u64,
    pub session_generation: u64,
    pub address_family: AddressFamily,
    pub address: IpAddr,
    pub port: u16,
    pub offer_bytes: Vec<u8>,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SshControllerPairingOffer {
    pub schema_version: u16,
    pub offer_id: PairingOfferId,
    pub identity_generation: u64,
    pub revocation_epoch: u64,
    pub session_generation: u64,
    pub offer_bytes: Vec<u8>,
}

impl SshControllerPairingOffer {
    pub fn new(
        offer_id: PairingOfferId,
        offer: &PairingOfferCore,
        identity_generation: u64,
        revocation_epoch: u64,
        session_generation: u64,
    ) -> Result<Self, ListenerError> {
        let value = Self {
            schema_version: PAIRING_ENVELOPE_VERSION,
            offer_id,
            identity_generation,
            revocation_epoch,
            session_generation,
            offer_bytes: encode_offer(offer)
                .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?
                .to_vec(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn offer(&self) -> Result<PairingOfferCore, ListenerError> {
        self.validate()?;
        decode_offer(&self.offer_bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))
    }

    pub async fn read_from<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Self, ListenerError> {
        let value: Self =
            decode_control(&read_bounded_frame(reader, MAX_PAIRING_ENVELOPE_BYTES).await?)?;
        value.validate()?;
        Ok(value)
    }

    pub async fn write_to<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
    ) -> Result<(), ListenerError> {
        self.validate()?;
        write_bounded_frame(writer, &encode_control(self)?, MAX_PAIRING_ENVELOPE_BYTES).await
    }

    fn validate(&self) -> Result<(), ListenerError> {
        if self.schema_version != PAIRING_ENVELOPE_VERSION
            || self.identity_generation == 0
            || self.session_generation == 0
            || self.offer_bytes.len() != PAIRING_OFFER_BYTES
        {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        decode_offer(&self.offer_bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        Ok(())
    }
}

impl fmt::Debug for SshControllerPairingOffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshControllerPairingOffer")
            .field("schema_version", &self.schema_version)
            .field("offer_id", &self.offer_id)
            .field("identity_generation", &self.identity_generation)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("session_generation", &self.session_generation)
            .field("offer_bytes", &"[REDACTED]")
            .finish()
    }
}

impl ControllerPairingOffer {
    pub fn new(
        offer_id: PairingOfferId,
        route: &RouteCandidate,
        offer: &PairingOfferCore,
        identity_generation: u64,
        revocation_epoch: u64,
        session_generation: u64,
    ) -> Result<Self, ListenerError> {
        let value = Self {
            schema_version: PAIRING_ENVELOPE_VERSION,
            offer_id,
            identity_generation,
            revocation_epoch,
            session_generation,
            address_family: route.address_family,
            address: route.address,
            port: route.port.value(),
            offer_bytes: encode_offer(offer)
                .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?
                .to_vec(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn offer(&self) -> Result<PairingOfferCore, ListenerError> {
        self.validate()?;
        decode_offer(&self.offer_bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))
    }

    pub fn encode_text(&self) -> Result<String, ListenerError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        if bytes.len() > MAX_PAIRING_ENVELOPE_BYTES {
            return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
        }
        String::from_utf8(bytes).map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))
    }

    pub fn decode_text(value: &str) -> Result<Self, ListenerError> {
        if value.is_empty() || value.len() > MAX_PAIRING_ENVELOPE_BYTES {
            return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
        }
        let offer: Self = serde_json::from_str(value)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        offer.validate()?;
        Ok(offer)
    }

    fn validate(&self) -> Result<(), ListenerError> {
        if self.schema_version != PAIRING_ENVELOPE_VERSION
            || self.identity_generation == 0
            || self.offer_bytes.len() != PAIRING_OFFER_BYTES
        {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        let port = ControllerPort::user_fixed(self.port).map_err(network_error)?;
        let route = RouteCandidate {
            interface_id: termirust_domain::NetworkInterfaceId::new("pairing-route")
                .map_err(network_error)?,
            address_family: self.address_family,
            address: self.address,
            port,
            discovery: DiscoveryPolicy::Off,
        };
        let policy = termirust_domain::ControllerListenPolicy {
            enabled: true,
            interface_id: Some(route.interface_id),
            address_family: Some(route.address_family),
            selected_address: Some(route.address),
            port: Some(route.port),
            discovery: route.discovery,
        };
        policy.validate().map_err(network_error)?;
        decode_offer(&self.offer_bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        Ok(())
    }
}

impl fmt::Debug for ControllerPairingOffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControllerPairingOffer")
            .field("schema_version", &self.schema_version)
            .field("offer_id", &self.offer_id)
            .field("identity_generation", &self.identity_generation)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("session_generation", &self.session_generation)
            .field("route", &"[REDACTED]")
            .field("offer_bytes", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingConnectRequest {
    pub schema_version: u16,
    pub offer_id: PairingOfferId,
}

impl PairingConnectRequest {
    pub fn new(offer_id: PairingOfferId) -> Self {
        Self {
            schema_version: PAIRING_ENVELOPE_VERSION,
            offer_id,
        }
    }

    pub async fn read_from<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Self, ListenerError> {
        decode_control(&read_bounded_frame(reader, MAX_PAIRING_CONTROL_BYTES).await?)
    }

    pub async fn write_to<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
    ) -> Result<(), ListenerError> {
        if self.schema_version != PAIRING_ENVELOPE_VERSION {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        write_bounded_frame(writer, &encode_control(self)?, MAX_PAIRING_CONTROL_BYTES).await
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingDeviceRegistration {
    pub schema_version: u16,
    pub device_id: ControllerDeviceId,
    pub display_name: String,
}

impl PairingDeviceRegistration {
    pub fn new(device_id: ControllerDeviceId, display_name: String) -> Self {
        Self {
            schema_version: PAIRING_ENVELOPE_VERSION,
            device_id,
            display_name,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, ListenerError> {
        if self.schema_version != PAIRING_ENVELOPE_VERSION
            || self.display_name.is_empty()
            || self.display_name.chars().count() > termirust_domain::MAX_DEVICE_NAME_SCALARS
            || self.display_name.chars().any(char::is_control)
        {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        encode_control(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ListenerError> {
        let value: Self = decode_control(bytes)?;
        value.encode()?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingHostAck {
    pub schema_version: u16,
    pub device_id: ControllerDeviceId,
    pub identity_generation: u64,
    pub revocation_epoch: u64,
    pub session_generation: u64,
    pub capability_bits: u16,
}

impl PairingHostAck {
    pub fn encode(&self) -> Result<Vec<u8>, ListenerError> {
        if self.schema_version != PAIRING_ENVELOPE_VERSION
            || self.identity_generation == 0
            || self.capability_bits & !termirust_domain::ControllerCapabilities::KNOWN_MASK != 0
        {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        encode_control(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ListenerError> {
        let value: Self = decode_control(bytes)?;
        value.encode()?;
        Ok(value)
    }
}

fn encode_control(value: &impl Serialize) -> Result<Vec<u8>, ListenerError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
    if bytes.is_empty() || bytes.len() > MAX_PAIRING_CONTROL_BYTES {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    Ok(bytes)
}

fn decode_control<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ListenerError> {
    if bytes.is_empty() || bytes.len() > MAX_PAIRING_CONTROL_BYTES {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    serde_json::from_slice(bytes).map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))
}

fn network_error(_: ControllerNetworkError) -> ListenerError {
    ListenerError::new(ListenerErrorCode::InvalidPolicy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_controller_security::{
        CONTROLLER_V1, CapabilitySet, ControllerCapability, HostStaticPublicKey, PairingNonce,
    };
    use termirust_domain::{NetworkInterfaceId, RouteCandidate};

    fn route() -> RouteCandidate {
        RouteCandidate {
            interface_id: NetworkInterfaceId::new("4:en0").unwrap(),
            address_family: AddressFamily::Ipv4,
            address: "192.168.1.9".parse().unwrap(),
            port: ControllerPort::generated(55_555).unwrap(),
            discovery: DiscoveryPolicy::Off,
        }
    }

    fn offer() -> PairingOfferCore {
        PairingOfferCore {
            version: CONTROLLER_V1,
            expires_at_unix_seconds: 500,
            nonce: PairingNonce([7; 32]),
            host_static_public_key: HostStaticPublicKey([8; 32]),
            capabilities: CapabilitySet::default().with(ControllerCapability::ObserveSessions),
        }
    }

    #[tokio::test]
    async fn connection_purpose_is_exact_and_rejects_unknown_or_reserved_bytes() {
        let (mut client, mut server) = tokio::io::duplex(64);
        ControllerConnectionPurpose::Pair
            .write_to(&mut client)
            .await
            .unwrap();
        assert_eq!(
            ControllerConnectionPurpose::read_from(&mut server)
                .await
                .unwrap(),
            ControllerConnectionPurpose::Pair
        );

        let mut unknown = ControllerConnectionPurpose::Authenticate.encode();
        unknown[6] = 9;
        assert_eq!(
            ControllerConnectionPurpose::decode(unknown)
                .unwrap_err()
                .code,
            ListenerErrorCode::MalformedFrame
        );
        let mut reserved = ControllerConnectionPurpose::Authenticate.encode();
        reserved[7] = 1;
        assert!(ControllerConnectionPurpose::decode(reserved).is_err());
    }

    #[test]
    fn pairing_offer_round_trips_without_interface_metadata() {
        let envelope =
            ControllerPairingOffer::new(PairingOfferId::new(), &route(), &offer(), 1, 3, 5)
                .unwrap();
        let text = envelope.encode_text().unwrap();
        assert!(!text.contains("en0"));
        assert!(!text.contains("4:en0"));
        assert_eq!(
            ControllerPairingOffer::decode_text(&text).unwrap(),
            envelope
        );
        assert_eq!(envelope.offer().unwrap(), offer());
        assert_eq!(
            format!("{envelope:?}"),
            format!(
                "ControllerPairingOffer {{ schema_version: 1, offer_id: {:?}, identity_generation: 1, revocation_epoch: 3, session_generation: 5, route: \"[REDACTED]\", offer_bytes: \"[REDACTED]\" }}",
                envelope.offer_id
            )
        );
    }

    #[tokio::test]
    async fn ssh_pairing_offer_round_trips_without_a_network_route() {
        let value =
            SshControllerPairingOffer::new(PairingOfferId::new(), &offer(), 1, 3, 5).unwrap();
        let (mut client, mut server) = tokio::io::duplex(4 * 1024);
        value.write_to(&mut client).await.unwrap();
        let decoded = SshControllerPairingOffer::read_from(&mut server)
            .await
            .unwrap();
        assert_eq!(decoded, value);
        let debug = format!("{decoded:?}");
        assert!(!debug.contains(&format!("{:?}", decoded.offer_bytes)));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn pairing_offer_rejects_public_routes_and_unknown_fields() {
        let mut public = route();
        public.address = "8.8.8.8".parse().unwrap();
        assert!(
            ControllerPairingOffer::new(PairingOfferId::new(), &public, &offer(), 1, 3, 5).is_err()
        );

        let envelope =
            ControllerPairingOffer::new(PairingOfferId::new(), &route(), &offer(), 1, 3, 5)
                .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_str(&envelope.encode_text().unwrap()).unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(ControllerPairingOffer::decode_text(&value.to_string()).is_err());
    }

    #[test]
    fn device_registration_is_bounded_and_rejects_control_characters() {
        let valid =
            PairingDeviceRegistration::new(ControllerDeviceId::new(), "Jacob's iPhone".into());
        assert_eq!(
            PairingDeviceRegistration::decode(&valid.encode().unwrap()).unwrap(),
            valid
        );
        assert!(
            PairingDeviceRegistration::new(ControllerDeviceId::new(), "bad\nname".into())
                .encode()
                .is_err()
        );
        assert!(
            PairingDeviceRegistration::new(ControllerDeviceId::new(), "x".repeat(65))
                .encode()
                .is_err()
        );
    }
}
