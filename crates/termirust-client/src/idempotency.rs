use std::collections::VecDeque;
use std::time::{Duration, Instant};

use termirust_domain::CommandId;
use termirust_host_protocol::{IDEMPOTENCY_TTL_SECONDS, MAX_IDEMPOTENCY_OUTCOMES};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdempotencyDecision {
    Apply,
    Replay { applied: bool },
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Outcome {
    command_id: CommandId,
    payload_hash: u32,
    applied: bool,
    inserted_at: Instant,
}

#[derive(Debug)]
pub struct IdempotencyCache {
    outcomes: VecDeque<Outcome>,
    capacity: usize,
    ttl: Duration,
}

impl Default for IdempotencyCache {
    fn default() -> Self {
        Self::new(
            MAX_IDEMPOTENCY_OUTCOMES,
            Duration::from_secs(IDEMPOTENCY_TTL_SECONDS),
        )
    }
}

impl IdempotencyCache {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            outcomes: VecDeque::with_capacity(capacity.min(MAX_IDEMPOTENCY_OUTCOMES)),
            capacity: capacity.clamp(1, MAX_IDEMPOTENCY_OUTCOMES),
            ttl: ttl.min(Duration::from_secs(IDEMPOTENCY_TTL_SECONDS)),
        }
    }

    pub fn inspect(
        &mut self,
        command_id: CommandId,
        payload_hash: u32,
        now: Instant,
    ) -> IdempotencyDecision {
        self.expire(now);
        match self
            .outcomes
            .iter()
            .find(|outcome| outcome.command_id == command_id)
        {
            Some(outcome) if outcome.payload_hash == payload_hash => IdempotencyDecision::Replay {
                applied: outcome.applied,
            },
            Some(_) => IdempotencyDecision::Conflict,
            None => IdempotencyDecision::Apply,
        }
    }

    pub fn record(
        &mut self,
        command_id: CommandId,
        payload_hash: u32,
        applied: bool,
        now: Instant,
    ) {
        self.expire(now);
        if self.outcomes.len() == self.capacity {
            self.outcomes.pop_front();
        }
        self.outcomes.push_back(Outcome {
            command_id,
            payload_hash,
            applied,
            inserted_at: now,
        });
    }

    pub fn len(&self) -> usize {
        self.outcomes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.outcomes.is_empty()
    }

    fn expire(&mut self, now: Instant) {
        while self
            .outcomes
            .front()
            .is_some_and(|outcome| now.saturating_duration_since(outcome.inserted_at) >= self.ttl)
        {
            self.outcomes.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn command(value: u128) -> CommandId {
        CommandId::from_uuid(Uuid::from_u128(value))
    }

    #[test]
    fn cache_replays_identical_rejects_conflict_and_expires() {
        let start = Instant::now();
        let mut cache = IdempotencyCache::new(2, Duration::from_secs(10));
        assert_eq!(
            cache.inspect(command(1), 10, start),
            IdempotencyDecision::Apply
        );
        cache.record(command(1), 10, true, start);
        assert_eq!(
            cache.inspect(command(1), 10, start),
            IdempotencyDecision::Replay { applied: true }
        );
        assert_eq!(
            cache.inspect(command(1), 11, start),
            IdempotencyDecision::Conflict
        );
        assert_eq!(
            cache.inspect(command(1), 11, start + Duration::from_secs(10)),
            IdempotencyDecision::Apply
        );
    }

    #[test]
    fn cache_never_exceeds_capacity() {
        let now = Instant::now();
        let mut cache = IdempotencyCache::new(2, Duration::from_secs(60));
        for value in 0..10 {
            cache.record(command(value), value as u32, true, now);
        }
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn cache_accepts_exact_protocol_maximum() {
        let now = Instant::now();
        let mut cache = IdempotencyCache::default();
        for value in 0..MAX_IDEMPOTENCY_OUTCOMES {
            cache.record(command(value as u128), value as u32, true, now);
        }
        assert_eq!(cache.len(), MAX_IDEMPOTENCY_OUTCOMES);
        cache.record(command(u128::MAX), u32::MAX, true, now);
        assert_eq!(cache.len(), MAX_IDEMPOTENCY_OUTCOMES);
    }
}
