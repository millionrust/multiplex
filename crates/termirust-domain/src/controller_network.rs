use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ControllerCapabilities, ControllerDeviceId, DevicePublicKey, HostIdentityGeneration};

pub const GENERATED_PORT_MIN: u16 = 49_152;
pub const USER_FIXED_PORT_MIN: u16 = 1_024;
pub const MAX_GENERATED_PORT_ATTEMPTS: usize = 16;
pub const MAX_NETWORK_INTERFACE_ID_BYTES: usize = 128;
pub const MAX_NETWORK_INTERFACE_LABEL_SCALARS: usize = 96;

#[derive(Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NetworkInterfaceId(String);

impl NetworkInterfaceId {
    pub fn new(value: impl Into<String>) -> Result<Self, ControllerNetworkError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_NETWORK_INTERFACE_ID_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(ControllerNetworkError::InvalidInterfaceId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NetworkInterfaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NetworkInterfaceId([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

impl AddressFamily {
    pub const fn matches(self, address: IpAddr) -> bool {
        matches!(
            (self, address),
            (Self::Ipv4, IpAddr::V4(_)) | (Self::Ipv6, IpAddr::V6(_))
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkInterfaceKind {
    Lan,
    Vpn,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkInterfaceCandidate {
    pub id: NetworkInterfaceId,
    pub label: String,
    pub kind: NetworkInterfaceKind,
    pub address_family: AddressFamily,
    pub address: IpAddr,
}

impl NetworkInterfaceCandidate {
    pub fn validate(&self) -> Result<(), ControllerNetworkError> {
        NetworkInterfaceId::new(self.id.as_str())?;
        if self.label.is_empty()
            || self.label.chars().count() > MAX_NETWORK_INTERFACE_LABEL_SCALARS
            || self.label.chars().any(char::is_control)
        {
            return Err(ControllerNetworkError::InvalidInterfaceLabel);
        }
        validate_private_address(self.address_family, self.address)
    }
}

impl fmt::Debug for NetworkInterfaceCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkInterfaceCandidate")
            .field("id", &self.id)
            .field("label", &"[REDACTED]")
            .field("kind", &self.kind)
            .field("address_family", &self.address_family)
            .field("address", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPort {
    Generated(u16),
    UserFixed(u16),
}

impl ControllerPort {
    pub fn generated(value: u16) -> Result<Self, ControllerNetworkError> {
        let port = Self::Generated(value);
        port.validate()?;
        Ok(port)
    }

    pub fn user_fixed(value: u16) -> Result<Self, ControllerNetworkError> {
        let port = Self::UserFixed(value);
        port.validate()?;
        Ok(port)
    }

    pub const fn value(self) -> u16 {
        match self {
            Self::Generated(value) | Self::UserFixed(value) => value,
        }
    }

    pub fn validate(self) -> Result<(), ControllerNetworkError> {
        match self {
            Self::Generated(GENERATED_PORT_MIN..=u16::MAX)
            | Self::UserFixed(USER_FIXED_PORT_MIN..=u16::MAX) => Ok(()),
            Self::Generated(_) => Err(ControllerNetworkError::InvalidGeneratedPort),
            Self::UserFixed(_) => Err(ControllerNetworkError::InvalidFixedPort),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryPolicy {
    #[default]
    Off,
}

/// Whether and on which port the Controller listener runs. When enabled it listens on every
/// eligible private address the computer has, so there is no network to choose.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(from = "StoredListenPolicy")]
pub struct ControllerListenPolicy {
    pub enabled: bool,
    pub port: Option<ControllerPort>,
    pub discovery: DiscoveryPolicy,
}

impl ControllerListenPolicy {
    pub fn validate(&self) -> Result<(), ControllerNetworkError> {
        match (self.enabled, self.port) {
            (_, Some(port)) => port.validate(),
            (false, None) => Ok(()),
            (true, None) => Err(ControllerNetworkError::IncompletePolicy),
        }
    }

    /// The port to listen on, or `None` when the listener is off.
    pub fn listening_port(&self) -> Result<Option<ControllerPort>, ControllerNetworkError> {
        self.validate()?;
        Ok(self.port.filter(|_| self.enabled))
    }
}

/// The saved form, which still accepts the exact-network fields written before the listener
/// used every private address. Those fields are ignored and dropped on the next save.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StoredListenPolicy {
    enabled: bool,
    port: Option<ControllerPort>,
    discovery: DiscoveryPolicy,
    #[serde(rename = "interface_id")]
    _interface_id: Option<NetworkInterfaceId>,
    #[serde(rename = "address_family")]
    _address_family: Option<AddressFamily>,
    #[serde(rename = "selected_address")]
    _selected_address: Option<IpAddr>,
}

impl From<StoredListenPolicy> for ControllerListenPolicy {
    fn from(stored: StoredListenPolicy) -> Self {
        Self {
            enabled: stored.enabled,
            port: stored.port,
            discovery: stored.discovery,
        }
    }
}

/// One address the listener is accepting connections on.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListeningAddress {
    pub interface_id: NetworkInterfaceId,
    pub label: String,
    pub kind: NetworkInterfaceKind,
    pub address: SocketAddr,
}

impl ListeningAddress {
    pub fn validate(&self) -> Result<(), ControllerNetworkError> {
        NetworkInterfaceCandidate {
            id: self.interface_id.clone(),
            label: self.label.clone(),
            kind: self.kind,
            address_family: if self.address.is_ipv4() {
                AddressFamily::Ipv4
            } else {
                AddressFamily::Ipv6
            },
            address: self.address.ip(),
        }
        .validate()?;
        ControllerPort::user_fixed(self.address.port()).map(|_| ())
    }
}

impl fmt::Debug for ListeningAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ListeningAddress")
            .field("interface_id", &self.interface_id)
            .field("label", &"[REDACTED]")
            .field("kind", &self.kind)
            .field("address", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ListenerInstanceId(Uuid);

impl ListenerInstanceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ListenerInstanceId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenerFailureCode {
    NoEligibleInterface,
    PermissionDenied,
    PortConflict,
    FirewallBlocked,
    InterfaceGone,
    IdentityUnavailable,
    RateLimited,
    ConnectionLimit,
    Protocol,
    Internal,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum ListenerState {
    #[default]
    Disabled,
    Binding,
    Ready {
        authenticated_connections: u16,
    },
    InterfaceGone,
    PortConflict,
    FirewallBlocked,
    Failed(ListenerFailureCode),
    ShuttingDown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedPeer {
    pub device_id: ControllerDeviceId,
    pub public_key: DevicePublicKey,
    pub identity_generation: HostIdentityGeneration,
    pub revocation_epoch: u64,
    pub capabilities: ControllerCapabilities,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionBudget {
    pub max_unauthenticated: usize,
    pub max_authenticated_per_host: usize,
    pub handshake_timeout_seconds: u64,
    pub max_control_frame_bytes: usize,
    pub max_terminal_frame_bytes: usize,
    pub max_queue_frames: usize,
    pub max_queue_payload_bytes: usize,
    pub failed_auth_attempts: usize,
    pub failed_auth_window_seconds: u64,
}

impl Default for ConnectionBudget {
    fn default() -> Self {
        Self {
            max_unauthenticated: 4,
            max_authenticated_per_host: 16,
            handshake_timeout_seconds: 30,
            max_control_frame_bytes: 64 * 1024,
            max_terminal_frame_bytes: 1024 * 1024,
            max_queue_frames: 64,
            max_queue_payload_bytes: 4 * 1024 * 1024,
            failed_auth_attempts: 5,
            failed_auth_window_seconds: 10 * 60,
        }
    }
}

impl ConnectionBudget {
    pub fn validate(self) -> Result<(), ControllerNetworkError> {
        if self == Self::default() {
            Ok(())
        } else {
            Err(ControllerNetworkError::InvalidConnectionBudget)
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct ControllerNetworkRevision(u64);

impl ControllerNetworkRevision {
    pub const ZERO: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerNetworkError {
    InvalidInterfaceId,
    InvalidInterfaceLabel,
    InvalidAddress,
    AddressFamilyMismatch,
    InvalidGeneratedPort,
    InvalidFixedPort,
    IncompletePolicy,
    InvalidConnectionBudget,
}

impl fmt::Display for ControllerNetworkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInterfaceId => "controller interface identifier is invalid",
            Self::InvalidInterfaceLabel => "controller interface label is invalid",
            Self::InvalidAddress => "controller listener address is not private and eligible",
            Self::AddressFamilyMismatch => "controller listener address family does not match",
            Self::InvalidGeneratedPort => "generated controller port must be 49152 through 65535",
            Self::InvalidFixedPort => "fixed controller port must be 1024 through 65535",
            Self::IncompletePolicy => "controller listener policy is incomplete",
            Self::InvalidConnectionBudget => {
                "controller connection budget is not the bounded v1 policy"
            }
        })
    }
}

impl std::error::Error for ControllerNetworkError {}

pub fn is_private_controller_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_private_ipv4(address),
        IpAddr::V6(address) => is_private_ipv6(address),
    }
}

fn validate_private_address(
    family: AddressFamily,
    address: IpAddr,
) -> Result<(), ControllerNetworkError> {
    if !family.matches(address) {
        return Err(ControllerNetworkError::AddressFamilyMismatch);
    }
    if !is_private_controller_address(address) {
        return Err(ControllerNetworkError::InvalidAddress);
    }
    Ok(())
}

fn is_private_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    (address.is_private() || (octets[0] == 100 && (64..=127).contains(&octets[1])))
        && !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_broadcast()
        && !address.is_multicast()
}

fn is_private_ipv6(address: Ipv6Addr) -> bool {
    (address.segments()[0] & 0xfe00) == 0xfc00
        && !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_disabled_and_has_no_port() {
        let policy = ControllerListenPolicy::default();
        assert_eq!(policy.listening_port().unwrap(), None);
        assert_eq!(policy.discovery, DiscoveryPolicy::Off);
    }

    #[test]
    fn enabled_policy_needs_a_valid_port_and_a_stopped_policy_keeps_it() {
        let enabled = ControllerListenPolicy {
            enabled: true,
            port: Some(ControllerPort::generated(55_000).unwrap()),
            discovery: DiscoveryPolicy::Off,
        };
        assert_eq!(enabled.listening_port().unwrap(), enabled.port);
        let stopped = ControllerListenPolicy {
            enabled: false,
            ..enabled.clone()
        };
        assert_eq!(stopped.listening_port().unwrap(), None);
        assert_eq!(
            ControllerListenPolicy {
                enabled: true,
                ..ControllerListenPolicy::default()
            }
            .validate(),
            Err(ControllerNetworkError::IncompletePolicy)
        );
        assert!(
            ControllerListenPolicy {
                port: Some(ControllerPort::Generated(80)),
                ..enabled
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn exact_network_policies_saved_by_older_versions_still_load() {
        let stored = r#"{"enabled":true,"interface_id":"17:en0","address_family":"ipv4","selected_address":"192.168.1.8","port":{"generated":55000},"discovery":"off"}"#;
        let policy: ControllerListenPolicy = serde_json::from_str(stored).unwrap();
        assert_eq!(
            policy,
            ControllerListenPolicy {
                enabled: true,
                port: Some(ControllerPort::Generated(55_000)),
                discovery: DiscoveryPolicy::Off,
            }
        );
        assert_eq!(
            serde_json::to_string(&policy).unwrap(),
            r#"{"enabled":true,"port":{"generated":55000},"discovery":"off"}"#
        );
        assert!(
            serde_json::from_str::<ControllerListenPolicy>(r#"{"enabled":false,"extra":1}"#)
                .is_err()
        );
    }

    #[test]
    fn listening_addresses_must_be_private_with_a_real_port() {
        let address = |value: &str| ListeningAddress {
            interface_id: NetworkInterfaceId::new("17:en0").unwrap(),
            label: "en0".into(),
            kind: NetworkInterfaceKind::Lan,
            address: value.parse().unwrap(),
        };
        for valid in [
            "192.168.1.8:55000",
            "100.81.253.53:55000",
            "[fd7a:115c:a1e0::1]:55000",
        ] {
            assert!(address(valid).validate().is_ok(), "rejected {valid}");
        }
        for invalid in [
            "0.0.0.0:55000",
            "127.0.0.1:55000",
            "8.8.8.8:55000",
            "169.254.1.1:55000",
            "[::]:55000",
            "[::1]:55000",
            "[fe80::1]:55000",
            "[2001:4860:4860::8888]:55000",
            "192.168.1.8:80",
        ] {
            assert!(address(invalid).validate().is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn ports_and_connection_budgets_are_exactly_bounded() {
        assert!(ControllerPort::generated(GENERATED_PORT_MIN).is_ok());
        assert!(ControllerPort::generated(GENERATED_PORT_MIN - 1).is_err());
        assert!(ControllerPort::user_fixed(USER_FIXED_PORT_MIN).is_ok());
        assert!(ControllerPort::user_fixed(USER_FIXED_PORT_MIN - 1).is_err());
        assert!(ConnectionBudget::default().validate().is_ok());
        assert!(
            ConnectionBudget {
                max_unauthenticated: 5,
                ..ConnectionBudget::default()
            }
            .validate()
            .is_err()
        );
    }
}
