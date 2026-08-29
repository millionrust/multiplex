use std::time::Instant;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, Hsla, InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _,
    Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};

use termirust_domain::{
    ActivityState, DeepLinkSessionState, HostedSession, HostedSessionId, NotificationActivity,
    NotificationClock, NotificationContext, NotificationDecision, NotificationEvent,
    NotificationMode, NotificationPolicy, NotificationRecord, PermissionState, PlatformDelivery,
    SessionDeepLink, reduce_notification, resolve_session_deep_link,
};
use termirust_store::{NotificationRepository, NotificationSnapshot, NotificationStoreError};

use crate::platform_notifications::{
    PlatformNotificationRequest, PlatformNotifications, system_platform_notifications,
};
use crate::storage::app_dir;
use crate::ui::util::current_unix_millis;
use crate::ui::{localization, theme};

use super::session_coordinator::SessionActivityObserver;
use super::{NavSection, TermiRustApp};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActivityCenterFailure {
    Corrupt,
    Newer,
    PermissionDenied,
    Unavailable,
}

pub(super) struct ActivityCenterState {
    repository: Option<NotificationRepository>,
    snapshot: Option<NotificationSnapshot>,
    failure: Option<ActivityCenterFailure>,
    platform: Box<dyn PlatformNotifications>,
    runtime_epoch_wall_millis: u64,
    started: Instant,
    last_permission_poll: Instant,
}

pub(super) enum ActivityActivation {
    Center,
    Session(SessionDeepLink),
}

impl ActivityCenterState {
    pub fn open_default() -> Self {
        let platform = system_platform_notifications();
        let result = app_dir()
            .map(|root| root.join("notifications"))
            .map_err(|_| ActivityCenterFailure::Unavailable)
            .and_then(|root| {
                NotificationRepository::open(root).map_err(|error| classify_failure(&error))
            });
        match result {
            Ok(repository) => Self::open_with_repository(repository, platform),
            Err(failure) => Self::failed(failure, platform),
        }
    }

    fn open_with_repository(
        repository: NotificationRepository,
        platform: Box<dyn PlatformNotifications>,
    ) -> Self {
        match repository.load() {
            Ok(snapshot) => Self {
                repository: Some(repository),
                snapshot: Some(snapshot),
                failure: None,
                platform,
                runtime_epoch_wall_millis: current_unix_millis(),
                started: Instant::now(),
                last_permission_poll: Instant::now(),
            },
            Err(error) => Self {
                repository: Some(repository),
                snapshot: None,
                failure: Some(classify_failure(&error)),
                platform,
                runtime_epoch_wall_millis: current_unix_millis(),
                started: Instant::now(),
                last_permission_poll: Instant::now(),
            },
        }
    }

    fn failed(failure: ActivityCenterFailure, platform: Box<dyn PlatformNotifications>) -> Self {
        Self {
            repository: None,
            snapshot: None,
            failure: Some(failure),
            platform,
            runtime_epoch_wall_millis: current_unix_millis(),
            started: Instant::now(),
            last_permission_poll: Instant::now(),
        }
    }

    pub fn failure(&self) -> Option<ActivityCenterFailure> {
        self.failure
    }

