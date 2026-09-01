use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, SharedString, Styled, Window, div, px, relative,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Disableable, IconName, Sizable, StyledExt as _, h_flex, v_flex};
use zeroize::Zeroizing;

use super::{TermiRustApp, app_icon};
use crate::models::{DEFAULT_VAULT_ID, IdentitySource, SavedIdentity, identity_id_for_path};
use crate::sftp::{
    AuthorizedKeyAction, AuthorizedKeyEvent, AuthorizedKeyOutcome, GeneratedKeyVerification,
    spawn_authorized_key_operation,
};
use crate::ssh_keys::{PublicKeyMaterial, generate_ed25519_key_pair, record_ssh_key_audit};
use crate::storage::save_saved_state;
use crate::ui::app::ICON_KEY;
use crate::ui::theme;

pub(super) struct KeyLifecycleInputs {
    pub(super) label: Entity<InputState>,
    pub(super) comment: Entity<InputState>,
    pub(super) passphrase: Entity<InputState>,
    pub(super) passphrase_confirm: Entity<InputState>,
    pub(super) deployment_passphrase: Entity<InputState>,
}

impl KeyLifecycleInputs {
    pub(super) fn new(window: &mut Window, cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            label: cx.new(|cx| InputState::new(window, cx).placeholder("Key label")),
            comment: cx
                .new(|cx| InputState::new(window, cx).placeholder("Optional public key comment")),
            passphrase: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Passphrase (recommended)")
            }),
            passphrase_confirm: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Confirm passphrase")
            }),
            deployment_passphrase: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Private key passphrase, if set")
            }),
        }
    }
}

#[derive(Clone)]
pub(super) enum KeyLifecycleDialog {
    Generate,
    Generating,
    Generated {
        identity_id: String,
        fingerprint: String,
    },
    HostPicker {
        identity_id: String,
        action: AuthorizedKeyAction,
    },
    Review {
        identity_id: String,
        profile_id: String,
        action: AuthorizedKeyAction,
    },
    Running {
        action: AuthorizedKeyAction,
    },
    Result {
        profile_id: String,
        action: AuthorizedKeyAction,
        fingerprint: String,
        message: String,
        success: bool,
    },
}

