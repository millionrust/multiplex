use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{HostInstanceId, OutputSequence};

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ProcessToken {
    host_instance: HostInstanceId,
    platform_identity: u64,
    generation: u64,
}

impl ProcessToken {
    pub const fn new(
        host_instance: HostInstanceId,
        platform_identity: u64,
        generation: u64,
    ) -> Self {
        Self {
            host_instance,
            platform_identity,
            generation,
        }
    }

    pub const fn host_instance(self) -> HostInstanceId {
        self.host_instance
    }

    pub const fn platform_identity(self) -> u64 {
        self.platform_identity
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }

    pub fn belongs_to(self, host_instance: HostInstanceId) -> bool {
        self.host_instance == host_instance
    }
}

impl fmt::Debug for ProcessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessToken")
            .field("host_instance", &self.host_instance)
            .field("platform_identity", &"[REDACTED]")
            .field("generation", &self.generation)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostLifecycle {
    Starting,
    Ready,
    Stopping,
    Exited,
    Failed,
    Orphaned,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurabilityWatermark {
    pub sequence: OutputSequence,
    pub monotonic_nanos: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn process_token_binds_identity_to_one_host_and_redacts_platform_value() {
        let host = HostInstanceId::from_uuid(Uuid::from_u128(7));
        let token = ProcessToken::new(host, 42, 3);
        assert!(token.belongs_to(host));
        assert!(!format!("{token:?}").contains("42"));
        assert_eq!(token.generation(), 3);
    }
}
