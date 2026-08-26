use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProjectId(Uuid);

impl ProjectId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ProjectId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ManagedWorktreeId(Uuid);

impl ManagedWorktreeId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ManagedWorktreeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ManagedWorktreeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ManagedWorktreeId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct GroupId(Uuid);

impl GroupId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for GroupId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for GroupId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct HostInstanceId(Uuid);

impl HostInstanceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for HostInstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for HostInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for HostInstanceId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CommandId(Uuid);

impl CommandId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for CommandId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for CommandId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct OutputSequence(u64);

impl OutputSequence {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    pub fn checked_add(self, value: u64) -> Option<Self> {
        self.0.checked_add(value).map(Self)
    }
}

impl fmt::Display for OutputSequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PresetId(Uuid);

impl PresetId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for PresetId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PresetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PresetId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct HostedSessionId(Uuid);

impl HostedSessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for HostedSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for HostedSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for HostedSessionId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PositionKey(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionError {
    NoSpace,
    Overflow,
}

impl PositionKey {
    pub const STRIDE: u64 = 1_024;
    pub const FIRST: Self = Self(Self::STRIDE);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn after(self) -> Result<Self, PositionError> {
        self.0
            .checked_add(Self::STRIDE)
            .map(Self)
            .ok_or(PositionError::Overflow)
    }

    pub fn between(left: Self, right: Self) -> Result<Self, PositionError> {
        if left >= right || right.0 - left.0 <= 1 {
            return Err(PositionError::NoSpace);
        }
        Ok(Self(left.0 + (right.0 - left.0) / 2))
    }

    pub fn rebalanced(index: usize) -> Result<Self, PositionError> {
        let ordinal = u64::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or(PositionError::Overflow)?;
        ordinal
            .checked_mul(Self::STRIDE)
            .map(Self)
            .ok_or(PositionError::Overflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_is_canonical_and_round_trips() {
        let id = ProjectId::from_uuid(Uuid::from_u128(1));
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000001");
        assert_eq!(id.to_string().parse(), Ok(id));
    }

    #[test]
    fn preset_id_is_canonical_and_round_trips() {
        let id = PresetId::from_uuid(Uuid::from_u128(2));
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000002");
        assert_eq!(id.to_string().parse(), Ok(id));
    }

    #[test]
    fn group_id_is_canonical_and_round_trips() {
        let id = GroupId::from_uuid(Uuid::from_u128(3));
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000003");
        assert_eq!(id.to_string().parse(), Ok(id));
    }

    #[test]
    fn host_and_command_ids_are_canonical_and_round_trip() {
        let host = HostInstanceId::from_uuid(Uuid::from_u128(4));
        let command = CommandId::from_uuid(Uuid::from_u128(5));
        assert_eq!(host.to_string().parse(), Ok(host));
        assert_eq!(command.to_string().parse(), Ok(command));
    }

    #[test]
    fn output_sequence_is_checked_and_monotonic() {
        assert_eq!(
            OutputSequence::ZERO.checked_next(),
            Some(OutputSequence::new(1))
        );
        assert_eq!(OutputSequence::new(u64::MAX).checked_next(), None);
    }

    #[test]
    fn position_midpoint_and_rebalance_are_integer_and_deterministic() {
        assert_eq!(
            PositionKey::between(PositionKey::new(1_024), PositionKey::new(2_048)),
            Ok(PositionKey::new(1_536))
        );
        assert_eq!(
            PositionKey::between(PositionKey::new(1), PositionKey::new(2)),
            Err(PositionError::NoSpace)
        );
        assert_eq!(
            PositionKey::rebalanced(999),
            Ok(PositionKey::new(1_024_000))
        );
    }

    #[test]
    fn revision_and_position_overflow_are_explicit() {
        assert_eq!(Revision::new(u64::MAX).next(), None);
        assert_eq!(
            PositionKey::new(u64::MAX).after(),
            Err(PositionError::Overflow)
        );
    }
}
