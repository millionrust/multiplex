use std::sync::Arc;

use gpui_component::Disableable as _;
use termirust_domain::{
    ControllerCapabilities, ControllerCapability, ControllerDeviceId, HostIdentityPublic,
    HostIdentityState, PairedDeviceRecord, PairedDeviceStatus,
};
use termirust_store::{ControllerDeviceRepository, ControllerDeviceStoreError};

use crate::controller::devices::{ControllerDeviceService, NoControllerChannels};
use crate::controller::host_identity::{
    HostIdentityService, OldSecretDeletion, OsIdentityEntropy, OsSecretStore,
};

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteDevicesFailure {
    Corrupt,
    Newer,
    Permission,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
enum PairingUiState {
    Idle,
    Generating,
    Waiting,
    SasReady,
    SasMismatch,
    Expired,
    RateLimited,
    StorageFailure,
    Uncertain,
    Paired,
    Revoked,
}

pub(super) struct RemoteDevicesState {
    repository: Option<ControllerDeviceRepository>,
    identity_state: HostIdentityState,
    identity: Option<HostIdentityPublic>,
    devices: Vec<PairedDeviceRecord>,
    failure: Option<RemoteDevicesFailure>,
    route_available: bool,
    pairing_state: PairingUiState,
    editing_device_id: Option<ControllerDeviceId>,
}

impl RemoteDevicesState {
    #[cfg(not(test))]
    pub(super) fn open_default() -> Self {
        let repository = match crate::storage::controller_store_dir()
            .map_err(|_| RemoteDevicesFailure::Unavailable)
            .and_then(|root| ControllerDeviceRepository::open(root).map_err(classify_store_failure))
        {
            Ok(repository) => repository,
            Err(failure) => return Self::failed(failure),
        };
        let identity_service =
            HostIdentityService::new(repository.clone(), OsSecretStore, OsIdentityEntropy);
        let identity = match identity_service.load_or_create() {
            Ok(identity) => identity,
            Err(_) => return Self::failed(RemoteDevicesFailure::Permission),
        };
        match repository.load() {
            Ok(snapshot) => Self {
                repository: Some(repository),
                identity_state: identity.state,
                identity: identity.public,
                devices: snapshot.authority.devices,
                failure: None,
                route_available: false,
                pairing_state: PairingUiState::Idle,
                editing_device_id: None,
            },
            Err(error) => Self::failed(classify_store_failure(error)),
        }
    }

    #[cfg(test)]
    pub(super) fn open_default() -> Self {
        Self {
            repository: None,
            identity_state: HostIdentityState::Ready,
            identity: Some(HostIdentityPublic::new(
                termirust_domain::HostIdentityGeneration::INITIAL,
                termirust_domain::HostPublicKey([4; 32]),
            )),
            devices: Vec::new(),
            failure: None,
            route_available: false,
            pairing_state: PairingUiState::Idle,
            editing_device_id: None,
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    fn failed(failure: RemoteDevicesFailure) -> Self {
        Self {
            repository: None,
            identity_state: HostIdentityState::Lost,
            identity: None,
            devices: Vec::new(),
            failure: Some(failure),
            route_available: false,
            pairing_state: PairingUiState::StorageFailure,
            editing_device_id: None,
        }
    }

    fn refresh(&mut self) -> Result<(), ()> {
        let repository = self.repository.as_ref().ok_or(())?;
        let snapshot = repository.load().map_err(|error| {
            self.failure = Some(classify_store_failure(error));
        })?;
        self.identity_state = snapshot.authority.state;
        self.identity = snapshot.authority.identity;
        self.devices = snapshot.authority.devices;
        self.failure = None;
        Ok(())
    }

    fn revoke(&mut self, device_id: ControllerDeviceId) -> Result<(), ()> {
        let repository = self.repository.clone().ok_or(())?;
        ControllerDeviceService::new(repository, Arc::new(NoControllerChannels))
            .revoke(device_id)
            .map_err(|_| ())?;
        self.pairing_state = PairingUiState::Revoked;
        self.refresh()
    }

    fn toggle_input(&mut self, device_id: ControllerDeviceId) -> Result<(), ()> {
        let device = self
            .devices
            .iter()
            .find(|device| device.device_id == device_id)
            .ok_or(())?;
        let next = if device
            .capabilities
            .contains(ControllerCapability::SendInput)
        {
            ControllerCapabilities::default()
                .with(ControllerCapability::ObserveSessions)
                .with(ControllerCapability::AttachOutput)
        } else {
            device
                .capabilities
                .with(ControllerCapability::SendInput)
                .with(ControllerCapability::Resize)
        };
        let repository = self.repository.clone().ok_or(())?;
        ControllerDeviceService::new(repository, Arc::new(NoControllerChannels))
            .set_capabilities(device_id, next)
            .map_err(|_| ())?;
        self.refresh()
    }

    fn reset_identity(&mut self) -> Result<OldSecretDeletion, ()> {
        let repository = self.repository.clone().ok_or(())?;
        let outcome = HostIdentityService::new(repository, OsSecretStore, OsIdentityEntropy)
            .reset()
            .map_err(|_| ())?;
        self.identity_state = outcome.identity.state;
        self.identity = outcome.identity.public;
        self.refresh()?;
        Ok(outcome.deletion)
    }
}

impl TermiRustApp {
    pub(super) fn render_remote_devices_settings_card(&self, cx: &Context<Self>) -> Div {
        let content = v_flex()
            .gap_3()
            .child(self.render_remote_route_section())
            .child(self.settings_divider())
            .child(self.render_remote_identity_section(cx))
            .child(self.settings_divider())
            .child(self.render_trusted_remote_devices(cx))
            .child(self.settings_divider())
            .child(self.render_remote_identity_reset_section(cx));
        self.settings_section_card(
            localization::remote_devices_title(),
            localization::remote_devices_description(),
            content.into_any_element(),
        )
    }

    fn render_remote_route_section(&self) -> AnyElement {
        let add_disabled = !self.remote_devices.route_available;
        v_flex()
            .gap_2()
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
                                    .text_size(px(13.))
                                    .font_medium()
                                    .text_color(theme::text_main())
                                    .child(localization::remote_devices_route_label()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child(localization::remote_devices_route_off()),
                            ),
                    )
                    .child(
                        Button::new("remote-devices-add")
                            .debug_selector(|| "remote-devices-add".to_string())
                            .small()
                            .icon(IconName::Plus)
                            .label(localization::remote_devices_add_action())
                            .disabled(add_disabled),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::text_muted())
                    .child(pairing_ui_status(self.remote_devices.pairing_state)),
            )
            .when(add_disabled, |this| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(px(8.))
                        .bg(theme::with_alpha(theme::warning(), 0.08))
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child(localization::remote_devices_route_required()),
                )
            })
            .into_any_element()
    }

    fn render_remote_identity_section(&self, cx: &Context<Self>) -> AnyElement {
        let identity_status = remote_identity_status(
            self.remote_devices.identity_state,
            self.remote_devices.failure,
        );
        let fingerprint = self
            .remote_devices
            .identity
            .as_ref()
            .map(|identity| identity.fingerprint.canonical());
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child(localization::remote_devices_identity_label()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::text_muted())
                            .child(identity_status),
                    ),
            )
            .when_some(fingerprint, |this, fingerprint| {
                let copy_value = fingerprint.clone();
                this.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_family("monospace")
                                .text_color(theme::text_main())
                                .child(fingerprint),
                        )
                        .child(
                            Button::new("remote-devices-copy-fingerprint")
                                .small()
                                .icon(IconName::Copy)
                                .label(localization::remote_devices_copy_action())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_value.clone(),
                                    ));
                                    this.status_message =
                                        localization::remote_devices_fingerprint_copied();
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(localization::remote_devices_fingerprint_explanation()),
            )
            .into_any_element()
    }

    fn render_trusted_remote_devices(&self, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(localization::remote_devices_trusted_title()),
            )
            .when(self.remote_devices.devices.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child(localization::remote_devices_empty()),
                )
            })
            .children(
                self.remote_devices
                    .devices
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, device)| self.render_remote_device_row(index, device, cx)),
            )
            .into_any_element()
    }

    fn render_remote_identity_reset_section(&self, cx: &Context<Self>) -> AnyElement {
        let reset_matches = self
            .settings_inputs
            .remote_identity_reset
            .read(cx)
            .value()
            .as_ref()
            == "RESET";
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .font_medium()
                    .text_color(theme::danger())
                    .child(localization::remote_devices_reset_title()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(localization::remote_devices_reset_description()),
            )
            .child(Input::new(&self.settings_inputs.remote_identity_reset))
            .child(
                Button::new("remote-devices-reset")
                    .debug_selector(|| "remote-devices-reset".to_string())
                    .small()
                    .danger()
                    .icon(IconName::Delete)
                    .label(localization::remote_devices_reset_action())
                    .disabled(!reset_matches)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reset_remote_identity(cx);
                    })),
            )
            .into_any_element()
    }

    fn render_remote_device_row(
        &self,
        index: usize,
        device: PairedDeviceRecord,
        cx: &Context<Self>,
    ) -> AnyElement {
        let device_id = device.device_id;
        let revoked = device.status == PairedDeviceStatus::Revoked;
        let editing = self.remote_devices.editing_device_id == Some(device_id);
        let input_allowed = device
            .capabilities
            .contains(ControllerCapability::SendInput);
        let status = remote_device_status(device.status);
        let last_seen = device
            .last_seen_at
            .map(|value| {
                localization::remote_devices_last_seen(format_relative_time(value).as_str())
            })
            .unwrap_or_else(localization::remote_devices_never_seen);
        v_flex()
            .id(("remote-device-row", index))
            .gap_2()
            .py_3()
            .border_t_1()
            .border_color(theme::soft_border())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_medium()
                                    .text_color(theme::text_main())
                                    .child(device.display_name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(localization::remote_devices_device_detail(
                                        device.fingerprint_suffix(),
                                        status,
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(last_seen),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new(("remote-device-capabilities", index))
                                    .small()
                                    .label(if input_allowed {
                                        localization::remote_devices_restrict_input_action()
                                    } else {
                                        localization::remote_devices_allow_input_action()
                                    })
                                    .disabled(revoked)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_remote_device_input(device_id, cx);
                                    })),
                            )
                            .child(
                                Button::new(("remote-device-revoke", index))
                                    .small()
                                    .danger()
                                    .label(localization::remote_devices_revoke_action())
                                    .disabled(revoked)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.revoke_remote_device(device_id, cx);
                                    })),
                            ),
                    ),
            )
            .when(editing, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(
                            div()
                                .min_w(px(220.))
                                .flex_1()
                                .child(Input::new(&self.settings_inputs.remote_device_name)),
                        )
                        .child(
                            Button::new(("remote-device-name-save", index))
                                .small()
                                .label(localization::remote_devices_name_save_action())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_remote_device_name(device_id, cx);
                                })),
                        )
                        .child(
                            Button::new(("remote-device-name-cancel", index))
                                .small()
                                .label(localization::common_cancel())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remote_devices.editing_device_id = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(!editing, |this| {
                this.child(
                    Button::new(("remote-device-name-edit", index))
                        .small()
                        .label(localization::remote_devices_name_edit_action())
                        .disabled(revoked)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_remote_device_rename(
                                device_id,
                                device.display_name.clone(),
                                window,
                                cx,
                            );
                        })),
                )
            })
            .into_any_element()
    }

    fn start_remote_device_rename(
        &mut self,
        device_id: ControllerDeviceId,
        display_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remote_devices.editing_device_id = Some(device_id);
        self.settings_inputs
            .remote_device_name
            .update(cx, |state, cx| state.set_value(display_name, window, cx));
        cx.notify();
    }

    fn save_remote_device_name(&mut self, device_id: ControllerDeviceId, cx: &mut Context<Self>) {
        let display_name = self
            .settings_inputs
            .remote_device_name
            .read(cx)
            .value()
            .to_string();
        let result = self
            .remote_devices
            .repository
            .clone()
            .ok_or(())
            .and_then(|repository| {
                ControllerDeviceService::new(repository, Arc::new(NoControllerChannels))
                    .rename(device_id, display_name)
                    .map_err(|_| ())
            })
            .and_then(|()| self.remote_devices.refresh());
        if result.is_ok() {
            self.remote_devices.editing_device_id = None;
            self.status_message = localization::remote_devices_name_saved();
        } else {
            self.error_message = localization::remote_devices_operation_failed();
        }
        cx.notify();
    }

    fn revoke_remote_device(&mut self, device_id: ControllerDeviceId, cx: &mut Context<Self>) {
        if self.remote_devices.revoke(device_id).is_ok() {
            self.status_message = localization::remote_devices_revoked_notice();
        } else {
            self.error_message = localization::remote_devices_operation_failed();
        }
        cx.notify();
    }

    fn toggle_remote_device_input(
        &mut self,
        device_id: ControllerDeviceId,
        cx: &mut Context<Self>,
    ) {
        if self.remote_devices.toggle_input(device_id).is_ok() {
            self.status_message = localization::remote_devices_capabilities_saved();
        } else {
            self.error_message = localization::remote_devices_operation_failed();
        }
        cx.notify();
    }

    fn reset_remote_identity(&mut self, cx: &mut Context<Self>) {
        let confirmed = self
            .settings_inputs
            .remote_identity_reset
            .read(cx)
            .value()
            .as_ref()
            == "RESET";
        if !confirmed {
            self.error_message = localization::remote_devices_reset_confirmation_required();
            cx.notify();
            return;
        }
        match self.remote_devices.reset_identity() {
            Ok(OldSecretDeletion::Failed(_)) => {
                self.status_message = localization::remote_devices_reset_old_key_warning();
            }
            Ok(_) => self.status_message = localization::remote_devices_reset_complete(),
            Err(()) => self.error_message = localization::remote_devices_operation_failed(),
        }
        cx.notify();
    }
}

