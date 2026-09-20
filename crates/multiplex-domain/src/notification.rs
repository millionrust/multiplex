use std::collections::VecDeque;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ActivityState, HostSequence, HostedSessionId, OccupantGeneration};

pub const MAX_NOTIFICATION_KEYS: usize = 256;
pub const MAX_NOTIFICATION_RECORDS: usize = 256;
pub const NOTIFICATION_KEY_TTL_MILLIS: u64 = 24 * 60 * 60 * 1_000;
pub const NOTIFICATION_RATE_WINDOW_MILLIS: u64 = 60 * 60 * 1_000;
pub const NOTIFICATION_COALESCE_WINDOW_MILLIS: u64 = 30 * 1_000;
pub const DEFAULT_OS_NOTIFICATIONS_PER_HOUR: usize = 20;
pub const ABSOLUTE_OS_NOTIFICATIONS_PER_HOUR: usize = 60;
pub const COALESCE_AFTER_EVENTS: usize = 5;
pub const MAX_NOTIFICATION_TITLE_SCALARS: usize = 160;
pub const GENERIC_NOTIFICATION_TITLE: &str = "A session";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationMode {
    Off,
    #[default]
    InApp,
    Os,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    #[default]
    Unknown,
    Granted,
    Denied,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationActivity {
    NeedsInput,
    Done,
    Failed,
}

impl NotificationActivity {
    pub const fn label(self) -> &'static str {
        match self {
            Self::NeedsInput => "Needs input",
            Self::Done => "Done",
            Self::Failed => "Failed",
        }
    }
}

impl TryFrom<ActivityState> for NotificationActivity {
    type Error = NotificationError;