    pub fn policy(&self) -> NotificationPolicy {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.policy)
            .unwrap_or_default()
    }

    pub fn records(&self) -> impl Iterator<Item = &NotificationRecord> {
        self.snapshot
            .iter()
            .flat_map(|snapshot| snapshot.ledger.records.iter())
            .filter(|record| !record.dismissed)
    }

    pub fn visible_count(&self) -> usize {
        self.records().count()
    }

    pub fn observe_transition(
        &mut self,
        previous: Option<&HostedSession>,
        current: &HostedSession,
        visibly_focused: bool,
    ) -> Result<NotificationDecision, NotificationStoreError> {
        self.observe_transition_once(previous, current, visibly_focused, true)
    }

    fn observe_transition_once(
        &mut self,
        previous: Option<&HostedSession>,
        current: &HostedSession,
        visibly_focused: bool,
        retry_stale: bool,
    ) -> Result<NotificationDecision, NotificationStoreError> {
        if previous.is_some_and(|previous| {
            previous.activity.state == current.activity.state
                && previous.activity.generation == current.activity.generation
                && previous.activity.effective_sequence == current.activity.effective_sequence
        }) {
            return Ok(NotificationDecision {
                recorded: false,
                platform: None,
                suppression: None,
            });
        }
        let Some(repository) = self.repository.as_ref() else {
            return Err(NotificationStoreError::Corrupt);
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Err(NotificationStoreError::Corrupt);
        };
        let event = NotificationEvent {
            session_id: current.id,
            generation: current.activity.generation,
            activity: current.activity.state,
            activity_sequence: current.activity.effective_sequence,
        };
        let mut ledger = snapshot.ledger.clone();
        let deep_link = SessionDeepLink::new();
        let decision = match reduce_notification(
            &mut ledger,
            snapshot.policy,
            event,
            if matches!(current.title_source, termirust_domain::TitleSource::Manual) {
                current.title.as_str()
            } else {
                termirust_domain::GENERIC_NOTIFICATION_TITLE
            },
            deep_link,
            NotificationContext {
                current_generation: current.activity.generation,
                unread: current.unread(),
                visibly_focused,
            },
            self.clock(),
        ) {
            Ok(decision) => decision,
            Err(_) => {
                return Ok(NotificationDecision {
                    recorded: false,
                    platform: None,
                    suppression: None,
                });
            }
        };
        if !decision.recorded {
            return Ok(decision);
        }
        let next = match repository.save(snapshot.revision, snapshot.policy, ledger) {
            Ok(next) => next,
            Err(NotificationStoreError::StaleRevision { .. }) if retry_stale => {
                self.snapshot = Some(repository.load()?);
                return self.observe_transition_once(previous, current, visibly_focused, false);
            }
            Err(error) => return Err(error),
        };
        let record = next.ledger.record_for_link(deep_link).cloned();
        self.snapshot = Some(next);

        if let (Some(delivery), Some(record)) = (decision.platform, record) {
            self.deliver_platform(delivery, &record);
        }
        Ok(decision)
    }

    pub fn set_mode(&mut self, mode: NotificationMode) -> Result<(), NotificationStoreError> {
        let mut policy = self.policy();
        policy.mode = mode;
        if mode == NotificationMode::Os {
            let queried = self.platform.query_permission();
            policy.permission = if queried == PermissionState::Unknown {
                self.platform.request_permission()
            } else {
                queried
            };
        }
        self.save_policy(policy)
    }

    pub fn set_recording_friendly(&mut self, enabled: bool) -> Result<(), NotificationStoreError> {
        let mut policy = self.policy();
        policy.recording_friendly = enabled;
        self.save_policy(policy)
    }

    pub fn refresh_permission(&mut self) -> Result<PermissionState, NotificationStoreError> {
        let permission = self.platform.query_permission();
        let mut policy = self.policy();
        policy.permission = permission;
        self.save_policy(policy)?;
        Ok(permission)
    }

    pub fn poll_permission_if_due(&mut self) -> Result<(), NotificationStoreError> {
        if self.policy().mode != NotificationMode::Os
            || self.last_permission_poll.elapsed().as_secs() < 2
        {
            return Ok(());
        }
        self.last_permission_poll = Instant::now();
        let permission = self.platform.query_permission();
        if permission == PermissionState::Unknown || permission == self.policy().permission {
            return Ok(());
        }
        let mut policy = self.policy();
        policy.permission = permission;
        self.save_policy(policy)
    }

    pub fn reset_after_corruption(&mut self) -> Result<(), NotificationStoreError> {
        let repository = self
            .repository
            .as_ref()
            .ok_or(NotificationStoreError::Corrupt)?;
        self.snapshot = Some(repository.reset_after_corruption()?);
        self.failure = None;
        Ok(())
    }

    pub fn dismiss(&mut self, link: SessionDeepLink) -> Result<bool, NotificationStoreError> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Err(NotificationStoreError::Corrupt);
        };
        let mut ledger = snapshot.ledger.clone();
        if !ledger.dismiss(link) {
            return Ok(false);
        }
        let repository = self
            .repository
            .as_ref()
            .ok_or(NotificationStoreError::Corrupt)?;
        match repository.save(snapshot.revision, snapshot.policy, ledger) {
            Ok(snapshot) => self.snapshot = Some(snapshot),
            Err(error @ NotificationStoreError::StaleRevision { .. }) => {
                self.snapshot = Some(repository.load()?);
                return Err(error);
            }
            Err(error) => return Err(error),
        }
        let _ = self.platform.remove(&link.platform_token());
        Ok(true)
    }

    pub fn take_platform_activation(&mut self) -> Option<ActivityActivation> {
        let identifier = self.platform.take_activation()?;
        if identifier == "termirust-activity-summary" {
            Some(ActivityActivation::Center)
        } else {
            SessionDeepLink::from_platform_token(&identifier).map(ActivityActivation::Session)
        }
    }

    pub fn resolve(
        &self,
        link: SessionDeepLink,
        current: DeepLinkSessionState,
    ) -> Result<HostedSessionId, termirust_domain::DeepLinkFailure> {
        let ledger = &self
            .snapshot
            .as_ref()
            .ok_or(termirust_domain::DeepLinkFailure::Unknown)?
            .ledger;
        resolve_session_deep_link(ledger, link, current)
    }

    fn save_policy(&mut self, policy: NotificationPolicy) -> Result<(), NotificationStoreError> {
        let repository = self
            .repository
            .as_ref()
            .ok_or(NotificationStoreError::Corrupt)?;
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or(NotificationStoreError::Corrupt)?;
        match repository.save(snapshot.revision, policy, snapshot.ledger.clone()) {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                Ok(())
            }
            Err(error @ NotificationStoreError::StaleRevision { .. }) => {
                self.snapshot = Some(repository.load()?);
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn deliver_platform(&mut self, delivery: PlatformDelivery, record: &NotificationRecord) {
        let policy = self.policy();
        let title = record.display_title(policy.recording_friendly);
        let body = match delivery {
            PlatformDelivery::Individual => localization::notification_individual_payload(
                title,
                notification_activity_label(record.key.activity),
            ),
            PlatformDelivery::CoalescedSummary { event_count }
            | PlatformDelivery::ReplaceSummary { event_count } => {
                localization::notification_summary_payload(event_count)
            }
        };
        let identifier = platform_identifier(record, delivery);
        let Ok(request) = PlatformNotificationRequest::new(&identifier, "TermiRust", &body) else {
            return;
        };
        let _ = self.platform.deliver(&request);
    }

    fn clock(&self) -> NotificationClock {
        NotificationClock {
            wall_millis: current_unix_millis(),
            monotonic_millis: self.started.elapsed().as_millis() as u64,
            runtime_epoch_wall_millis: self.runtime_epoch_wall_millis,
        }
    }
}

impl SessionActivityObserver for ActivityCenterState {
    fn observe_session_transition(
        &mut self,
        previous: Option<&HostedSession>,
        current: &HostedSession,
        visibly_focused: bool,
    ) -> Result<(), String> {
        ActivityCenterState::observe_transition(self, previous, current, visibly_focused)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn platform_identifier(record: &NotificationRecord, delivery: PlatformDelivery) -> String {
    if !matches!(delivery, PlatformDelivery::Individual) {
        return "termirust-activity-summary".to_string();
    }
    record.deep_link.platform_token()
}

fn classify_failure(error: &NotificationStoreError) -> ActivityCenterFailure {
    match error {
        NotificationStoreError::Corrupt | NotificationStoreError::TooLarge => {
            ActivityCenterFailure::Corrupt
        }
        NotificationStoreError::Newer { .. } => ActivityCenterFailure::Newer,
        NotificationStoreError::Io {
            kind: std::io::ErrorKind::PermissionDenied,
            ..
        } => ActivityCenterFailure::PermissionDenied,
        _ => ActivityCenterFailure::Unavailable,
    }
}

fn activity_icon(activity: NotificationActivity) -> IconName {
    match activity {
        NotificationActivity::NeedsInput => IconName::Bell,
        NotificationActivity::Done => IconName::CircleCheck,
        NotificationActivity::Failed => IconName::TriangleAlert,
    }
}

fn activity_tone(activity: NotificationActivity) -> Hsla {
    match activity {
        NotificationActivity::NeedsInput => theme::warning(),
        NotificationActivity::Done => theme::success(),
        NotificationActivity::Failed => theme::danger(),
    }
}

fn notification_activity_label(activity: NotificationActivity) -> String {
    match activity {
        NotificationActivity::NeedsInput => localization::session_library_activity_needs_input(),
        NotificationActivity::Done => localization::session_library_activity_done(),
        NotificationActivity::Failed => localization::session_library_activity_failed(),
    }
}

fn localized_activity_age(created_at_millis: u64, now_millis: u64) -> String {
    if created_at_millis == 0 || created_at_millis > now_millis {
        return localization::activity_age_just_now();
    }
    let elapsed_seconds = now_millis.saturating_sub(created_at_millis) / 1_000;
    if elapsed_seconds < 60 {
        return localization::activity_age_just_now();
    }
    let minutes = elapsed_seconds / 60;
    if minutes < 60 {
        return localization::activity_age_minutes(minutes as usize);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return localization::activity_age_hours(hours as usize);
    }
    let days = hours / 24;
    if days == 1 {
        return localization::activity_age_yesterday();
    }
    if days < 7 {
        return localization::activity_age_days(days as usize);
    }
    if days < 30 {
        return localization::activity_age_weeks((days / 7) as usize);
    }
    if days < 365 {
        return localization::activity_age_months((days / 30) as usize);
    }
    localization::activity_age_years((days / 365) as usize)
}

impl TermiRustApp {
    pub(super) fn process_activity_activation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.activity_center.poll_permission_if_due().is_err() {
            self.error_message = localization::activity_center_operation_failed();
        }
        match self.activity_center.take_platform_activation() {
            Some(ActivityActivation::Center) => {
                self.activate_library_section(NavSection::Activity, window, cx);
            }
            Some(ActivityActivation::Session(link)) => {
                self.open_activity_record(link, window, cx);
            }
            None => {}
        }
    }

    pub(super) fn render_activity_center_view(&self, cx: &Context<Self>) -> Div {
        let records = self.activity_center.records().cloned().collect::<Vec<_>>();
        let record_count = records.len();
        let policy = self.activity_center.policy();
        let failure = self.activity_center.failure();
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_5()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(localization::activity_center_title()),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme::text_muted())
                                    .child(localization::activity_center_description()),
                            ),
                    )
                    .child(
                        Button::new("activity-center-settings")
                            .small()
                            .icon(IconName::Settings)
                            .label(localization::activity_center_settings_action())
                            .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.activate_library_section(NavSection::Settings, window, cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .overflow_y_scrollbar()
                    .when_some(failure, |this, failure| {
                        let message = match failure {
                            ActivityCenterFailure::Corrupt => {
                                localization::activity_center_store_corrupt()
                            }
                            ActivityCenterFailure::Newer => {
                                localization::activity_center_store_newer()
                            }
                            ActivityCenterFailure::PermissionDenied => {
                                localization::activity_center_store_permission_denied()
                            }
                            ActivityCenterFailure::Unavailable => {
                                localization::activity_center_store_unavailable()
                            }
                        };
                        this.child(activity_notice(
                            message,
                            theme::danger(),
                            IconName::TriangleAlert,
                        ))
                    })
                    .when(
                        policy.mode == NotificationMode::Os
                            && policy.permission == PermissionState::Denied,
                        |this| {
                            this.child(activity_notice(
                                localization::notification_permission_denied_guidance(),
                                theme::warning(),
                                IconName::Info,
                            ))
                        },
                    )
                    .when(records.is_empty() && failure.is_none(), |this| {
                        this.child(
                            self.render_library_empty_state(
                                Icon::new(IconName::Inbox)
                                    .size(px(24.))
                                    .text_color(theme::accent()),
                                localization::activity_center_empty_title(),
                                localization::activity_center_empty_description(),
                            ),
                        )
                    })
                    .children(records.into_iter().enumerate().map(|(index, record)| {
                        let open_link = record.deep_link;
                        let dismiss_link = record.deep_link;
                        let tone = activity_tone(record.key.activity);
                        let title = record.display_title(policy.recording_friendly).to_string();
                        let age =
                            localized_activity_age(record.created_at_millis, current_unix_millis());
                        h_flex()
                            .id(("activity-record", index))
                            .debug_selector(move || {
                                "activity-record-".to_string() + &index.to_string()
                            })
                            .w_full()
                            .items_center()
                            .flex_wrap()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::library_card())
                            .border_1()
                            .border_color(theme::soft_border())
                            .child(
                                div()
                                    .size(px(34.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.))
                                    .bg(theme::with_alpha(tone, 0.12))
                                    .child(
                                        Icon::new(activity_icon(record.key.activity))
                                            .size(px(17.))
                                            .text_color(tone),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .flex_wrap()
                                            .child(
                                                div()
                                                    .font_medium()
                                                    .text_size(px(14.))
                                                    .text_color(theme::text_main())
                                                    .child(title),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .font_medium()
                                                    .text_color(tone)
                                                    .child(notification_activity_label(
                                                        record.key.activity,
                                                    )),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .text_size(px(12.))
                                            .text_color(theme::text_muted())
                                            .child(age)
                                            .child(localization::activity_center_position(
                                                index + 1,
                                                record_count,
                                            )),
                                    ),
                            )
                            .child(
                                Button::new(("activity-open", index))
                                    .small()
                                    .label(localization::common_open())
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_activity_record(open_link, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("activity-dismiss", index))
                                    .small()
                                    .icon(IconName::Close)
                                    .label(localization::activity_center_dismiss_action())
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dismiss_activity_record(dismiss_link, cx);
                                    })),
                            )
                            .into_any_element()
                    })),
            )
    }

    fn dismiss_activity_record(&mut self, link: SessionDeepLink, cx: &mut Context<Self>) {
        match self.activity_center.dismiss(link) {
            Ok(true) => self.status_message = localization::activity_center_dismissed(),
            Ok(false) => self.error_message = localization::activity_center_link_stale(),
            Err(_) => self.error_message = localization::activity_center_operation_failed(),
        }
        cx.notify();
    }

    fn open_activity_record(
        &mut self,
        link: SessionDeepLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let record_session_id = self
            .activity_center
            .records()
            .find(|record| record.deep_link == link)
            .map(|record| record.key.session_id);
        let current = record_session_id.and_then(|id| self.session_library.session(id).cloned());
        let state = match current.as_ref() {
            Some(session) => DeepLinkSessionState {
                authorized: true,
                exists: true,
                archived: session.archived_at.is_some(),
                generation: session.activity.generation,
                activity: session.activity.state,
                activity_sequence: session.activity.effective_sequence,
            },
            None => DeepLinkSessionState {
                authorized: true,
                exists: false,
                archived: false,
                generation: termirust_domain::OccupantGeneration::ZERO,
                activity: ActivityState::Unknown,
                activity_sequence: termirust_domain::HostSequence::ZERO,
            },
        };
        match self.activity_center.resolve(link, state) {
            Ok(session_id) => self.reattach_saved_session(session_id, window, cx),
            Err(_) => {
                self.activate_library_section(NavSection::Projects, window, cx);
                self.error_message = localization::activity_center_link_stale();
                cx.notify();
            }
        }
    }
}