impl TermiRustApp {
    pub(super) fn open_key_generation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input_value(&self.key_lifecycle_inputs.label, "", window, cx);
        Self::set_input_value(&self.key_lifecycle_inputs.comment, "", window, cx);
        Self::set_input_value(&self.key_lifecycle_inputs.passphrase, "", window, cx);
        Self::set_input_value(
            &self.key_lifecycle_inputs.passphrase_confirm,
            "",
            window,
            cx,
        );
        self.key_lifecycle_dialog = Some(KeyLifecycleDialog::Generate);
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn close_key_lifecycle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.key_lifecycle_dialog,
            Some(KeyLifecycleDialog::Generating | KeyLifecycleDialog::Running { .. })
        ) {
            return;
        }
        Self::set_input_value(&self.key_lifecycle_inputs.passphrase, "", window, cx);
        Self::set_input_value(
            &self.key_lifecycle_inputs.passphrase_confirm,
            "",
            window,
            cx,
        );
        Self::set_input_value(
            &self.key_lifecycle_inputs.deployment_passphrase,
            "",
            window,
            cx,
        );
        self.key_lifecycle_dialog = None;
        self.key_lifecycle_control = None;
        self.error_message.clear();
        cx.notify();
    }

    fn choose_generated_key_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let label = self
            .key_lifecycle_inputs
            .label
            .read(cx)
            .value()
            .trim()
            .to_string();
        let passphrase = self
            .key_lifecycle_inputs
            .passphrase
            .read(cx)
            .value()
            .to_string();
        let confirmation = self
            .key_lifecycle_inputs
            .passphrase_confirm
            .read(cx)
            .value()
            .to_string();
        if label.is_empty() {
            self.error_message = "Enter a label for the generated key.".to_string();
            cx.notify();
            return;
        }
        if passphrase != confirmation {
            self.error_message = "The key passphrases do not match.".to_string();
            cx.notify();
            return;
        }
        if let Some(path) = Self::take_dialog_path_for_tests() {
            self.generate_key_at_path(path, window, cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = rfd::AsyncFileDialog::new()
                .set_title("Save generated Ed25519 private key")
                .set_file_name("id_ed25519_termirust")
                .save_file()
                .await
                .map(|file| file.path().to_path_buf())
            else {
                return;
            };
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| app.generate_key_at_path(path, window, cx));
            });
        })
        .detach();
    }

    pub(super) fn generate_key_at_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self
            .key_lifecycle_inputs
            .label
            .read(cx)
            .value()
            .trim()
            .to_string();
        let comment = self
            .key_lifecycle_inputs
            .comment
            .read(cx)
            .value()
            .trim()
            .to_string();
        let passphrase = Zeroizing::new(
            self.key_lifecycle_inputs
                .passphrase
                .read(cx)
                .value()
                .to_string(),
        );
        Self::set_input_value(&self.key_lifecycle_inputs.passphrase, "", window, cx);
        Self::set_input_value(
            &self.key_lifecycle_inputs.passphrase_confirm,
            "",
            window,
            cx,
        );
        self.key_lifecycle_dialog = Some(KeyLifecycleDialog::Generating);
        self.status_message = "Generating an Ed25519 key with operating-system entropy...".into();
        self.error_message.clear();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    generate_ed25519_key_pair(
                        &path,
                        &comment,
                        (!passphrase.is_empty()).then_some(passphrase.as_str()),
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| match result {
                Ok(generated) => {
                    let path = generated.private_key_path.display().to_string();
                    let identity = SavedIdentity {
                        id: identity_id_for_path(&path),
                        label,
                        vault_id: Some(
                            app.selected_vault_id
                                .clone()
                                .unwrap_or_else(|| DEFAULT_VAULT_ID.to_string()),
                        ),
                        key_path: path,
                        kind: "ED25519".to_string(),
                        source: IdentitySource::Generated,
                    };
                    let identity_id = identity.id.clone();
                    app.saved.upsert_identity(identity);
                    app.key_lifecycle_dialog = Some(KeyLifecycleDialog::Generated {
                        identity_id,
                        fingerprint: generated.fingerprint.clone(),
                    });
                    if save_saved_state(&app.saved).is_err() {
                        app.status_message =
                            "The key files were generated, but Keychain metadata was not saved."
                                .to_string();
                        app.error_message = "The key files are intact at the selected destination. Add the private key file to Keychain before restarting TermiRust."
                            .to_string();
                    } else {
                        app.status_message = "Generated key added to the Keychain.".to_string();
                        app.error_message.clear();
                    }
                    cx.notify();
                }
                Err(error) => {
                    app.key_lifecycle_dialog = Some(KeyLifecycleDialog::Generate);
                    app.error_message = error.to_string();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn open_key_host_picker(
        &mut self,
        identity_id: String,
        action: AuthorizedKeyAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::set_input_value(
            &self.key_lifecycle_inputs.deployment_passphrase,
            "",
            window,
            cx,
        );
        self.key_lifecycle_dialog = Some(KeyLifecycleDialog::HostPicker {
            identity_id,
            action,
        });
        self.error_message.clear();
        cx.notify();
    }

    fn review_key_operation(
        &mut self,
        identity_id: String,
        profile_id: String,
        action: AuthorizedKeyAction,
        cx: &mut Context<Self>,
    ) {
        self.key_lifecycle_dialog = Some(KeyLifecycleDialog::Review {
            identity_id,
            profile_id,
            action,
        });
        self.error_message.clear();
        cx.notify();
    }

    fn start_key_operation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(KeyLifecycleDialog::Review {
            identity_id,
            profile_id,
            action,
        }) = self.key_lifecycle_dialog.clone()
        else {
            return;
        };
        let Some(identity) = self
            .saved
            .identities
            .iter()
            .find(|identity| identity.id == identity_id)
            .cloned()
        else {
            self.error_message = "The selected identity is no longer available.".into();
            cx.notify();
            return;
        };
        let Some(profile) = self
            .saved
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .cloned()
        else {
            self.error_message = "The selected host is no longer available.".into();
            cx.notify();
            return;
        };
        let public_path = public_key_path_for_identity(&identity.key_path);
        let public_key = match PublicKeyMaterial::from_file(&public_path) {
            Ok(key) => key,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };
        let operation_id = self.next_sftp_operation_id();
        let mut request = match self.connect_request_for_saved_canvas_host(&profile) {
            Ok(request) => request,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };
        request.session_id = operation_id;
        let passphrase = Zeroizing::new(
            self.key_lifecycle_inputs
                .deployment_passphrase
                .read(cx)
                .value()
                .to_string(),
        );
        Self::set_input_value(
            &self.key_lifecycle_inputs.deployment_passphrase,
            "",
            window,
            cx,
        );
        let verification = (action == AuthorizedKeyAction::Add).then(|| {
            GeneratedKeyVerification::new(
                PathBuf::from(&identity.key_path),
                (!passphrase.is_empty()).then(|| passphrase.to_string()),
            )
        });
        let fingerprint = public_key.fingerprint.clone();
        let (control, events) = match spawn_authorized_key_operation(
            operation_id,
            request,
            self.known_hosts.clone(),
            public_key,
            action,
            verification,
        ) {
            Ok(operation) => operation,
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };
        self.key_lifecycle_control = Some(control);
        self.key_lifecycle_dialog = Some(KeyLifecycleDialog::Running { action });
        self.status_message = match action {
            AuthorizedKeyAction::Add => "Installing and verifying the public key...",
            AuthorizedKeyAction::Remove => "Removing the exact public key...",
        }
        .to_string();
        self.error_message.clear();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let event = cx
                .background_executor()
                .spawn(async move { events.recv().map_err(|_| ()) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.finish_key_operation(profile_id, action, fingerprint, event, cx);
            });
        })
        .detach();
    }

    fn finish_key_operation(
        &mut self,
        profile_id: String,
        action: AuthorizedKeyAction,
        fallback_fingerprint: String,
        event: Result<AuthorizedKeyEvent, ()>,
        cx: &mut Context<Self>,
    ) {
        self.key_lifecycle_control = None;
        let (fingerprint, result_code, mut message, success) = match event {
            Ok(AuthorizedKeyEvent::Complete {
                fingerprint,
                outcome,
                ..
            }) => {
                let (code, message, success) = key_outcome_copy(outcome);
                (fingerprint, code, message, success)
            }
            Ok(AuthorizedKeyEvent::Error {
                fingerprint,
                message,
                ..
            }) => (fingerprint, "safely_rejected", message, false),
            Err(()) => (
                fallback_fingerprint,
                "unavailable",
                "The SSH key operation ended without a result.".to_string(),
                false,
            ),
        };
        if let Some(profile) = self.saved.profiles.iter().find(|p| p.id == profile_id)
            && record_ssh_key_audit(
                match action {
                    AuthorizedKeyAction::Add => "install",
                    AuthorizedKeyAction::Remove => "remove",
                },
                result_code,
                &fingerprint,
                &profile.id,
                &profile.endpoint(),
                &profile.username,
            )
            .is_err()
        {
            message.push_str(" The local audit record could not be saved.");
        }
        self.key_lifecycle_dialog = Some(KeyLifecycleDialog::Result {
            profile_id,
            action,
            fingerprint,
            message: message.clone(),
            success,
        });
        if success {
            self.status_message = message;
            self.error_message.clear();
        } else {
            self.error_message = message;
        }
        cx.notify();
    }

    pub(super) fn cancel_key_operation(&mut self, cx: &mut Context<Self>) {
        if let Some(control) = self.key_lifecycle_control.as_ref() {
            control.cancel();
            self.status_message = "Cancelling the SSH key operation...".to_string();
            cx.notify();
        }
    }

    pub(super) fn render_key_lifecycle_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.key_lifecycle_dialog.as_ref() else {
            return div().into_any_element();
        };
        let busy = matches!(
            dialog,
            KeyLifecycleDialog::Generating | KeyLifecycleDialog::Running { .. }
        );
        let title = match dialog {
            KeyLifecycleDialog::Generate | KeyLifecycleDialog::Generating => "Generate SSH key",
            KeyLifecycleDialog::Generated { .. } => "Key generated",
            KeyLifecycleDialog::HostPicker { action, .. } => action_title(*action),
            KeyLifecycleDialog::Review { action, .. }
            | KeyLifecycleDialog::Running { action, .. }
            | KeyLifecycleDialog::Result { action, .. } => action_title(*action),
        };

        div()
            .id("key-lifecycle-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(theme::SPACE_4))
            .bg(theme::modal_scrim())
            .child(
                v_flex()
                    .id("key-lifecycle-dialog")
                    .w(relative(0.96))
                    .max_w(px(theme::current_design_tokens()
                        .layout_security_dialog_maximum()
                        .0))
                    .max_h(relative(0.92))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .p(px(theme::SPACE_5))
                            .border_b_1()
                            .border_color(theme::border())
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        app_icon(ICON_KEY)
                                            .size(px(theme::ICON_SIZE_DEFAULT))
                                            .text_color(theme::accent()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                                            .font_semibold()
                                            .child(title),
                                    ),
                            )
                            .child(
                                Button::new("key-lifecycle-close")
                                    .icon(IconName::Close)
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_key_lifecycle(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .p(px(theme::SPACE_5))
                            .gap(px(theme::SPACE_4))
                            .when(
                                !self.error_message.is_empty()
                                    && !matches!(dialog, KeyLifecycleDialog::Result { .. }),
                                |this| this.child(key_notice(&self.error_message, true)),
                            )
                            .child(self.render_key_lifecycle_body(dialog, cx)),
                    )
                    .child(self.render_key_lifecycle_footer(dialog, cx)),
            )
            .into_any_element()
    }

    fn render_key_lifecycle_body(
        &self,
        dialog: &KeyLifecycleDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match dialog {
            KeyLifecycleDialog::Generate => v_flex()
                .gap_3()
                .child(key_notice(
                    "TermiRust creates an Ed25519 private key and matching .pub file at the destination you choose. Existing files are never overwritten.",
                    false,
                ))
                .child(lifecycle_field(
                    "Label",
                    Input::new(&self.key_lifecycle_inputs.label),
                ))
                .child(lifecycle_field(
                    "Public comment",
                    Input::new(&self.key_lifecycle_inputs.comment),
                ))
                .child(lifecycle_field(
                    "Passphrase",
                    Input::new(&self.key_lifecycle_inputs.passphrase),
                ))
                .child(lifecycle_field(
                    "Confirm passphrase",
                    Input::new(&self.key_lifecycle_inputs.passphrase_confirm),
                ))
                .child(key_notice(
                    "A blank passphrase is allowed, but an encrypted private key is safer if the file is copied or stolen.",
                    true,
                ))
                .into_any_element(),
            KeyLifecycleDialog::Generating => key_progress(
                "Generating key pair",
                "Writing the private and public files atomically with strict permissions.",
            ),
            KeyLifecycleDialog::Generated {
                identity_id,
                fingerprint,
            } => {
                let identity = self.saved.identities.iter().find(|i| i.id == *identity_id);
                let public_key = identity
                    .and_then(|identity| {
                        PublicKeyMaterial::from_file(&public_key_path_for_identity(
                            &identity.key_path,
                        ))
                        .ok()
                    })
                    .map(|key| key.openssh)
                    .unwrap_or_else(|| "Unavailable".to_string());
                v_flex()
                    .gap_3()
                    .child(key_notice(
                        "The private key remains only at the local destination you selected. Deploy sends the public key only.",
                        false,
                    ))
                    .child(lifecycle_review_row(
                        "Identity",
                        identity
                            .map(|i| i.label.clone())
                            .unwrap_or_else(|| "Unavailable".to_string()),
                    ))
                    .child(lifecycle_review_row("Fingerprint", fingerprint.clone()))
                    .child(lifecycle_public_key_preview(public_key))
                    .child(lifecycle_review_row(
                        "Private key",
                        identity
                            .map(|i| i.key_path.clone())
                            .unwrap_or_else(|| "Unavailable".to_string()),
                    ))
                    .into_any_element()
            }
            KeyLifecycleDialog::HostPicker {
                identity_id,
                action,
            } => {
                let identity_id = identity_id.clone();
                let action = *action;
                v_flex()
                    .gap_3()
                    .child(key_notice(
                        "Choose a saved SSH Connection. TermiRust uses that Connection's configured authentication and normal host-key verification for this operation.",
                        false,
                    ))
                    .children(self.saved.profiles.iter().enumerate().map(|(index, profile)| {
                        let identity_id = identity_id.clone();
                        let profile_id = profile.id.clone();
                        let available = profile.saved_auth_config().is_ok()
                            && !profile.host.trim().is_empty()
                            && !profile.username.trim().is_empty();
                        h_flex()
                            .id(("key-lifecycle-host", index))
                            .debug_selector(move || format!("key-lifecycle-host-{index}"))
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_3()
                            .border_b_1()
                            .border_color(theme::border())
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SIZE))
                                            .font_semibold()
                                            .child(profile.display_name()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(format!(
                                                "{}@{}:{} · {}",
                                                profile.username,
                                                profile.host,
                                                profile.port,
                                                profile.auth_mode.label()
                                            )),
                                    ),
                            )
                            .child(
                                Button::new(("key-lifecycle-choose-host", index))
                                    .debug_selector(move || {
                                        format!("key-lifecycle-choose-host-{index}")
                                    })
                                    .small()
                                    .label("Review")
                                    .disabled(!available)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.review_key_operation(
                                            identity_id.clone(),
                                            profile_id.clone(),
                                            action,
                                            cx,
                                        );
                                    })),
                            )
                            .into_any_element()
                    }))
                    .when(self.saved.profiles.is_empty(), |this| {
                        this.child(key_notice(
                            "No saved SSH Connections are available. Add and configure a Connection first.",
                            true,
                        ))
                    })
                    .into_any_element()
            }
            KeyLifecycleDialog::Review {
                identity_id,
                profile_id,
                action,
            } => self.render_key_operation_review(identity_id, profile_id, *action),
            KeyLifecycleDialog::Running { action, .. } => key_progress(
                match action {
                    AuthorizedKeyAction::Add => "Installing and verifying key",
                    AuthorizedKeyAction::Remove => "Removing exact key",
                },
                "You can cancel. TermiRust will finish or clean up any remote mutation already in progress.",
            ),
            KeyLifecycleDialog::Result {
                profile_id,
                fingerprint,
                message,
                success,
                ..
            } => {
                let profile = self.saved.profiles.iter().find(|p| p.id == *profile_id);
                v_flex()
                    .gap_3()
                    .child(key_notice(message, !success))
                    .child(lifecycle_review_row(
                        "Connection",
                        profile
                            .map(|profile| profile.display_name())
                            .unwrap_or_else(|| "Unavailable".to_string()),
                    ))
                    .child(lifecycle_review_row("Fingerprint", fingerprint.clone()))
                    .into_any_element()
            }
        }
    }

    fn render_key_operation_review(
        &self,
        identity_id: &str,
        profile_id: &str,
        action: AuthorizedKeyAction,
    ) -> AnyElement {
        let identity = self.saved.identities.iter().find(|i| i.id == identity_id);
        let profile = self.saved.profiles.iter().find(|p| p.id == profile_id);
        let public_key = identity.and_then(|identity| {
            PublicKeyMaterial::from_file(&public_key_path_for_identity(&identity.key_path)).ok()
        });
        let fingerprint = public_key
            .as_ref()
            .map(|key| key.fingerprint.clone())
            .unwrap_or_else(|| "Unavailable".to_string());
        let public_preview = public_key
            .map(|key| key.openssh)
            .unwrap_or_else(|| "Unavailable".to_string());
        let destination = "Authenticated user's ~/.ssh/authorized_keys";
        v_flex()
            .gap_3()
            .child(key_notice(
                match action {
                    AuthorizedKeyAction::Add => "Review the exact target before installing. Success is reported as verified only after a separate fresh login using this private key.",
                    AuthorizedKeyAction::Remove => "Removal deletes only the matching decoded key. It does not verify that another login method will remain available.",
                },
                action == AuthorizedKeyAction::Remove,
            ))
            .child(lifecycle_review_row(
                "Connection",
                profile
                    .map(|profile| profile.display_name())
                    .unwrap_or_else(|| "Unavailable".to_string()),
            ))
            .child(lifecycle_review_row(
                "Target",
                profile
                    .map(|profile| format!("{}@{}:{}", profile.username, profile.host, profile.port))
                    .unwrap_or_else(|| "Unavailable".to_string()),
            ))
            .child(lifecycle_review_row(
                "Operation auth",
                profile
                    .map(|profile| profile.auth_mode.label())
                    .unwrap_or("Unavailable"),
            ))
            .child(lifecycle_review_row("Destination", destination))
            .child(lifecycle_review_row("Fingerprint", fingerprint))
            .child(lifecycle_public_key_preview(public_preview))
            .when(action == AuthorizedKeyAction::Add, |this| {
                this.child(lifecycle_field(
                    "Private key passphrase",
                    Input::new(&self.key_lifecycle_inputs.deployment_passphrase),
                ))
            })
            .into_any_element()
    }

    fn render_key_lifecycle_footer(
        &self,
        dialog: &KeyLifecycleDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .justify_end()
            .gap_2()
            .p(px(theme::SPACE_4))
            .border_t_1()
            .border_color(theme::border())
            .when(matches!(dialog, KeyLifecycleDialog::Generate), |this| {
                this.child(
                    Button::new("key-lifecycle-generate")
                        .debug_selector(|| "key-lifecycle-generate".to_string())
                        .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                        .icon(IconName::Plus)
                        .label("Choose Destination")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.choose_generated_key_destination(window, cx);
                        })),
                )
            })
            .when_some(
                match dialog {
                    KeyLifecycleDialog::Generated { identity_id, .. } => Some(identity_id.clone()),
                    _ => None,
                },
                |this, identity_id| {
                    this.child(
                        Button::new("key-lifecycle-deploy-generated")
                            .debug_selector(|| "key-lifecycle-deploy-generated".to_string())
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .label("Deploy Public Key")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_key_host_picker(
                                    identity_id.clone(),
                                    AuthorizedKeyAction::Add,
                                    window,
                                    cx,
                                );
                            })),
                    )
                },
            )
            .when(
                matches!(dialog, KeyLifecycleDialog::Review { .. }),
                |this| {
                    let destructive = matches!(
                        dialog,
                        KeyLifecycleDialog::Review {
                            action: AuthorizedKeyAction::Remove,
                            ..
                        }
                    );
                    this.child(
                        Button::new("key-lifecycle-confirm")
                            .debug_selector(|| "key-lifecycle-confirm".to_string())
                            .custom(Self::action_button_style(
                                if destructive {
                                    theme::ActionTone::Danger
                                } else {
                                    theme::ActionTone::Accent
                                },
                                cx,
                            ))
                            .label(if destructive {
                                "Remove Exact Key"
                            } else {
                                "Install and Verify"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_key_operation(window, cx);
                            })),
                    )
                },
            )
            .when(
                matches!(dialog, KeyLifecycleDialog::Running { .. }),
                |this| {
                    this.child(
                        Button::new("key-lifecycle-cancel-operation")
                            .debug_selector(|| "key-lifecycle-cancel-operation".to_string())
                            .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_key_operation(cx);
                            })),
                    )
                },
            )
            .when(
                matches!(dialog, KeyLifecycleDialog::Result { .. }),
                |this| {
                    this.child(
                        Button::new("key-lifecycle-done")
                            .debug_selector(|| "key-lifecycle-done".to_string())
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .label("Done")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_key_lifecycle(window, cx);
                            })),
                    )
                },
            )
            .into_any_element()
    }
}

