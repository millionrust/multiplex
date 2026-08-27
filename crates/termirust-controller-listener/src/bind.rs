use std::io;
use std::net::{SocketAddr, TcpListener};

use rand::Rng as _;
use termirust_domain::{
    ControllerListenPolicy, ControllerPort, DiscoveryPolicy, GENERATED_PORT_MIN,
    MAX_GENERATED_PORT_ATTEMPTS, NetworkInterfaceCandidate, NetworkInterfaceId,
};

use crate::{InterfaceProvider, ListenerError, ListenerErrorCode, resolve_selected_interface};

pub trait ControllerBinder: Send + Sync {
    type Listener;

    fn bind_exact(&self, address: SocketAddr) -> io::Result<Self::Listener>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemBinder;

impl ControllerBinder for SystemBinder {
    type Listener = TcpListener;

    fn bind_exact(&self, address: SocketAddr) -> io::Result<Self::Listener> {
        if address.ip().is_unspecified() || address.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "wildcard and loopback controller binds are forbidden",
            ));
        }
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        Ok(listener)
    }
}

pub trait GeneratedPortSource {
    fn next_port(&mut self) -> Result<u16, ListenerError>;
}

#[derive(Debug, Default)]
pub struct SystemGeneratedPortSource;

impl GeneratedPortSource for SystemGeneratedPortSource {
    fn next_port(&mut self) -> Result<u16, ListenerError> {
        Ok(rand::rngs::OsRng.gen_range(GENERATED_PORT_MIN..=u16::MAX))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundRoute {
    pub interface_id: NetworkInterfaceId,
    pub address: SocketAddr,
    pub port: ControllerPort,
    pub discovery: DiscoveryPolicy,
}

pub struct BoundControllerListener<L> {
    pub listener: L,
    pub route: BoundRoute,
}

impl<L> std::fmt::Debug for BoundControllerListener<L> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundControllerListener")
            .field("listener", &"[OPAQUE]")
            .field("route", &"[REDACTED]")
            .finish()
    }
}

pub fn bind_selected_route<P, B, R>(
    policy: &ControllerListenPolicy,
    interfaces: &P,
    binder: &B,
    ports: &mut R,
) -> Result<BoundControllerListener<B::Listener>, ListenerError>
where
    P: InterfaceProvider,
    B: ControllerBinder,
    R: GeneratedPortSource,
{
    policy.validate()?;
    if !policy.enabled {
        return Err(ListenerError::new(ListenerErrorCode::Disabled));
    }
    let candidates = interfaces.eligible_interfaces()?;
    if candidates.is_empty() {
        return Err(ListenerError::new(ListenerErrorCode::NoEligibleInterface));
    }
    let selected = resolve_selected_interface(policy, &candidates)?;
    let requested_port = policy
        .port
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::InvalidPolicy))?;
    match requested_port {
        ControllerPort::UserFixed(port) => bind_one(selected, requested_port, port, binder),
        ControllerPort::Generated(persisted_port) => {
            let mut last_conflict = None;
            for attempt in 0..MAX_GENERATED_PORT_ATTEMPTS {
                let port = if attempt == 0 {
                    persisted_port
                } else {
                    ports.next_port()?
                };
                ControllerPort::generated(port)?;
                match bind_one(
                    selected.clone(),
                    ControllerPort::Generated(port),
                    port,
                    binder,
                ) {
                    Ok(bound) => return Ok(bound),
                    Err(error) if error.code == ListenerErrorCode::PortConflict => {
                        last_conflict = Some(error);
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(last_conflict
                .unwrap_or_else(|| ListenerError::new(ListenerErrorCode::PortConflict)))
        }
    }
}

fn bind_one<B: ControllerBinder>(
    selected: NetworkInterfaceCandidate,
    port: ControllerPort,
    port_number: u16,
    binder: &B,
) -> Result<BoundControllerListener<B::Listener>, ListenerError> {
    let address = SocketAddr::new(selected.address, port_number);
    let listener = binder
        .bind_exact(address)
        .map_err(crate::error::bind_error)?;
    Ok(BoundControllerListener {
        listener,
        route: BoundRoute {
            interface_id: selected.id,
            address,
            port,
            discovery: DiscoveryPolicy::Off,
        },
    })
}
