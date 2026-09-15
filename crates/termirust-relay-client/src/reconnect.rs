use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayOperationClass {
    IdempotentRead,
    Mutation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayReconnectDecision {
    RetryAfter(Duration),
    Exhausted,
    UnknownCompletion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayReconnectPolicy {
    pub max_attempts: u8,
    pub max_elapsed: Duration,
}

impl Default for RelayReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            max_elapsed: Duration::from_secs(90),
        }
    }
}

impl RelayReconnectPolicy {
    pub fn decide(
        self,
        operation: RelayOperationClass,
        completed_attempts: u8,
        elapsed: Duration,
        random_u64: u64,
    ) -> RelayReconnectDecision {
        if operation == RelayOperationClass::Mutation {
            return RelayReconnectDecision::UnknownCompletion;
        }
        if completed_attempts >= self.max_attempts || elapsed >= self.max_elapsed {
            return RelayReconnectDecision::Exhausted;
        }
        let exponent = u32::from(completed_attempts.min(6));
        let cap_millis = 250_u64.saturating_mul(1_u64 << exponent).min(10_000);
        let jitter = random_u64 % cap_millis.saturating_add(1);
        let remaining = self.max_elapsed.saturating_sub(elapsed);
        RelayReconnectDecision::RetryAfter(Duration::from_millis(jitter).min(remaining))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationDisposition {
    Pending,
    Acknowledged,
    UnknownCompletion,
}

#[derive(Default)]
pub struct MutationReconciliation<CommandId: Eq + std::hash::Hash> {
    commands: HashMap<CommandId, MutationDisposition>,
}

impl<CommandId: Copy + Eq + std::hash::Hash> MutationReconciliation<CommandId> {
    pub fn sent(&mut self, command_id: CommandId) {
        self.commands
            .insert(command_id, MutationDisposition::Pending);
    }

    pub fn acknowledged(&mut self, command_id: CommandId) {
        if let Some(status) = self.commands.get_mut(&command_id) {
            *status = MutationDisposition::Acknowledged;
        }
    }

    pub fn disconnected(&mut self) {
        for status in self.commands.values_mut() {
            if *status == MutationDisposition::Pending {
                *status = MutationDisposition::UnknownCompletion;
            }
        }
    }

    pub fn disposition(&self, command_id: CommandId) -> Option<MutationDisposition> {
        self.commands.get(&command_id).copied()
    }
}