fn public_key_path_for_identity(private_key_path: &str) -> PathBuf {
    let path = Path::new(private_key_path);
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".pub");
    path.with_file_name(name)
}

fn action_title(action: AuthorizedKeyAction) -> &'static str {
    match action {
        AuthorizedKeyAction::Add => "Deploy public key",
        AuthorizedKeyAction::Remove => "Remove public key",
    }
}

fn key_outcome_copy(outcome: AuthorizedKeyOutcome) -> (&'static str, String, bool) {
    match outcome {
        AuthorizedKeyOutcome::InstalledAndVerified => (
            "installed_and_verified",
            "Public key installed and verified with a fresh key-only login.".to_string(),
            true,
        ),
        AuthorizedKeyOutcome::AlreadyPresentAndVerified => (
            "already_present_and_verified",
            "Public key was already present and a fresh key-only login succeeded.".to_string(),
            true,
        ),
        AuthorizedKeyOutcome::InstalledVerificationFailed => (
            "installed_verification_failed",
            "Public key was installed, but the fresh key-only login failed.".to_string(),
            false,
        ),
        AuthorizedKeyOutcome::AlreadyPresentVerificationFailed => (
            "already_present_verification_failed",
            "Public key was already present, but the fresh key-only login failed.".to_string(),
            false,
        ),
        AuthorizedKeyOutcome::Removed => (
            "removed",
            "The exact public key was removed; unrelated entries were preserved.".to_string(),
            true,
        ),
        AuthorizedKeyOutcome::NotPresent => (
            "not_present",
            "The exact public key was not present; no remote content changed.".to_string(),
            true,
        ),
        AuthorizedKeyOutcome::Cancelled => (
            "cancelled",
            "The SSH key operation was cancelled.".to_string(),
            false,
        ),
    }
}