fn classify_store_failure(error: ControllerDeviceStoreError) -> RemoteDevicesFailure {
    match error {
        ControllerDeviceStoreError::Corrupt
        | ControllerDeviceStoreError::TooLarge
        | ControllerDeviceStoreError::UnsafeEntry
        | ControllerDeviceStoreError::Domain(_) => RemoteDevicesFailure::Corrupt,
        ControllerDeviceStoreError::Newer { .. } => RemoteDevicesFailure::Newer,
        ControllerDeviceStoreError::Io {
            kind: std::io::ErrorKind::PermissionDenied,
            ..
        } => RemoteDevicesFailure::Permission,
        ControllerDeviceStoreError::Io { .. }
        | ControllerDeviceStoreError::StaleRevision { .. }
        | ControllerDeviceStoreError::RevisionOverflow => RemoteDevicesFailure::Unavailable,
    }
}

fn remote_identity_status(
    state: HostIdentityState,
    failure: Option<RemoteDevicesFailure>,
) -> String {
    if let Some(failure) = failure {
        return match failure {
            RemoteDevicesFailure::Corrupt => localization::remote_devices_store_corrupt(),
            RemoteDevicesFailure::Newer => localization::remote_devices_store_newer(),
            RemoteDevicesFailure::Permission => localization::remote_devices_permission_denied(),
            RemoteDevicesFailure::Unavailable => localization::remote_devices_unavailable(),
        };
    }
    match state {
        HostIdentityState::Ready => localization::remote_devices_identity_ready(),
        HostIdentityState::Locked => localization::remote_devices_identity_locked(),
        HostIdentityState::PermissionDenied => localization::remote_devices_permission_denied(),
        HostIdentityState::Lost => localization::remote_devices_identity_lost(),
        HostIdentityState::ResetRequired => localization::remote_devices_reset_required(),
    }
}

