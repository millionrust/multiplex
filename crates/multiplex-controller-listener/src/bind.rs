use std::io;
use std::net::{SocketAddr, TcpListener};

use multiplex_domain::{
    ControllerListenPolicy, ControllerPort, GENERATED_PORT_MIN, ListeningAddress,
    MAX_GENERATED_PORT_ATTEMPTS, NetworkInterfaceCandidate,
};
use rand::Rng as _;

use crate::{InterfaceProvider, ListenerError, ListenerErrorCode};

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

/// One bound socket and the private address it accepts on.
pub struct BoundAddress<L> {
    pub listener: L,
    pub address: ListeningAddress,
}

/// Every private address the listener could bind, all on one port.
pub struct BoundControllerListeners<L> {
    pub port: ControllerPort,
    pub bound: Vec<BoundAddress<L>>,
}

impl<L> BoundControllerListeners<L> {
    pub fn addresses(&self) -> Vec<ListeningAddress> {
        self.bound
            .iter()
            .map(|bound| bound.address.clone())
            .collect()
    }
}

impl<L> std::fmt::Debug for BoundControllerListeners<L> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundControllerListeners")
            .field("port", &self.port)
            .field("bound", &self.bound.len())
            .finish()
    }
}

/// Binds every eligible private address on the policy's port.
///
/// Addresses that cannot take the port are skipped; the runtime retries them as networks
/// change. A generated port is replaced only when no address at all can use it, and a
/// computer with no private network yet binds nothing and waits.
pub fn bind_private_addresses<P, B, R>(
    policy: &ControllerListenPolicy,
    interfaces: &P,
    binder: &B,
    ports: &mut R,
) -> Result<BoundControllerListeners<B::Listener>, ListenerError>
where
    P: InterfaceProvider,
    B: ControllerBinder,
    R: GeneratedPortSource,
{
    let requested = policy
        .listening_port()?
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::Disabled))?;
    let candidates = interfaces.eligible_interfaces()?;
    if candidates.is_empty() {
        return Ok(BoundControllerListeners {
            port: requested,
            bound: Vec::new(),
        });
    }
    let attempts = match requested {
        ControllerPort::UserFixed(_) => 1,
        ControllerPort::Generated(_) => MAX_GENERATED_PORT_ATTEMPTS,
    };
    let mut port = requested;
    let mut last_error = None;
    for attempt in 0..attempts {
        if attempt > 0 {
            port = ControllerPort::generated(ports.next_port()?)?;
        }
        let mut bound = Vec::new();
        let mut another_port_may_work = true;
        for candidate in &candidates {
            match bind_address(candidate, port, binder) {
                Ok(address) => bound.push(address),
                Err(error) => {
                    another_port_may_work &= port_refused(error.code);
                    last_error = Some(error);
                }
            }
        }
        if !bound.is_empty() {
            return Ok(BoundControllerListeners { port, bound });
        }
        if !another_port_may_work {
            break;
        }
    }
    Err(last_error.unwrap_or_else(|| ListenerError::new(ListenerErrorCode::PortConflict)))
}

/// Whether a generated port was refused for being that port, so a different one may bind.
///
/// Windows reserves blocks of the dynamic range this port is drawn from for Hyper-V, WSL, and
/// Docker (`netsh interface ipv4 show excludedportrange protocol=tcp`), and binding inside one
/// fails with WSAEACCES, which reads as permission denied rather than as a port in use. A port the
/// person chose is never replaced, so this only decides whether to draw another.
fn port_refused(code: ListenerErrorCode) -> bool {
    matches!(
        code,
        ListenerErrorCode::PortConflict | ListenerErrorCode::PermissionDenied
    )
}

/// Binds one eligible private address on `port`.
pub fn bind_address<B: ControllerBinder + ?Sized>(
    candidate: &NetworkInterfaceCandidate,
    port: ControllerPort,
    binder: &B,
) -> Result<BoundAddress<B::Listener>, ListenerError> {
    candidate.validate()?;
    port.validate()?;
    let address = ListeningAddress {
        interface_id: candidate.id.clone(),
        label: candidate.label.clone(),
        kind: candidate.kind,
        address: SocketAddr::new(candidate.address, port.value()),
    };
    let listener = binder
        .bind_exact(address.address)
        .map_err(crate::error::bind_error)?;
    Ok(BoundAddress { listener, address })
}
