//! Bonjour advertisement, so a phone on the same Wi-Fi or Ethernet network can find the
//! computer without typing its address.
//!
//! Only LAN addresses are announced; VPN interfaces such as Tailscale are never used, because
//! multicast does not cross them and announcing into a VPN would reach networks the user did
//! not mean to. The announcement names the service by an opaque identifier derived from the Host
//! fingerprint, never by the computer's name, and carries no secret: finding the computer is
//! not trusting it, and pairing still needs the code or offer.

use std::net::IpAddr;

use termirust_domain::{HostFingerprint, HostPublicKey, ListeningAddress, NetworkInterfaceKind};

use crate::{ListenerError, ListenerErrorCode};

pub const BONJOUR_SERVICE_TYPE: &str = "_termirust._tcp.local.";
const DISCOVERY_ID_BYTES: usize = 16;

/// What one announcement publishes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BonjourAdvertisement {
    pub instance: String,
    pub host_name: String,
    pub interfaces: Vec<String>,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub properties: Vec<(String, String)>,
}

/// The opaque identifier a phone matches a saved computer against: the first 16 bytes of the
/// Host fingerprint, in lowercase hex.
pub fn discovery_id(host: HostPublicKey) -> String {
    HostFingerprint::derive(host).digest()[..DISCOVERY_ID_BYTES]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The advertisement for the LAN addresses among `addresses`, or `None` when there are none.
pub fn bonjour_advertisement(
    host: HostPublicKey,
    addresses: &[ListeningAddress],
) -> Option<BonjourAdvertisement> {
    let lan = addresses
        .iter()
        .filter(|address| address.kind == NetworkInterfaceKind::Lan && address.validate().is_ok())
        .collect::<Vec<_>>();
    let port = lan.first()?.address.port();
    let id = discovery_id(host);
    let mut interfaces = lan
        .iter()
        .map(|address| address.label.clone())
        .collect::<Vec<_>>();
    interfaces.sort();
    interfaces.dedup();
    Some(BonjourAdvertisement {
        instance: format!("TermiRust {}", id[..6].to_ascii_uppercase()),
        host_name: format!("termirust-{id}.local."),
        interfaces,
        addresses: lan
            .iter()
            .filter(|address| address.address.port() == port)
            .map(|address| address.address.ip())
            .collect(),
        port,
        properties: vec![("v".into(), "1".into()), ("id".into(), id)],
    })
}

/// A running Bonjour announcement that follows the listener's addresses.
pub struct BonjourAnnouncement {
    daemon: mdns_sd::ServiceDaemon,
    host: HostPublicKey,
    published: Option<(String, BonjourAdvertisement)>,
}

impl std::fmt::Debug for BonjourAnnouncement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BonjourAnnouncement")
            .field("published", &self.published.is_some())
            .finish()
    }
}

impl BonjourAnnouncement {
    pub fn start(host: HostPublicKey) -> Result<Self, ListenerError> {
        let daemon =
            mdns_sd::ServiceDaemon::new().map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        daemon
            .disable_interface(mdns_sd::IfKind::All)
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        Ok(Self {
            daemon,
            host,
            published: None,
        })
    }

    /// Announces the LAN addresses the listener accepts on, replacing any earlier announcement.
    pub fn update(&mut self, addresses: &[ListeningAddress]) -> Result<(), ListenerError> {
        let next = bonjour_advertisement(self.host, addresses);
        if self.published.as_ref().map(|(_, current)| current) == next.as_ref() {
            return Ok(());
        }
        if let Some((fullname, current)) = self.published.take() {
            let _ = self.daemon.unregister(&fullname);
            for interface in current.interfaces {
                let _ = self
                    .daemon
                    .disable_interface(mdns_sd::IfKind::Name(interface));
            }
        }
        let Some(advertisement) = next else {
            return Ok(());
        };
        for interface in &advertisement.interfaces {
            self.daemon
                .enable_interface(mdns_sd::IfKind::Name(interface.clone()))
                .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        }
        let service = mdns_sd::ServiceInfo::new(
            BONJOUR_SERVICE_TYPE,
            &advertisement.instance,
            &advertisement.host_name,
            advertisement.addresses.as_slice(),
            advertisement.port,
            advertisement
                .properties
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>()
                .as_slice(),
        )
        .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        let fullname = service.get_fullname().to_owned();
        self.daemon
            .register(service)
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        self.published = Some((fullname, advertisement));
        Ok(())
    }
}

impl Drop for BonjourAnnouncement {
    fn drop(&mut self) {
        if let Some((fullname, _)) = self.published.take() {
            let _ = self.daemon.unregister(&fullname);
        }
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use termirust_domain::NetworkInterfaceId;

    use super::*;

    fn address(label: &str, kind: NetworkInterfaceKind, value: &str) -> ListeningAddress {
        ListeningAddress {
            interface_id: NetworkInterfaceId::new(format!("1:{label}")).unwrap(),
            label: label.into(),
            kind,
            address: value.parse().unwrap(),
        }
    }

    #[test]
    fn only_lan_addresses_are_announced_under_an_opaque_name() {
        let host = HostPublicKey([7; 32]);
        let advertisement = bonjour_advertisement(
            host,
            &[
                address("en0", NetworkInterfaceKind::Lan, "192.168.88.4:55000"),
                address("en0", NetworkInterfaceKind::Lan, "[fd00::4]:55000"),
                address("utun4", NetworkInterfaceKind::Vpn, "100.81.253.53:55000"),
            ],
        )
        .unwrap();
        let id = discovery_id(host);
        assert_eq!(id.len(), 32);
        assert_eq!(advertisement.interfaces, ["en0"]);
        assert_eq!(
            advertisement.addresses,
            [
                "192.168.88.4".parse::<IpAddr>().unwrap(),
                "fd00::4".parse().unwrap()
            ]
        );
        assert_eq!(advertisement.port, 55_000);
        assert_eq!(advertisement.host_name, format!("termirust-{id}.local."));
        assert!(advertisement.instance.starts_with("TermiRust "));
        assert_eq!(
            advertisement.properties,
            [("v".to_owned(), "1".to_owned()), ("id".to_owned(), id)]
        );
    }

    #[test]
    fn a_vpn_only_computer_announces_nothing() {
        assert_eq!(
            bonjour_advertisement(
                HostPublicKey([7; 32]),
                &[address(
                    "utun4",
                    NetworkInterfaceKind::Vpn,
                    "100.81.253.53:55000"
                )]
            ),
            None
        );
    }

    #[test]
    fn the_identifier_changes_with_the_host_key() {
        assert_ne!(
            discovery_id(HostPublicKey([7; 32])),
            discovery_id(HostPublicKey([8; 32]))
        );
    }
}
