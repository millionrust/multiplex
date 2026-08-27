use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::net::IpAddr;

use sha2::{Digest as _, Sha256};
use termirust_domain::ConnectionBudget;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{ListenerError, ListenerErrorCode};

const MAX_SOURCE_BUCKETS: usize = 1_024;
const SOURCE_BUCKET_DOMAIN: &[u8] = b"termirust-controller-source-bucket-v1\0";

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct SourceBucketKey([u8; 32]);

impl SourceBucketKey {
    pub fn from_random(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for SourceBucketKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceBucketKey([REDACTED])")
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct SourceBucket([u8; 16]);

impl SourceBucket {
    pub fn derive(key: &SourceBucketKey, address: IpAddr) -> Self {
        let mut hash = Sha256::new();
        hash.update(SOURCE_BUCKET_DOMAIN);
        hash.update(key.0);
        match address {
            IpAddr::V4(address) => hash.update(address.octets()),
            IpAddr::V6(address) => hash.update(address.octets()),
        }
        let digest = hash.finalize();
        Self(digest[..16].try_into().unwrap_or([0; 16]))
    }
}

impl fmt::Debug for SourceBucket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceBucket([REDACTED])")
    }
}

#[derive(Debug)]
pub struct AuthRateLimiter {
    failures: HashMap<SourceBucket, VecDeque<u64>>,
    budget: ConnectionBudget,
}

impl AuthRateLimiter {
    pub fn new(budget: ConnectionBudget) -> Result<Self, ListenerError> {
        budget.validate()?;
        Ok(Self {
            failures: HashMap::new(),
            budget,
        })
    }

    pub fn check(&mut self, source: SourceBucket, now_seconds: u64) -> Result<(), ListenerError> {
        self.expire(source, now_seconds);
        if self
            .failures
            .get(&source)
            .is_some_and(|timestamps| timestamps.len() >= self.budget.failed_auth_attempts)
        {
            Err(ListenerError::new(ListenerErrorCode::RateLimited))
        } else {
            Ok(())
        }
    }

    pub fn record_failure(
        &mut self,
        source: SourceBucket,
        now_seconds: u64,
    ) -> Result<(), ListenerError> {
        self.expire(source, now_seconds);
        if !self.failures.contains_key(&source) && self.failures.len() >= MAX_SOURCE_BUCKETS {
            self.evict_oldest();
        }
        self.failures
            .entry(source)
            .or_default()
            .push_back(now_seconds);
        self.check(source, now_seconds)
    }

    pub fn record_success(&mut self, source: SourceBucket) {
        self.failures.remove(&source);
    }

    fn expire(&mut self, source: SourceBucket, now_seconds: u64) {
        let window = self.budget.failed_auth_window_seconds;
        if let Some(timestamps) = self.failures.get_mut(&source) {
            while timestamps
                .front()
                .is_some_and(|timestamp| now_seconds.saturating_sub(*timestamp) >= window)
            {
                timestamps.pop_front();
            }
            if timestamps.is_empty() {
                self.failures.remove(&source);
            }
        }
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest) = self
            .failures
            .iter()
            .min_by_key(|(_, timestamps)| timestamps.back().copied().unwrap_or_default())
            .map(|(source, _)| *source)
        {
            self.failures.remove(&oldest);
        }
    }
}