fn remote_device_status(status: PairedDeviceStatus) -> String {
    match status {
        PairedDeviceStatus::Online => localization::remote_devices_status_online(),
        PairedDeviceStatus::Offline => localization::remote_devices_status_offline(),
        PairedDeviceStatus::Revoked => localization::remote_devices_status_revoked(),
    }
}

fn pairing_ui_status(state: PairingUiState) -> String {
    match state {
        PairingUiState::Idle => localization::remote_devices_pairing_idle(),
        PairingUiState::Generating => localization::remote_devices_pairing_generating(),
        PairingUiState::Waiting => localization::remote_devices_pairing_waiting(),
        PairingUiState::SasReady => localization::remote_devices_pairing_sas_ready(),
        PairingUiState::SasMismatch => localization::remote_devices_pairing_sas_mismatch(),
        PairingUiState::Expired => localization::remote_devices_pairing_expired(),
        PairingUiState::RateLimited => localization::remote_devices_pairing_rate_limited(),
        PairingUiState::StorageFailure => localization::remote_devices_pairing_storage_failure(),
        PairingUiState::Uncertain => localization::remote_devices_pairing_uncertain(),
        PairingUiState::Paired => localization::remote_devices_pairing_paired(),
        PairingUiState::Revoked => localization::remote_devices_pairing_revoked(),
    }
}