fn lifecycle_field(label: &'static str, input: Input) -> AnyElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .font_medium()
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(input)
        .into_any_element()
}

fn lifecycle_review_row(label: &'static str, value: impl Into<gpui::SharedString>) -> AnyElement {
    let value: SharedString = value.into();
    h_flex()
        .justify_between()
        .items_start()
        .gap_4()
        .py_2()
        .border_b_1()
        .border_color(theme::border())
        .child(
            div()
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .max_w(relative(0.68))
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .text_color(theme::text_main())
                .child(value),
        )
        .into_any_element()
}

fn lifecycle_public_key_preview(value: impl Into<SharedString>) -> AnyElement {
    let value = value.into();
    v_flex()
        .gap_1()
        .py_2()
        .child(
            div()
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .text_color(theme::text_muted())
                .child("Public key"),
        )
        .child(
            div()
                .w_full()
                .overflow_x_scrollbar()
                .px_3()
                .py_2()
                .rounded(px(theme::CONTROL_RADIUS))
                .bg(theme::with_alpha(theme::hover(), 0.72))
                .border_1()
                .border_color(theme::border())
                .text_size(px(theme::TYPE_MICRO_SIZE))
                .font_family(theme::current_design_tokens().font_mono_family().0)
                .child(value),
        )
        .into_any_element()
}

