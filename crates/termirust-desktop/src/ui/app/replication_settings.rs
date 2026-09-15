use super::*;
use crate::replication::{
    DesktopReplicationDeletionReview, DesktopReplicationDevice, replication_has_pending_enrollment,
};
use gpui_component::Disableable as _;

pub(super) struct ReplicationSettingsInputs {
    pub owner_request: Entity<InputState>,
    pub joining_bundle: Entity<InputState>,
    pub verification_code: Entity<InputState>,
    pub authority_update: Entity<InputState>,
    pub deletion_confirmation: Entity<InputState>,
}

impl ReplicationSettingsInputs {
    pub(super) fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            owner_request: cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .auto_grow(2, 5)
                    .placeholder(localization::static_message(
                        MessageId::SettingsSecureSyncEnrollmentRequestPlaceholder,
                    ))
            }),
            joining_bundle: cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .auto_grow(2, 5)
                    .placeholder(localization::static_message(
                        MessageId::SettingsSecureSyncEnrollmentBundlePlaceholder,
                    ))
            }),
            verification_code: cx.new(|cx| {
                InputState::new(window, cx).placeholder(localization::static_message(
                    MessageId::SettingsSecureSyncVerificationCodePlaceholder,
                ))
            }),
            authority_update: cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .auto_grow(2, 5)
                    .placeholder(localization::static_message(
                        MessageId::SettingsSecureSyncAuthorityUpdatePlaceholder,
                    ))
            }),
            deletion_confirmation: cx.new(|cx| {
                InputState::new(window, cx).placeholder(DesktopReplication::<
                    OsReplicationSecretBackend,
                >::deletion_confirmation_phrase(
                ))
            }),
        }
    }
}

#[derive(Default)]
pub(super) struct ReplicationLifecycleState {
    pub status: Option<termirust_store::ReplicationProductStatus>,
    pub devices: Vec<DesktopReplicationDevice>,
    pub joining_request: Option<String>,
    pub enrollment_bundle: Option<String>,
    pub verification_code: Option<String>,
    pub authority_update: Option<String>,
    pub pending_authority_revision: Option<u64>,
    pub pending_revoke_device: Option<String>,
    pub deletion_review: Option<DesktopReplicationDeletionReview>,
}