#[cfg(test)]
mod tests {
    use termirust_domain::{HostIdentityState, PairedDeviceStatus};

    use crate::ui::localization;

    use super::{
        PairingUiState, RemoteDevicesState, pairing_ui_status, remote_device_status,
        remote_identity_status,
    };

    #[test]
    fn remote_devices_add_controller_is_disabled_without_route() {
        let state = RemoteDevicesState::open_default();
        assert!(!state.route_available);
        assert!(state.devices.is_empty());
        assert_eq!(
            localization::remote_devices_route_required(),
            "Enable a LAN or SSH route first."
        );
    }

    #[test]
    fn remote_devices_reports_every_pairing_and_identity_recovery_state() {
        for state in [
            PairingUiState::Idle,
            PairingUiState::Generating,
            PairingUiState::Waiting,
            PairingUiState::SasReady,
            PairingUiState::SasMismatch,
            PairingUiState::Expired,
            PairingUiState::RateLimited,
            PairingUiState::StorageFailure,
            PairingUiState::Uncertain,
            PairingUiState::Paired,
            PairingUiState::Revoked,
        ] {
            assert!(!pairing_ui_status(state).is_empty());
        }
        for state in [
            HostIdentityState::Ready,
            HostIdentityState::Locked,
            HostIdentityState::PermissionDenied,
            HostIdentityState::Lost,
            HostIdentityState::ResetRequired,
        ] {
            assert!(!remote_identity_status(state, None).is_empty());
        }
    }

    #[test]
    fn remote_devices_safe_reset_and_status_labels_are_explicit() {
        assert_eq!(
            localization::remote_devices_reset_placeholder(),
            "Type RESET to confirm"
        );
        assert_eq!(remote_device_status(PairedDeviceStatus::Revoked), "Revoked");
        assert!(localization::remote_devices_reset_description().contains("all paired devices"));
    }
}