    fn try_from(value: ActivityState) -> Result<Self, Self::Error> {
        match value {
            ActivityState::NeedsInput => Ok(Self::NeedsInput),
            ActivityState::Done => Ok(Self::Done),
            ActivityState::Failed => Ok(Self::Failed),
            _ => Err(NotificationError::IneligibleActivity),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationKey {
    pub session_id: HostedSessionId,
    pub generation: OccupantGeneration,
    pub activity: NotificationActivity,
    pub activity_sequence: HostSequence,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationEvent {
    pub session_id: HostedSessionId,
    pub generation: OccupantGeneration,
    pub activity: ActivityState,
    pub activity_sequence: HostSequence,
}

impl NotificationEvent {
    pub fn key(self) -> Result<NotificationKey, NotificationError> {
        if self.generation == OccupantGeneration::ZERO
            || self.activity_sequence == HostSequence::ZERO
        {
            return Err(NotificationError::InvalidEvent);
        }
        Ok(NotificationKey {
            session_id: self.session_id,
            generation: self.generation,
            activity: self.activity.try_into()?,
            activity_sequence: self.activity_sequence,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationPolicy {
    pub mode: NotificationMode,
    pub permission: PermissionState,
    pub recording_friendly: bool,
    pub os_rate_limit_per_hour: u16,
}

impl Default for NotificationPolicy {
    fn default() -> Self {
        Self {
            mode: NotificationMode::InApp,
            permission: PermissionState::Unknown,
            recording_friendly: false,
            os_rate_limit_per_hour: DEFAULT_OS_NOTIFICATIONS_PER_HOUR as u16,
        }
    }
}

impl NotificationPolicy {
    pub fn normalize(&mut self) {
        self.os_rate_limit_per_hour = self
            .os_rate_limit_per_hour
            .clamp(1, ABSOLUTE_OS_NOTIFICATIONS_PER_HOUR as u16);
    }
}

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SessionDeepLink(Uuid);

impl SessionDeepLink {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub fn platform_token(self) -> String {
        let mut token = "termirust-activity-".to_string();
        token.push_str(&self.0.to_string());
        token
    }

    pub fn from_platform_token(value: &str) -> Option<Self> {
        let value = value.strip_prefix("termirust-activity-")?;
        Uuid::parse_str(value).ok().map(Self)
    }
}

impl Default for SessionDeepLink {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SessionDeepLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionDeepLink(<opaque>)")
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationRecord {
    pub key: NotificationKey,
    pub title: String,
    pub created_at_millis: u64,
    pub deep_link: SessionDeepLink,
    #[serde(default)]
    pub dismissed: bool,
}

impl fmt::Debug for NotificationRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NotificationRecord")
            .field("key", &self.key)
            .field("title", &"<redacted>")
            .field("created_at_millis", &self.created_at_millis)
            .field("deep_link", &self.deep_link)
            .field("dismissed", &self.dismissed)
            .finish()
    }
}

impl NotificationRecord {
    pub fn display_title(&self, recording_friendly: bool) -> &str {
        if recording_friendly {
            GENERIC_NOTIFICATION_TITLE
        } else {
            &self.title
        }
    }

    pub fn validate(&self) -> Result<(), NotificationError> {
        normalize_title(&self.title)?;
        if self.key.generation == OccupantGeneration::ZERO
            || self.key.activity_sequence == HostSequence::ZERO
        {
            return Err(NotificationError::InvalidEvent);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationClock {
    pub wall_millis: u64,
    pub monotonic_millis: u64,
    pub runtime_epoch_wall_millis: u64,
}

impl NotificationClock {
    fn conservative_now(self, high_water: u64) -> u64 {
        self.wall_millis
            .max(
                self.runtime_epoch_wall_millis
                    .saturating_add(self.monotonic_millis),
            )
            .max(high_water)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotificationContext {
    pub current_generation: OccupantGeneration,
    pub unread: bool,
    pub visibly_focused: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct TimedKey {
    key: NotificationKey,
    observed_at_millis: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct OsDeliveryEvidence {
    activity: NotificationActivity,
    delivered_at_millis: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationLedger {
    #[serde(default)]
    pub records: VecDeque<NotificationRecord>,
    #[serde(default)]
    keys: VecDeque<TimedKey>,
    #[serde(default)]
    os_deliveries: VecDeque<OsDeliveryEvidence>,
    #[serde(default)]
    wall_high_water_millis: u64,
}

impl NotificationLedger {
    pub fn validate(&self) -> Result<(), NotificationError> {
        if self.records.len() > MAX_NOTIFICATION_RECORDS
            || self.keys.len() > MAX_NOTIFICATION_KEYS
            || self.os_deliveries.len() > ABSOLUTE_OS_NOTIFICATIONS_PER_HOUR
        {
            return Err(NotificationError::ResourceLimit);
        }
        self.records
            .iter()
            .try_for_each(NotificationRecord::validate)
    }

    pub fn sanitize(&mut self) {
        while self.records.len() > MAX_NOTIFICATION_RECORDS {
            self.records.pop_back();
        }
        while self.keys.len() > MAX_NOTIFICATION_KEYS {
            self.keys.pop_front();
        }
        while self.os_deliveries.len() > ABSOLUTE_OS_NOTIFICATIONS_PER_HOUR {
            self.os_deliveries.pop_front();
        }
    }

    pub fn dismiss(&mut self, link: SessionDeepLink) -> bool {
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.deep_link == link)
        else {
            return false;
        };
        record.dismissed = true;
        true
    }

    pub fn record_for_link(&self, link: SessionDeepLink) -> Option<&NotificationRecord> {
        self.records.iter().find(|record| record.deep_link == link)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationSuppression {
    Disabled,
    NotUnread,
    Focused,
    StaleGeneration,
    Duplicate,
    Permission,
    RateLimited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformDelivery {
    Individual,
    CoalescedSummary { event_count: usize },
    ReplaceSummary { event_count: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotificationDecision {
    pub recorded: bool,
    pub platform: Option<PlatformDelivery>,
    pub suppression: Option<NotificationSuppression>,
}

impl NotificationDecision {
    const fn suppressed(reason: NotificationSuppression) -> Self {
        Self {
            recorded: false,
            platform: None,
            suppression: Some(reason),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeepLinkFailure {
    Unknown,
    Unauthorized,
    Removed,
    Archived,
    StaleGeneration,
    StateChanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeepLinkSessionState {
    pub authorized: bool,
    pub exists: bool,
    pub archived: bool,
    pub generation: OccupantGeneration,
    pub activity: ActivityState,
    pub activity_sequence: HostSequence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationError {
    IneligibleActivity,
    InvalidEvent,
    InvalidTitle,
    ResourceLimit,
}

pub fn reduce_notification(
    ledger: &mut NotificationLedger,
    policy: NotificationPolicy,
    event: NotificationEvent,
    title: &str,
    deep_link: SessionDeepLink,
    context: NotificationContext,
    clock: NotificationClock,
) -> Result<NotificationDecision, NotificationError> {
    let key = event.key()?;
    if policy.mode == NotificationMode::Off {
        return Ok(NotificationDecision::suppressed(
            NotificationSuppression::Disabled,
        ));
    }
    if !context.unread {
        return Ok(NotificationDecision::suppressed(
            NotificationSuppression::NotUnread,
        ));
    }
    if context.visibly_focused {
        return Ok(NotificationDecision::suppressed(
            NotificationSuppression::Focused,
        ));
    }
    if event.generation != context.current_generation {
        return Ok(NotificationDecision::suppressed(
            NotificationSuppression::StaleGeneration,
        ));
    }
    let title = normalize_title(title)?;
    let now = clock.conservative_now(ledger.wall_high_water_millis);
    ledger.wall_high_water_millis = now;
    prune_ledger(ledger, now);
    if ledger.keys.iter().any(|entry| entry.key == key) {
        return Ok(NotificationDecision::suppressed(
            NotificationSuppression::Duplicate,
        ));
    }

    ledger.keys.push_back(TimedKey {
        key,
        observed_at_millis: now,
    });
    while ledger.keys.len() > MAX_NOTIFICATION_KEYS {
        ledger.keys.pop_front();
    }
    ledger.records.push_front(NotificationRecord {
        key,
        title,
        created_at_millis: now,
        deep_link,
        dismissed: false,
    });
    while ledger.records.len() > MAX_NOTIFICATION_RECORDS {
        ledger.records.pop_back();
    }

    if policy.mode != NotificationMode::Os || policy.permission != PermissionState::Granted {
        return Ok(NotificationDecision {
            recorded: true,
            platform: None,
            suppression: Some(NotificationSuppression::Permission),
        });
    }

    let hourly_count = ledger.os_deliveries.len();
    if hourly_count >= ABSOLUTE_OS_NOTIFICATIONS_PER_HOUR {
        return Ok(NotificationDecision {
            recorded: true,
            platform: None,
            suppression: Some(NotificationSuppression::RateLimited),
        });
    }
    let configured_limit = usize::from(
        policy
            .os_rate_limit_per_hour
            .clamp(1, ABSOLUTE_OS_NOTIFICATIONS_PER_HOUR as u16),
    );
    if hourly_count >= configured_limit && key.activity != NotificationActivity::NeedsInput {
        return Ok(NotificationDecision {
            recorded: true,
            platform: None,
            suppression: Some(NotificationSuppression::RateLimited),
        });
    }

    let recent_count = ledger
        .os_deliveries
        .iter()
        .filter(|entry| {
            now.saturating_sub(entry.delivered_at_millis) <= NOTIFICATION_COALESCE_WINDOW_MILLIS
        })
        .count()
        + 1;
    let platform = if hourly_count >= configured_limit {
        PlatformDelivery::ReplaceSummary {
            event_count: recent_count,
        }
    } else if recent_count > COALESCE_AFTER_EVENTS {
        PlatformDelivery::CoalescedSummary {
            event_count: recent_count,
        }
    } else {
        PlatformDelivery::Individual
    };
    ledger.os_deliveries.push_back(OsDeliveryEvidence {
        activity: key.activity,
        delivered_at_millis: now,
    });
    Ok(NotificationDecision {
        recorded: true,
        platform: Some(platform),
        suppression: None,
    })
}

pub fn resolve_session_deep_link(
    ledger: &NotificationLedger,
    link: SessionDeepLink,
    current: DeepLinkSessionState,
) -> Result<HostedSessionId, DeepLinkFailure> {
    let record = ledger
        .record_for_link(link)
        .ok_or(DeepLinkFailure::Unknown)?;
    if !current.authorized {
        return Err(DeepLinkFailure::Unauthorized);
    }
    if !current.exists {
        return Err(DeepLinkFailure::Removed);
    }
    if current.archived {
        return Err(DeepLinkFailure::Archived);
    }
    if current.generation != record.key.generation {
        return Err(DeepLinkFailure::StaleGeneration);
    }
    if NotificationActivity::try_from(current.activity).ok() != Some(record.key.activity)
        || current.activity_sequence != record.key.activity_sequence
    {
        return Err(DeepLinkFailure::StateChanged);
    }
    Ok(record.key.session_id)
}

fn normalize_title(value: &str) -> Result<String, NotificationError> {
    let value = value.trim();
    let count = value.chars().count();
    if count == 0 || count > MAX_NOTIFICATION_TITLE_SCALARS || value.chars().any(char::is_control) {
        return Err(NotificationError::InvalidTitle);
    }
    Ok(value.to_string())
}

fn prune_ledger(ledger: &mut NotificationLedger, now: u64) {
    while ledger.keys.front().is_some_and(|entry| {
        now.saturating_sub(entry.observed_at_millis) > NOTIFICATION_KEY_TTL_MILLIS
    }) {
        ledger.keys.pop_front();
    }
    while ledger.os_deliveries.front().is_some_and(|entry| {
        now.saturating_sub(entry.delivered_at_millis) >= NOTIFICATION_RATE_WINDOW_MILLIS
    }) {
        ledger.os_deliveries.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct NotificationBoundaryFixture {
        coalesce_sequences: Vec<u64>,
        configured_rate_sequences: Vec<u64>,
        absolute_rate_sequences: Vec<u64>,
        ledger_sequences: Vec<u64>,
    }

    fn boundary_fixture() -> NotificationBoundaryFixture {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/notifications/boundary_streams.json"
        ))
        .unwrap()
    }

    fn clock(millis: u64) -> NotificationClock {
        NotificationClock {
            wall_millis: millis,
            monotonic_millis: millis,
            runtime_epoch_wall_millis: 0,
        }
    }

    fn event(sequence: u64, activity: ActivityState) -> NotificationEvent {
        NotificationEvent {
            session_id: HostedSessionId::from_uuid(Uuid::from_u128(1)),
            generation: OccupantGeneration::new(2),
            activity,
            activity_sequence: HostSequence::new(sequence),
        }
    }

    fn context() -> NotificationContext {
        NotificationContext {
            current_generation: OccupantGeneration::new(2),
            unread: true,
            visibly_focused: false,
        }
    }

    fn os_policy(limit: u16) -> NotificationPolicy {
        NotificationPolicy {
            mode: NotificationMode::Os,
            permission: PermissionState::Granted,
            recording_friendly: false,
            os_rate_limit_per_hour: limit,
        }
    }

    fn observe(
        ledger: &mut NotificationLedger,
        sequence: u64,
        activity: ActivityState,
        millis: u64,
        policy: NotificationPolicy,
    ) -> NotificationDecision {
        reduce_notification(
            ledger,
            policy,
            event(sequence, activity),
            "Build session",
            SessionDeepLink::from_uuid(Uuid::from_u128(sequence as u128)),
            context(),
            clock(millis),
        )
        .unwrap()
    }

    #[test]
    fn notification_eligibility_requires_unread_unfocused_current_generation() {
        let mut ledger = NotificationLedger::default();
        for (context, expected) in [
            (
                NotificationContext {
                    unread: false,
                    ..context()
                },
                NotificationSuppression::NotUnread,
            ),
            (
                NotificationContext {
                    visibly_focused: true,
                    ..context()
                },
                NotificationSuppression::Focused,
            ),
            (
                NotificationContext {
                    current_generation: OccupantGeneration::new(3),
                    ..context()
                },
                NotificationSuppression::StaleGeneration,
            ),
        ] {
            let decision = reduce_notification(
                &mut ledger,
                NotificationPolicy::default(),
                event(1, ActivityState::NeedsInput),
                "A session",
                SessionDeepLink::default(),
                context,
                clock(1),
            )
            .unwrap();
            assert_eq!(decision.suppression, Some(expected));
        }
        assert!(ledger.records.is_empty());
    }

    #[test]
    fn notification_deduplicates_exact_generation_kind_and_sequence() {
        let mut ledger = NotificationLedger::default();
        assert!(
            observe(
                &mut ledger,
                1,
                ActivityState::Done,
                1,
                NotificationPolicy::default()
            )
            .recorded
        );
        let duplicate = observe(
            &mut ledger,
            1,
            ActivityState::Done,
            2,
            NotificationPolicy::default(),
        );
        assert_eq!(
            duplicate.suppression,
            Some(NotificationSuppression::Duplicate)
        );
        assert_eq!(ledger.records.len(), 1);
        assert!(
            observe(
                &mut ledger,
                1,
                ActivityState::Failed,
                3,
                NotificationPolicy::default()
            )
            .recorded
        );
    }

    #[test]
    fn notification_coalesces_at_exact_five_six_boundary() {
        let mut ledger = NotificationLedger::default();
        let fixture = boundary_fixture();
        for sequence in fixture.coalesce_sequences.iter().copied().take(5) {
            assert_eq!(
                observe(
                    &mut ledger,
                    sequence,
                    ActivityState::Done,
                    sequence,
                    os_policy(20)
                )
                .platform,
                Some(PlatformDelivery::Individual)
            );
        }
        let sixth = fixture.coalesce_sequences[5];
        assert_eq!(
            observe(
                &mut ledger,
                sixth,
                ActivityState::Done,
                sixth,
                os_policy(20)
            )
            .platform,
            Some(PlatformDelivery::CoalescedSummary { event_count: 6 })
        );
    }

    #[test]
    fn notification_rates_at_twenty_twenty_one_and_sixty_sixty_one() {
        let mut ledger = NotificationLedger::default();
        let fixture = boundary_fixture();
        for sequence in fixture.configured_rate_sequences.iter().copied().take(20) {
            assert!(
                observe(
                    &mut ledger,
                    sequence,
                    ActivityState::Done,
                    sequence,
                    os_policy(20)
                )
                .platform
                .is_some()
            );
        }
        assert_eq!(
            observe(
                &mut ledger,
                fixture.configured_rate_sequences[20],
                ActivityState::Done,
                fixture.configured_rate_sequences[20],
                os_policy(20),
            )
            .suppression,
            Some(NotificationSuppression::RateLimited)
        );
        for sequence in fixture.absolute_rate_sequences.iter().copied().skip(21) {
            let decision = observe(
                &mut ledger,
                sequence,
                ActivityState::NeedsInput,
                sequence,
                os_policy(20),
            );
            assert_eq!(
                decision.platform,
                Some(PlatformDelivery::ReplaceSummary {
                    event_count: sequence as usize - 1,
                })
            );
        }
        assert_eq!(
            observe(
                &mut ledger,
                62,
                ActivityState::NeedsInput,
                62,
                os_policy(20)
            )
            .suppression,
            Some(NotificationSuppression::RateLimited)
        );
    }

    #[test]
    fn notification_key_ledger_is_bounded_at_256_257() {
        let mut ledger = NotificationLedger::default();
        for sequence in boundary_fixture().ledger_sequences {
            observe(
                &mut ledger,
                sequence,
                ActivityState::Done,
                sequence,
                NotificationPolicy::default(),
            );
        }
        assert_eq!(ledger.keys.len(), 256);
        assert_eq!(ledger.records.len(), 256);
        assert_eq!(ledger.keys.front().unwrap().key.activity_sequence.get(), 2);
    }

    #[test]
    fn wall_clock_rollback_does_not_expire_or_reset_evidence() {
        let mut ledger = NotificationLedger::default();
        observe(
            &mut ledger,
            1,
            ActivityState::Done,
            100_000,
            NotificationPolicy::default(),
        );
        let rollback_clock = NotificationClock {
            wall_millis: 1,
            monotonic_millis: 2,
            runtime_epoch_wall_millis: 1,
        };
        let duplicate = reduce_notification(
            &mut ledger,
            NotificationPolicy::default(),
            event(1, ActivityState::Done),
            "Build session",
            SessionDeepLink::default(),
            context(),
            rollback_clock,
        )
        .unwrap();
        assert_eq!(
            duplicate.suppression,
            Some(NotificationSuppression::Duplicate)
        );
        assert_eq!(ledger.wall_high_water_millis, 100_000);
    }

    #[test]
    fn persisted_clock_evidence_survives_restart_and_ttl_boundaries_are_exact() {
        let mut ledger = NotificationLedger::default();
        observe(
            &mut ledger,
            1,
            ActivityState::Done,
            100,
            NotificationPolicy::default(),
        );
        let encoded = serde_json::to_vec(&ledger).unwrap();
        let mut restarted: NotificationLedger = serde_json::from_slice(&encoded).unwrap();

        let duplicate = observe(
            &mut restarted,
            1,
            ActivityState::Done,
            1,
            NotificationPolicy::default(),
        );
        assert_eq!(
            duplicate.suppression,
            Some(NotificationSuppression::Duplicate)
        );
        observe(
            &mut restarted,
            2,
            ActivityState::Done,
            100 + NOTIFICATION_KEY_TTL_MILLIS,
            NotificationPolicy::default(),
        );
        assert_eq!(
            observe(
                &mut restarted,
                1,
                ActivityState::Done,
                100 + NOTIFICATION_KEY_TTL_MILLIS,
                NotificationPolicy::default(),
            )
            .suppression,
            Some(NotificationSuppression::Duplicate)
        );
        observe(
            &mut restarted,
            3,
            ActivityState::Done,
            101 + NOTIFICATION_KEY_TTL_MILLIS,
            NotificationPolicy::default(),
        );
        assert!(
            observe(
                &mut restarted,
                1,
                ActivityState::Done,
                101 + NOTIFICATION_KEY_TTL_MILLIS,
                NotificationPolicy::default(),
            )
            .recorded
        );
    }

    #[test]
    fn persisted_rate_window_resists_rollback_and_reopens_at_exact_hour() {
        let mut ledger = NotificationLedger::default();
        for sequence in 1..=20 {
            assert!(
                observe(
                    &mut ledger,
                    sequence,
                    ActivityState::Done,
                    99 + sequence,
                    os_policy(20),
                )
                .platform
                .is_some()
            );
        }
        let encoded = serde_json::to_vec(&ledger).unwrap();
        let mut restarted: NotificationLedger = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            observe(&mut restarted, 21, ActivityState::Done, 1, os_policy(20)).suppression,
            Some(NotificationSuppression::RateLimited)
        );
        assert!(
            observe(
                &mut restarted,
                22,
                ActivityState::Done,
                100 + NOTIFICATION_RATE_WINDOW_MILLIS,
                os_policy(20),
            )
            .platform
            .is_some()
        );
    }

    #[test]
    fn deep_links_revalidate_access_generation_and_current_state_without_mutation() {
        let mut ledger = NotificationLedger::default();
        observe(
            &mut ledger,
            7,
            ActivityState::NeedsInput,
            1,
            NotificationPolicy::default(),
        );
        let link = SessionDeepLink::from_uuid(Uuid::from_u128(7));
        assert_eq!(
            SessionDeepLink::from_platform_token(&link.platform_token()),
            Some(link)
        );
        assert_eq!(
            SessionDeepLink::from_platform_token("https://example.test"),
            None
        );
        let current = DeepLinkSessionState {
            authorized: true,
            exists: true,
            archived: false,
            generation: OccupantGeneration::new(2),
            activity: ActivityState::NeedsInput,
            activity_sequence: HostSequence::new(7),
        };
        assert_eq!(
            resolve_session_deep_link(&ledger, link, current),
            Ok(event(7, ActivityState::NeedsInput).session_id)
        );
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(
            resolve_session_deep_link(
                &ledger,
                link,
                DeepLinkSessionState {
                    generation: OccupantGeneration::new(3),
                    ..current
                }
            ),
            Err(DeepLinkFailure::StaleGeneration)
        );
        assert_eq!(
            resolve_session_deep_link(
                &ledger,
                link,
                DeepLinkSessionState {
                    activity: ActivityState::Done,
                    ..current
                }
            ),
            Err(DeepLinkFailure::StateChanged)
        );
        assert!(!ledger.records[0].dismissed);
    }

    #[test]
    fn recording_friendly_payload_hides_title_and_debug_is_redacted() {
        let mut ledger = NotificationLedger::default();
        observe(
            &mut ledger,
            1,
            ActivityState::Failed,
            1,
            NotificationPolicy::default(),
        );
        let record = &ledger.records[0];
        assert_eq!(record.display_title(true), "A session");
        assert!(!format!("{record:?}").contains("Build session"));
        assert!(!format!("{:?}", record.deep_link).contains('-'));
    }
}