fn activity_notice(message: String, color: Hsla, icon: IconName) -> Div {
    h_flex()
        .w_full()
        .gap_3()
        .px_4()
        .py_3()
        .rounded(px(theme::CARD_RADIUS))
        .bg(theme::with_alpha(color, 0.1))
        .border_1()
        .border_color(theme::with_alpha(color, 0.28))
        .text_size(px(13.))
        .text_color(color)
        .child(Icon::new(icon).size(px(17.)))
        .child(message)
}

#[cfg(test)]
pub(super) mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use termirust_domain::{
        ActivityAggregate, ActivityConfidence, ActivitySourceKind, HostSequence,
        HostedSessionState, OccupantGeneration, OutputSequence, PositionKey, ProjectId, Revision,
        SessionTitle, TitleSource,
    };

    use crate::platform_notifications::PlatformNotificationError;

    use super::*;

    #[derive(Default)]
    struct FakeState {
        delivered: usize,
        fail: bool,
        permission: PermissionState,
        request_result: PermissionState,
        requests: usize,
    }

    struct FakePlatform {
        state: Arc<Mutex<FakeState>>,
    }

    impl PlatformNotifications for FakePlatform {
        fn query_permission(&self) -> PermissionState {
            self.state.lock().unwrap().permission
        }

        fn request_permission(&mut self) -> PermissionState {
            let mut state = self.state.lock().unwrap();
            state.requests += 1;
            state.permission = state.request_result;
            state.permission
        }

        fn deliver(
            &mut self,
            _request: &PlatformNotificationRequest,
        ) -> Result<(), PlatformNotificationError> {
            let mut state = self.state.lock().unwrap();
            if state.fail {
                return Err(PlatformNotificationError::DeliveryFailed);
            }
            state.delivered += 1;
            Ok(())
        }

        fn remove(&mut self, _identifier: &str) -> Result<(), PlatformNotificationError> {
            Ok(())
        }

        fn take_activation(&mut self) -> Option<String> {
            None
        }
    }

    fn session(sequence: u64) -> HostedSession {
        const TEST_TITLE: &str = "Private title";
        HostedSession {
            id: HostedSessionId::new(),
            project_id: ProjectId::new(),
            group_id: None,
            preset_id: None,
            title: SessionTitle::new(TEST_TITLE).unwrap(),
            title_source: TitleSource::Manual,
            lifecycle: HostedSessionState::Live,
            activity: ActivityAggregate {
                state: ActivityState::Done,
                confidence: ActivityConfidence::Verified,
                effective_sequence: HostSequence::new(sequence),
                generation: OccupantGeneration::new(1),
                source_kind: ActivitySourceKind::StructuredAdapter,
                source_id: "test".to_string(),
                expires_at: None,
                stale: false,
                attention_reason: None,
                attention_sequence: Some(OutputSequence::new(sequence)),
            },
            pinned: false,
            position: PositionKey::new(1),
            last_output_sequence: OutputSequence::new(sequence),
            read_through_sequence: OutputSequence::ZERO,
            unread_sequence: Some(OutputSequence::new(sequence)),
            archived_at: None,
            created_at: 1,
            updated_at: 1,
            revision: Revision::ZERO,
        }
    }

    #[test]
    fn activity_center_commits_in_app_before_platform_delivery_failure() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = NotificationRepository::open(fixture.path()).unwrap();
        let fake_state = Arc::new(Mutex::new(FakeState {
            delivered: 0,
            fail: true,
            permission: PermissionState::Granted,
            request_result: PermissionState::Granted,
            requests: 0,
        }));
        let mut center = ActivityCenterState::open_with_repository(
            repository,
            Box::new(FakePlatform {
                state: fake_state.clone(),
            }),
        );
        center.set_mode(NotificationMode::Os).unwrap();
        assert!(
            center
                .observe_transition(None, &session(1), false)
                .unwrap()
                .recorded
        );
        assert_eq!(center.visible_count(), 1);
        assert_eq!(fake_state.lock().unwrap().delivered, 0);
        drop(center);
        assert_eq!(
            NotificationRepository::open(fixture.path())
                .unwrap()
                .load()
                .unwrap()
                .ledger
                .records
                .len(),
            1
        );
    }

    #[test]
    fn activity_center_dismiss_does_not_change_session_read_state() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = NotificationRepository::open(fixture.path()).unwrap();
        let mut center = ActivityCenterState::open_with_repository(
            repository,
            Box::new(FakePlatform {
                state: Arc::new(Mutex::new(FakeState {
                    permission: PermissionState::Unavailable,
                    ..FakeState::default()
                })),
            }),
        );
        let session = session(1);
        center.observe_transition(None, &session, false).unwrap();
        let link = center.records().next().unwrap().deep_link;
        assert!(center.dismiss(link).unwrap());
        assert_eq!(center.visible_count(), 0);
        assert!(session.unread());
    }

    #[test]
    fn activity_center_age_boundaries_are_localized() {
        let now = 400 * 24 * 60 * 60 * 1_000;
        assert_eq!(localized_activity_age(now, now), "just now");
        assert_eq!(localized_activity_age(now - 60_000, now), "1 minute ago");
        assert_eq!(localized_activity_age(now - 3_600_000, now), "1 hour ago");
        assert_eq!(localized_activity_age(now - 86_400_000, now), "yesterday");
        assert_eq!(
            localized_activity_age(now - 2 * 86_400_000, now),
            "2 days ago"
        );
        assert_eq!(
            localized_activity_age(now - 14 * 86_400_000, now),
            "2 weeks ago"
        );
    }

    #[test]
    fn activity_center_reloads_once_when_another_window_commits_first() {
        let fixture = tempfile::tempdir().unwrap();
        let first_repository = NotificationRepository::open(fixture.path()).unwrap();
        let second_repository = NotificationRepository::open(fixture.path()).unwrap();
        let mut first = ActivityCenterState::open_with_repository(
            first_repository,
            Box::new(FakePlatform {
                state: Arc::new(Mutex::new(FakeState::default())),
            }),
        );
        let mut second = ActivityCenterState::open_with_repository(
            second_repository,
            Box::new(FakePlatform {
                state: Arc::new(Mutex::new(FakeState::default())),
            }),
        );

        assert!(
            first
                .observe_transition(None, &session(1), false)
                .unwrap()
                .recorded
        );
        assert!(
            second
                .observe_transition(None, &session(2), false)
                .unwrap()
                .recorded
        );
        assert_eq!(
            NotificationRepository::open(fixture.path())
                .unwrap()
                .load()
                .unwrap()
                .ledger
                .records
                .len(),
            2
        );
    }

    #[test]
    fn activity_center_does_not_repeat_a_denied_permission_request() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = NotificationRepository::open(fixture.path()).unwrap();
        let fake_state = Arc::new(Mutex::new(FakeState {
            permission: PermissionState::Unknown,
            request_result: PermissionState::Denied,
            ..FakeState::default()
        }));
        let mut center = ActivityCenterState::open_with_repository(
            repository,
            Box::new(FakePlatform {
                state: fake_state.clone(),
            }),
        );

        center.set_mode(NotificationMode::Os).unwrap();
        center.set_mode(NotificationMode::Os).unwrap();
        let state = fake_state.lock().unwrap();
        assert_eq!(state.requests, 1);
        assert_eq!(center.policy().permission, PermissionState::Denied);
    }

    #[test]
    fn activity_center_reconciles_an_asynchronous_permission_result() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = NotificationRepository::open(fixture.path()).unwrap();
        let fake_state = Arc::new(Mutex::new(FakeState {
            permission: PermissionState::Unknown,
            request_result: PermissionState::Unknown,
            ..FakeState::default()
        }));
        let mut center = ActivityCenterState::open_with_repository(
            repository,
            Box::new(FakePlatform {
                state: fake_state.clone(),
            }),
        );
        center.set_mode(NotificationMode::Os).unwrap();
        fake_state.lock().unwrap().permission = PermissionState::Granted;
        center.last_permission_poll = Instant::now() - Duration::from_secs(3);

        center.poll_permission_if_due().unwrap();
        assert_eq!(center.policy().permission, PermissionState::Granted);
    }
}