impl TermiRustApp {
    fn replication_folder(&self) -> anyhow::Result<std::path::PathBuf> {
        self.saved
            .settings
            .sync_folder_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!(localization::static_message(
                    MessageId::SettingsSecureSyncFolderRequired,
                ))
            })
    }

    fn set_replication_error(
        &mut self,
        action: MessageId,
        error: impl std::fmt::Display,
        cx: &mut Context<Self>,
    ) {
        self.status_message.clear();
        self.error_message = localization::dynamic_user_data_message(
            MessageId::SettingsSecureSyncErrorDetail,
            vec![localization::static_message(action), error.to_string()],
        );
        cx.notify();
    }

    pub(super) fn reload_replication_lifecycle(&mut self) -> anyhow::Result<()> {
        let root = desktop_replication_root()?;
        if !replication_is_configured(&root) {
            self.replication_lifecycle.status = None;
            self.replication_lifecycle.devices.clear();
            self.replication_lifecycle.authority_update = None;
            self.replication_lifecycle.pending_authority_revision = None;
            if replication_has_pending_enrollment(&root) {
                self.replication_lifecycle.joining_request = Some(DesktopReplication::<
                    OsReplicationSecretBackend,
                >::pending_enrollment_request(
                    &root
                )?);
            }
            return Ok(());
        }

        let replication = self.secure_replication_service()?;
        self.replication_lifecycle.status = Some(replication.status()?);
        self.replication_lifecycle.devices = replication.devices();
        if let Some(update) = replication.pending_authority_package()? {
            self.replication_lifecycle.authority_update = Some(update.payload);
            self.replication_lifecycle.pending_authority_revision = Some(update.authority_revision);
        } else {
            self.replication_lifecycle.authority_update = None;
            self.replication_lifecycle.pending_authority_revision = None;
        }
        Ok(())
    }

    pub(super) fn refresh_replication_lifecycle(&mut self, cx: &mut Context<Self>) {
        match self.reload_replication_lifecycle() {
            Ok(()) => {
                self.error_message.clear();
                self.status_message =
                    localization::static_message(MessageId::SettingsSecureSyncDevicesRefreshed);
                cx.notify();
            }
            Err(error) => {
                self.set_replication_error(MessageId::SettingsSecureSyncRefreshFailed, error, cx)
            }
        }
    }

    pub(super) fn prepare_replication_enrollment(&mut self, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<String> {
            let root = desktop_replication_root()?;
            let folder = self.replication_folder()?;
            if replication_is_configured(&root) {
                anyhow::bail!(localization::static_message(
                    MessageId::SettingsSecureSyncAlreadyConfigured,
                ));
            }
            if replication_has_pending_enrollment(&root) {
                return DesktopReplication::<OsReplicationSecretBackend>::pending_enrollment_request(
                    &root,
                );
            }
            let request =
                DesktopReplication::prepare_enrollment(&root, folder, OsReplicationSecretBackend)?;
            String::from_utf8(request.to_canonical_bytes()?)
                .map_err(|_| anyhow::anyhow!("enrollment request is not canonical UTF-8"))
        })();
        match result {
            Ok(request) => {
                self.replication_lifecycle.joining_request = Some(request);
                self.error_message.clear();
                self.status_message =
                    localization::static_message(MessageId::SettingsSecureSyncEnrollmentPrepared);
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncEnrollmentPrepareFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn cancel_replication_enrollment(&mut self, cx: &mut Context<Self>) {
        let result = desktop_replication_root().and_then(|root| {
            DesktopReplication::<OsReplicationSecretBackend>::cancel_pending_enrollment(
                root,
                OsReplicationSecretBackend,
            )
        });
        match result {
            Ok(_) => {
                self.replication_lifecycle.joining_request = None;
                self.error_message.clear();
                self.status_message =
                    localization::static_message(MessageId::SettingsSecureSyncEnrollmentCancelled);
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncEnrollmentCancelFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn enroll_replication_device(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = self
            .replication_inputs
            .owner_request
            .read(cx)
            .value()
            .trim()
            .to_string();
        let result = (|| -> anyhow::Result<_> {
            if request.is_empty() {
                anyhow::bail!(localization::static_message(
                    MessageId::SettingsSecureSyncEnrollmentRequestRequired,
                ));
            }
            let mut replication = self.secure_replication_service()?;
            replication.enroll_text(&request)
        })();
        match result {
            Ok(package) => {
                Self::set_input_value(&self.replication_inputs.owner_request, "", window, cx);
                self.replication_lifecycle.enrollment_bundle = Some(package.bundle);
                self.replication_lifecycle.verification_code = Some(package.verification_code);
                self.replication_lifecycle.pending_authority_revision =
                    Some(package.authority_revision);
                let _ = self.reload_replication_lifecycle();
                self.error_message.clear();
                self.status_message = localization::static_message(
                    MessageId::SettingsSecureSyncEnrollmentBundleReady,
                );
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncEnrollDeviceFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn accept_replication_enrollment(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bundle = self
            .replication_inputs
            .joining_bundle
            .read(cx)
            .value()
            .trim()
            .to_string();
        let code = self
            .replication_inputs
            .verification_code
            .read(cx)
            .value()
            .trim()
            .to_string();
        let result = (|| -> anyhow::Result<()> {
            if bundle.is_empty() || code.is_empty() {
                anyhow::bail!(localization::static_message(
                    MessageId::SettingsSecureSyncBundleAndCodeRequired,
                ));
            }
            let root = desktop_replication_root()?;
            DesktopReplication::accept_enrollment_text(
                root,
                OsReplicationSecretBackend,
                &bundle,
                &code,
            )?;
            self.reload_replication_lifecycle()
        })();
        match result {
            Ok(()) => {
                Self::set_input_value(&self.replication_inputs.joining_bundle, "", window, cx);
                Self::set_input_value(&self.replication_inputs.verification_code, "", window, cx);
                self.replication_lifecycle.joining_request = None;
                self.error_message.clear();
                self.status_message =
                    localization::static_message(MessageId::SettingsSecureSyncEnrollmentAccepted);
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncEnrollmentAcceptFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn apply_replication_authority_update(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let payload = self
            .replication_inputs
            .authority_update
            .read(cx)
            .value()
            .trim()
            .to_string();
        let result = (|| -> anyhow::Result<()> {
            if payload.is_empty() {
                anyhow::bail!(localization::static_message(
                    MessageId::SettingsSecureSyncAuthorityUpdateRequired,
                ));
            }
            let mut replication = self.secure_replication_service()?;
            replication.apply_authority_package(&payload)?;
            self.reload_replication_lifecycle()
        })();
        match result {
            Ok(()) => {
                Self::set_input_value(&self.replication_inputs.authority_update, "", window, cx);
                self.error_message.clear();
                self.status_message = localization::static_message(
                    MessageId::SettingsSecureSyncAuthorityUpdateApplied,
                );
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncAuthorityUpdateFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn rotate_replication_keys(&mut self, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            let mut replication = self.secure_replication_service()?;
            let update = replication.rotate_keys()?;
            self.replication_lifecycle.authority_update = Some(update.payload);
            self.replication_lifecycle.pending_authority_revision = Some(update.authority_revision);
            self.reload_replication_lifecycle()
        })();
        match result {
            Ok(()) => {
                self.error_message.clear();
                self.status_message =
                    localization::static_message(MessageId::SettingsSecureSyncKeysRotated);
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncKeyRotationFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn stage_replication_device_revoke(
        &mut self,
        device_id: String,
        cx: &mut Context<Self>,
    ) {
        self.replication_lifecycle.pending_revoke_device = Some(device_id);
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn cancel_replication_device_revoke(&mut self, cx: &mut Context<Self>) {
        self.replication_lifecycle.pending_revoke_device = None;
        cx.notify();
    }

    pub(super) fn confirm_replication_device_revoke(&mut self, cx: &mut Context<Self>) {
        let Some(device_id) = self.replication_lifecycle.pending_revoke_device.clone() else {
            return;
        };
        let result = (|| -> anyhow::Result<()> {
            let mut replication = self.secure_replication_service()?;
            let update = replication.revoke_device(&device_id)?;
            self.replication_lifecycle.authority_update = Some(update.payload);
            self.replication_lifecycle.pending_authority_revision = Some(update.authority_revision);
            self.replication_lifecycle.pending_revoke_device = None;
            self.reload_replication_lifecycle()
        })();
        match result {
            Ok(()) => {
                self.error_message.clear();
                self.status_message =
                    localization::static_message(MessageId::SettingsSecureSyncDeviceRevoked);
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncDeviceRevokeFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn acknowledge_replication_authority_update(&mut self, cx: &mut Context<Self>) {
        let Some(revision) = self.replication_lifecycle.pending_authority_revision else {
            return;
        };
        let result = self
            .secure_replication_service()
            .and_then(|replication| replication.acknowledge_authority_package(revision));
        match result {
            Ok(_) => {
                self.replication_lifecycle.authority_update = None;
                self.replication_lifecycle.pending_authority_revision = None;
                self.replication_lifecycle.enrollment_bundle = None;
                self.replication_lifecycle.verification_code = None;
                self.error_message.clear();
                self.status_message = localization::static_message(
                    MessageId::SettingsSecureSyncAuthorityUpdateAcknowledged,
                );
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncAuthorityAcknowledgeFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn review_replication_deletion(&mut self, cx: &mut Context<Self>) {
        match self
            .secure_replication_service()
            .and_then(|replication| replication.deletion_review())
        {
            Ok(review) => {
                self.replication_lifecycle.deletion_review = Some(review);
                self.error_message.clear();
                self.status_message.clear();
                cx.notify();
            }
            Err(error) => self.set_replication_error(
                MessageId::SettingsSecureSyncDeletionReviewFailed,
                error,
                cx,
            ),
        }
    }

    pub(super) fn cancel_replication_deletion(&mut self, cx: &mut Context<Self>) {
        self.replication_lifecycle.deletion_review = None;
        cx.notify();
    }

    pub(super) fn delete_local_replication(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let confirmation = self
            .replication_inputs
            .deletion_confirmation
            .read(cx)
            .value()
            .to_string();
        let Some(review) = self.replication_lifecycle.deletion_review.take() else {
            return;
        };
        let result = self
            .secure_replication_service()
            .and_then(|replication| replication.delete_local_replica(&review, &confirmation));
        match result {
            Ok(()) => {
                Self::set_input_value(
                    &self.replication_inputs.deletion_confirmation,
                    "",
                    window,
                    cx,
                );
                self.replication_lifecycle = ReplicationLifecycleState::default();
                self.error_message.clear();
                self.status_message =
                    localization::static_message(MessageId::SettingsSecureSyncLocalDataDeleted);
                cx.notify();
            }
            Err(error) => {
                self.replication_lifecycle.deletion_review = Some(review);
                self.set_replication_error(
                    MessageId::SettingsSecureSyncLocalDeletionFailed,
                    error,
                    cx,
                );
            }
        }
    }

    fn copy_replication_payload(
        &mut self,
        payload: Option<String>,
        status: MessageId,
        cx: &mut Context<Self>,
    ) {
        if let Some(payload) = payload {
            cx.write_to_clipboard(ClipboardItem::new_string(payload));
            self.error_message.clear();
            self.status_message = localization::static_message(status);
            cx.notify();
        }
    }

    pub(super) fn render_replication_lifecycle(&self, cx: &Context<Self>) -> impl IntoElement {
        let root = desktop_replication_root().ok();
        let configured = root.as_deref().is_some_and(replication_is_configured);
        let pending_enrollment = root
            .as_deref()
            .is_some_and(replication_has_pending_enrollment);
        let owner = self
            .replication_lifecycle
            .status
            .as_ref()
            .is_some_and(|status| status.authority_owner);
        let exact_delete = self
            .replication_inputs
            .deletion_confirmation
            .read(cx)
            .value()
            == DesktopReplication::<OsReplicationSecretBackend>::deletion_confirmation_phrase();

        v_flex()
            .gap_3()
            .pt_3()
            .border_t_1()
            .border_color(theme::border())
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SIZE))
                            .text_color(theme::text_main())
                            .child(localization::static_message(
                                MessageId::SettingsSecureSyncDevicesTitle,
                            )),
                    )
                    .when(configured, |this| {
                        this.child(
                            Button::new("settings-secure-sync-refresh-devices")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(localization::static_message(
                                    MessageId::SettingsSecureSyncRefreshAction,
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.refresh_replication_lifecycle(cx);
                                })),
                        )
                    }),
            )
            .when(!configured, |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::static_message(
                            MessageId::SettingsSecureSyncJoinDescription,
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("settings-secure-sync-prepare-enrollment")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(localization::static_message(if pending_enrollment {
                                    MessageId::SettingsSecureSyncResumeEnrollmentAction
                                } else {
                                    MessageId::SettingsSecureSyncPrepareEnrollmentAction
                                }))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.prepare_replication_enrollment(cx);
                                })),
                        )
                        .when(pending_enrollment, |this| {
                            this.child(
                                Button::new("settings-secure-sync-cancel-enrollment")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Danger,
                                        cx,
                                    ))
                                    .label(localization::static_message(
                                        MessageId::SettingsSecureSyncCancelEnrollmentAction,
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_replication_enrollment(cx);
                                    })),
                            )
                        }),
                )
                .when_some(
                    self.replication_lifecycle.joining_request.clone(),
                    |this, request| {
                        this.child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::success())
                                        .child(localization::static_message(
                                            MessageId::SettingsSecureSyncEnrollmentRequestReady,
                                        )),
                                )
                                .child(
                                    Button::new("settings-secure-sync-copy-request")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncCopyRequestAction,
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.copy_replication_payload(
                                                Some(request.clone()),
                                                MessageId::SettingsSecureSyncRequestCopied,
                                                cx,
                                            );
                                        })),
                                ),
                        )
                    },
                )
                .child(Input::new(&self.replication_inputs.joining_bundle))
                .child(Input::new(&self.replication_inputs.verification_code))
                .child(
                    Button::new("settings-secure-sync-accept-enrollment")
                        .small()
                        .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                        .label(localization::static_message(
                            MessageId::SettingsSecureSyncAcceptEnrollmentAction,
                        ))
                        .disabled(!pending_enrollment)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.accept_replication_enrollment(window, cx);
                        })),
                )
            })
            .when(configured, |this| {
                this.when_some(self.replication_lifecycle.status.clone(), |this, status| {
                    this.child(
                        div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(localization::dynamic_user_data_message(
                                MessageId::SettingsSecureSyncDeviceSummary,
                                vec![
                                    status.active_devices.to_string(),
                                    status.total_devices.to_string(),
                                    status.key_epoch.to_string(),
                                    if status.authority_owner {
                                        localization::static_message(
                                            MessageId::SettingsSecureSyncOwnerRole,
                                        )
                                    } else {
                                        localization::static_message(
                                            MessageId::SettingsSecureSyncMemberRole,
                                        )
                                    },
                                ],
                            )),
                    )
                })
                .when(self.replication_lifecycle.status.is_none(), |this| {
                    this.child(
                        div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(localization::static_message(
                                MessageId::SettingsSecureSyncRefreshHint,
                            )),
                    )
                })
                .children(self.replication_lifecycle.devices.iter().enumerate().map(
                    |(index, device)| {
                        let device_id = device.id.clone();
                        h_flex()
                            .items_center()
                            .gap_2()
                            .py_1()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_main())
                                    .child(replication_device_label(device)),
                            )
                            .when(owner && device.active && !device.local, |this| {
                                this.child(
                                    Button::new(("settings-secure-sync-revoke-device", index))
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Danger,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncRevokeAction,
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.stage_replication_device_revoke(
                                                device_id.clone(),
                                                cx,
                                            );
                                        })),
                                )
                            })
                    },
                ))
                .when_some(
                    self.replication_lifecycle.pending_revoke_device.clone(),
                    |this, _| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    Button::new("settings-secure-sync-confirm-revoke")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Danger,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncConfirmRevokeAction,
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_replication_device_revoke(cx);
                                        })),
                                )
                                .child(
                                    Button::new("settings-secure-sync-cancel-revoke")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncCancelAction,
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_replication_device_revoke(cx);
                                        })),
                                ),
                        )
                    },
                )
                .when(owner, |this| {
                    this.child(Input::new(&self.replication_inputs.owner_request))
                        .child(
                            h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    Button::new("settings-secure-sync-enroll-device")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncAddDeviceAction,
                                        ))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.enroll_replication_device(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("settings-secure-sync-rotate-keys")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncRotateKeysAction,
                                        ))
                                        .disabled(
                                            self.replication_lifecycle
                                                .pending_authority_revision
                                                .is_some(),
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.rotate_replication_keys(cx);
                                        })),
                                ),
                        )
                })
                .when_some(
                    self.replication_lifecycle.enrollment_bundle.clone(),
                    |this, bundle| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    Button::new("settings-secure-sync-copy-bundle")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncCopyBundleAction,
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.copy_replication_payload(
                                                Some(bundle.clone()),
                                                MessageId::SettingsSecureSyncBundleCopied,
                                                cx,
                                            );
                                        })),
                                )
                                .when_some(
                                    self.replication_lifecycle.verification_code.clone(),
                                    |this, code| {
                                        this.child(
                                            div()
                                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                .text_color(theme::success())
                                                .child(localization::dynamic_user_data_message(
                                                    MessageId::SettingsSecureSyncVerificationCode,
                                                    vec![code.clone()],
                                                )),
                                        )
                                        .child(
                                            Button::new("settings-secure-sync-copy-code")
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Neutral,
                                                    cx,
                                                ))
                                                .label(localization::static_message(
                                                    MessageId::SettingsSecureSyncCopyCodeAction,
                                                ))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.copy_replication_payload(
                                                        Some(code.clone()),
                                                        MessageId::SettingsSecureSyncCodeCopied,
                                                        cx,
                                                    );
                                                })),
                                        )
                                    },
                                ),
                        )
                    },
                )
                .when_some(
                    self.replication_lifecycle.authority_update.clone(),
                    |this, update| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    Button::new("settings-secure-sync-copy-authority-update")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Accent,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncCopyAuthorityUpdateAction,
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.copy_replication_payload(
                                                Some(update.clone()),
                                                MessageId::SettingsSecureSyncAuthorityUpdateCopied,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    Button::new("settings-secure-sync-ack-authority-update")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncAcknowledgeDeliveryAction,
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.acknowledge_replication_authority_update(cx);
                                        })),
                                ),
                        )
                    },
                )
                .when(!owner, |this| {
                    this.child(Input::new(&self.replication_inputs.authority_update))
                        .child(
                            Button::new("settings-secure-sync-apply-authority-update")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(localization::static_message(
                                    MessageId::SettingsSecureSyncApplyAuthorityUpdateAction,
                                ))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_replication_authority_update(window, cx);
                                })),
                        )
                })
                .when(
                    self.replication_lifecycle.deletion_review.is_none(),
                    |this| {
                        this.child(
                            Button::new("settings-secure-sync-review-delete")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Danger, cx))
                                .label(localization::static_message(
                                    MessageId::SettingsSecureSyncDeleteLocalAction,
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.review_replication_deletion(cx);
                                })),
                        )
                    },
                )
                .when_some(
                    self.replication_lifecycle.deletion_review.as_ref(),
                    |this, review| {
                        this.child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::warning())
                                .child(localization::dynamic_user_data_message(
                                    MessageId::SettingsSecureSyncDeletionWarning,
                                    vec![
                                        review.record_count.to_string(),
                                        review.secret_count.to_string(),
                                        if review.authority_owner {
                                            localization::static_message(
                                                MessageId::SettingsSecureSyncOwnerRole,
                                            )
                                        } else {
                                            localization::static_message(
                                                MessageId::SettingsSecureSyncMemberRole,
                                            )
                                        },
                                    ],
                                )),
                        )
                        .child(Input::new(&self.replication_inputs.deletion_confirmation))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("settings-secure-sync-confirm-delete")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Danger,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncConfirmDeleteAction,
                                        ))
                                        .disabled(!exact_delete)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.delete_local_replication(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("settings-secure-sync-cancel-delete")
                                        .small()
                                        .custom(Self::action_button_style(
                                            theme::ActionTone::Neutral,
                                            cx,
                                        ))
                                        .label(localization::static_message(
                                            MessageId::SettingsSecureSyncCancelAction,
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_replication_deletion(cx);
                                        })),
                                ),
                        )
                    },
                )
            })
    }
}

fn replication_device_label(device: &DesktopReplicationDevice) -> String {
    let short_id = device
        .id
        .trim_start_matches("device-")
        .chars()
        .take(8)
        .collect::<String>();
    let status = if device.active {
        localization::static_message(MessageId::SettingsSecureSyncDeviceActive)
    } else {
        localization::static_message(MessageId::SettingsSecureSyncDeviceRevokedStatus)
    };
    let location = if device.local {
        localization::static_message(MessageId::SettingsSecureSyncThisDevice)
    } else {
        localization::static_message(MessageId::SettingsSecureSyncOtherDevice)
    };
    localization::dynamic_user_data_message(
        MessageId::SettingsSecureSyncDeviceRow,
        vec![short_id, location, status],
    )
}
