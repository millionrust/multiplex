use termirust_domain::{NotificationMode, PermissionState};

use super::*;

impl TermiRustApp {
    pub(super) fn render_notification_settings_card(&self, cx: &Context<Self>) -> Div {
        let policy = self.activity_center.policy();
        let permission = match policy.permission {
            PermissionState::Unknown => localization::notification_permission_unknown(),
            PermissionState::Granted => localization::notification_permission_granted(),
            PermissionState::Denied => localization::notification_permission_denied(),
            PermissionState::Unavailable => localization::notification_permission_unavailable(),
        };
        self.settings_section_card(
            localization::notification_settings_title(),
            localization::notification_settings_description(),
            v_flex()
                .gap_3()
                .child(
                    h_flex().gap_2().flex_wrap().children(
                        [
                            (NotificationMode::Off, localization::notification_mode_off()),
                            (
                                NotificationMode::InApp,
                                localization::notification_mode_in_app(),
                            ),
                            (NotificationMode::Os, localization::notification_mode_os()),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (mode, label))| {
                            Button::new(("notification-mode", index))
                                .small()
                                .custom(Self::action_button_style(
                                    if policy.mode == mode {
                                        theme::ActionTone::Accent
                                    } else {
                                        theme::ActionTone::Neutral
                                    },
                                    cx,
                                ))
                                .label(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_notification_mode(mode, cx);
                                }))
                                .into_any_element()
                        }),
                    ),
                )
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
                                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                        .font_medium()
                                        .text_color(theme::text_main())
                                        .child(localization::notification_recording_title()),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_muted())
                                        .child(localization::notification_recording_description()),
                                ),
                        )
                        .child(
                            Button::new("notification-recording-friendly")
                                .small()
                                .custom(Self::action_button_style(
                                    if policy.recording_friendly {
                                        theme::ActionTone::Accent
                                    } else {
                                        theme::ActionTone::Neutral
                                    },
                                    cx,
                                ))
                                .label(if policy.recording_friendly {
                                    localization::notification_toggle_on()
                                } else {
                                    localization::notification_toggle_off()
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_recording_friendly_notifications(
                                        !policy.recording_friendly,
                                        cx,
                                    );
                                })),
                        ),
                )
                .child(self.settings_divider())
                .child(
                    h_flex()
                        .items_center()
                        .flex_wrap()
                        .gap_3()
                        .child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(localization::notification_permission_status(permission)),
                        )
                        .when(policy.mode == NotificationMode::Os, |this| {
                            this.child(
                                Button::new("notification-refresh-permission")
                                    .small()
                                    .label(localization::notification_refresh_action())
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh_notification_permission(cx);
                                    })),
                            )
                        })
                        .when(
                            self.activity_center.failure()
                                == Some(activity_center::ActivityCenterFailure::Corrupt),
                            |this| {
                                this.child(
                                    Button::new("notification-reset-store")
                                        .small()
                                        .label(localization::notification_reset_action())
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Danger,
                                            cx,
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reset_notification_store(cx);
                                        })),
                                )
                            },
                        ),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::with_alpha(theme::accent(), 0.08))
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::notification_preview()),
                ),
        )
    }

    fn update_notification_mode(&mut self, mode: NotificationMode, cx: &mut Context<Self>) {
        match self.activity_center.set_mode(mode) {
            Ok(()) => self.status_message = localization::notification_settings_saved(),
            Err(_) => self.error_message = localization::activity_center_operation_failed(),
        }
        cx.notify();
    }

    fn update_recording_friendly_notifications(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match self.activity_center.set_recording_friendly(enabled) {
            Ok(()) => self.status_message = localization::notification_settings_saved(),
            Err(_) => self.error_message = localization::activity_center_operation_failed(),
        }
        cx.notify();
    }

    fn refresh_notification_permission(&mut self, cx: &mut Context<Self>) {
        match self.activity_center.refresh_permission() {
            Ok(_) => self.status_message = localization::notification_permission_refreshed(),
            Err(_) => self.error_message = localization::activity_center_operation_failed(),
        }
        cx.notify();
    }

    fn reset_notification_store(&mut self, cx: &mut Context<Self>) {
        match self.activity_center.reset_after_corruption() {
            Ok(()) => self.status_message = localization::notification_reset_complete(),
            Err(_) => self.error_message = localization::activity_center_operation_failed(),
        }
        cx.notify();
    }
}