fn key_notice(message: impl Into<gpui::SharedString>, warning: bool) -> AnyElement {
    let message: SharedString = message.into();
    div()
        .px_3()
        .py_3()
        .rounded(px(theme::CONTROL_RADIUS))
        .bg(theme::with_alpha(
            if warning {
                theme::warning()
            } else {
                theme::accent()
            },
            0.08,
        ))
        .border_1()
        .border_color(theme::with_alpha(
            if warning {
                theme::warning()
            } else {
                theme::accent()
            },
            0.28,
        ))
        .text_size(px(theme::TYPE_CAPTION_SIZE))
        .text_color(theme::text_main())
        .child(message)
        .into_any_element()
}

fn key_progress(title: &'static str, detail: &'static str) -> AnyElement {
    v_flex()
        .gap_2()
        .py_4()
        .items_center()
        .child(
            app_icon(ICON_KEY)
                .size(px(theme::ICON_SIZE_LARGE))
                .text_color(theme::accent()),
        )
        .child(
            div()
                .text_size(px(theme::TYPE_BODY_SIZE))
                .font_semibold()
                .child(title),
        )
        .child(
            div()
                .max_w(px(theme::current_design_tokens()
                    .layout_security_info_maximum()
                    .0))
                .text_center()
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .text_color(theme::text_muted())
                .child(detail),
        )
        .into_any_element()
}
