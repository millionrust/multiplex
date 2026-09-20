use multiplex_domain::ListeningAddress;

use crate::{ListenerError, ListenerErrorCode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirewallObservation {
    Allowed,
    Blocked,
    Unknown,
}

pub trait FirewallObserver: Send + Sync {
    fn observe(&self, addresses: &[ListeningAddress])
    -> Result<FirewallObservation, ListenerError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemFirewallObserver;

impl FirewallObserver for SystemFirewallObserver {
    fn observe(
        &self,
        addresses: &[ListeningAddress],
    ) -> Result<FirewallObservation, ListenerError> {
        if addresses.iter().any(|address| address.validate().is_err()) {
            return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
        }

        // A successful local bind cannot prove inbound reachability. Querying or changing
        // platform firewall state would require broader authority, so v1 reports Unknown.
        Ok(FirewallObservation::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use multiplex_domain::{NetworkInterfaceId, NetworkInterfaceKind};

    use super::*;

    #[test]
    fn system_observation_is_read_only_unknown_for_private_addresses() {
        let address = |value: &str| ListeningAddress {
            interface_id: NetworkInterfaceId::new("4:en0").unwrap(),
            label: "en0".into(),
            kind: NetworkInterfaceKind::Lan,
            address: value.parse().unwrap(),
        };
        assert_eq!(
            SystemFirewallObserver
                .observe(&[address("192.168.1.20:55555"), address("100.81.1.2:55555")])
                .unwrap(),
            FirewallObservation::Unknown
        );
        assert!(
            SystemFirewallObserver
                .observe(&[address("0.0.0.0:55555")])
                .is_err()
        );
    }
}
