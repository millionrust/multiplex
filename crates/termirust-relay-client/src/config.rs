use std::fmt;
use termirust_relay_protocol::{RelayEndpointRole, RelayRevocationEpoch, RelayRouteId};
use url::Url;

use crate::{RelayRouteError, RelayRouteErrorCode};

const MAX_OPAQUE_ID_BYTES: usize = 128;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Eq, PartialEq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, RelayRouteError> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_OPAQUE_ID_BYTES
                    || !value.bytes().all(|byte| byte.is_ascii_graphic())
                {
                    return Err(RelayRouteError::new(RelayRouteErrorCode::InvalidConfig));
                }
                Ok(Self(value))
            }

            pub fn expose_for_store(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

opaque_id!(RelayEndpointId);
opaque_id!(RelayCredentialRef);

#[derive(Clone, Eq, PartialEq)]
pub struct RelayWssUrl(Url);

impl RelayWssUrl {
    pub fn parse(value: &str) -> Result<Self, RelayRouteError> {
        let url = Url::parse(value)
            .map_err(|_| RelayRouteError::new(RelayRouteErrorCode::InvalidConfig))?;
        if url.scheme() != "wss"
            || url.host_str().is_none()
            || url.path() != "/relay/v1"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(RelayRouteError::new(RelayRouteErrorCode::InvalidConfig));
        }
        Ok(Self(url))
    }

    pub(crate) fn expose_for_connection(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for RelayWssUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayWssUrl([REDACTED])")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RelaySpkiPin(pub [u8; 32]);

impl fmt::Debug for RelaySpkiPin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelaySpkiPin([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RelayDeviceId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayRouteBinding {
    pub host_identity_generation: u64,
    pub device_id: RelayDeviceId,
    pub relay_epoch: RelayRevocationEpoch,
}

impl RelayRouteBinding {
    pub fn validate(self) -> Result<Self, RelayRouteError> {
        if self.host_identity_generation == 0 || self.device_id.0 == [0; 16] {
            Err(RelayRouteError::new(RelayRouteErrorCode::InvalidConfig))
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayEndpointConfig {
    pub endpoint_id: RelayEndpointId,
    pub wss_url: RelayWssUrl,
    pub route_id: RelayRouteId,
    pub credential_ref: RelayCredentialRef,
    pub expected_spki_pin: RelaySpkiPin,
    pub binding: Option<RelayRouteBinding>,
    pub relay_epoch: RelayRevocationEpoch,
}

impl RelayEndpointConfig {
    pub fn new(
        endpoint_id: RelayEndpointId,
        wss_url: RelayWssUrl,
        route_id: RelayRouteId,
        credential_ref: RelayCredentialRef,
        expected_spki_pin: RelaySpkiPin,
        binding: RelayRouteBinding,
    ) -> Result<Self, RelayRouteError> {
        if route_id.0 == [0; 32] {
            return Err(RelayRouteError::new(RelayRouteErrorCode::InvalidConfig));
        }
        Ok(Self {
            endpoint_id,
            wss_url,
            route_id,
            credential_ref,
            expected_spki_pin,
            binding: Some(binding.validate()?),
            relay_epoch: binding.relay_epoch,
        })
    }

    pub fn new_host(
        endpoint_id: RelayEndpointId,
        wss_url: RelayWssUrl,
        route_id: RelayRouteId,
        credential_ref: RelayCredentialRef,
        expected_spki_pin: RelaySpkiPin,
        relay_epoch: RelayRevocationEpoch,
    ) -> Result<Self, RelayRouteError> {
        if route_id.0 == [0; 32] {
            return Err(RelayRouteError::new(RelayRouteErrorCode::InvalidConfig));
        }
        Ok(Self {
            endpoint_id,
            wss_url,
            route_id,
            credential_ref,
            expected_spki_pin,
            binding: None,
            relay_epoch,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayClientRole {
    Host,
    DesktopController,
}

impl RelayClientRole {
    pub(crate) fn protocol_role(self) -> RelayEndpointRole {
        match self {
            Self::Host => RelayEndpointRole::Host,
            Self::DesktopController => RelayEndpointRole::Controller,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayClientState {
    Disabled,
    Connecting,
    TlsAuthenticating,
    Admitting,
    WaitingPeer,
    AuthenticatingController,
    Ready,
    Reconnecting,
    Revoked,
    CredentialLost,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RelayAttemptId(pub u64);
