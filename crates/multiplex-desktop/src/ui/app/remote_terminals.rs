//! Remote Devices settings for terminals opened in other apps: whether paired devices see
//! tmux sessions, and the reviewed shell startup change that starts new terminals in tmux.

use std::path::PathBuf;

use gpui_component::Disableable as _;
use multiplex_tmux::Tmux;
use multiplex_tmux::shell_integration::{
    ChangePlan, DiffLine, FileChange, IntegrationError, IntegrationStatus, Shell, ShellIntegration,
};

use crate::controller::background_service::{self, ServiceError, ServiceStatus};

use super::*;

/// Installs and inspects the background listener. Tests substitute a fake so they never touch
/// launchd.
pub(super) trait BackgroundServiceControl {
    fn status(&self) -> ServiceStatus;
    fn install(&self) -> Result<(), ServiceError>;
    fn remove(&self) -> Result<(), ServiceError>;
}

#[cfg_attr(test, allow(dead_code))]
struct SystemBackgroundService;

impl BackgroundServiceControl for SystemBackgroundService {
    fn status(&self) -> ServiceStatus {
        background_service::status()
    }

    fn install(&self) -> Result<(), ServiceError> {
        background_service::install()
    }

    fn remove(&self) -> Result<(), ServiceError> {
        background_service::remove()
    }
}

#[cfg(test)]
struct UnsupportedBackgroundService;

#[cfg(test)]
impl BackgroundServiceControl for UnsupportedBackgroundService {
    fn status(&self) -> ServiceStatus {
        ServiceStatus::Unsupported
    }

    fn install(&self) -> Result<(), ServiceError> {
        Err(ServiceError::UNSUPPORTED)
    }

    fn remove(&self) -> Result<(), ServiceError> {
        Err(ServiceError::UNSUPPORTED)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RemoteTerminalChange {
    Enable,
    Disable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TmuxAvailability {
    Ready,
    Missing,
    TooOld,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RemoteTerminalVerification {
    Idle,
    Running,
    Passed,
    Failed,
}

pub(super) struct PendingRemoteTerminalChange {
    kind: RemoteTerminalChange,
    plan: ChangePlan,
}

pub(super) struct RemoteTerminalsState {
    home: Option<PathBuf>,
    login_shell: Option<String>,
    tmux: Option<Tmux>,
    availability: TmuxAvailability,
    status: IntegrationStatus,
    pending: Option<PendingRemoteTerminalChange>,
    verification: RemoteTerminalVerification,
    service: Box<dyn BackgroundServiceControl>,
    service_status: ServiceStatus,
}

/// The tmux path written into the startup file: the absolute path tmux was found at, such as
/// `/opt/homebrew/bin/tmux`, so a package manager upgrade does not strand the file on a
/// versioned directory that no longer exists. A relative path falls back to the resolved one.
fn startup_file_tmux_path(tmux: &Tmux) -> Option<PathBuf> {
    let found = tmux.executable();
    if found.is_absolute() {
        Some(found.to_path_buf())
    } else {
        tmux.canonical_executable().ok()
    }
}

impl RemoteTerminalsState {
    pub(super) fn open(home: Option<PathBuf>, login_shell: Option<String>) -> Self {
        let mut state = Self {
            home,
            login_shell,
            tmux: None,
            availability: TmuxAvailability::Unsupported,
            status: IntegrationStatus::Off,
            pending: None,
            verification: RemoteTerminalVerification::Idle,
            #[cfg(not(test))]
            service: Box::new(SystemBackgroundService),
            #[cfg(test)]
            service: Box::new(UnsupportedBackgroundService),
            service_status: ServiceStatus::Unsupported,
        };
        state.refresh();
        state
    }

    #[cfg(test)]
    pub(super) fn with_service(mut self, service: Box<dyn BackgroundServiceControl>) -> Self {
        self.service = service;
        self.service_status = self.service.status();
        self
    }

    #[cfg(test)]
    pub(super) fn service_status(&self) -> ServiceStatus {
        self.service_status
    }

    #[cfg(not(test))]
    pub(super) fn open_default() -> Self {
        Self::open(dirs::home_dir(), std::env::var("SHELL").ok())
    }

    /// Tests point the state at a temporary home explicitly.
    #[cfg(test)]
    pub(super) fn open_default() -> Self {
        Self::open(None, None)
    }

    /// Re-reads tmux and the user's files. Cheap: one `tmux -V` and a few small reads.
    pub(super) fn refresh(&mut self) {
        self.service_status = self.service.status();
        if cfg!(windows) || self.home.is_none() {
            self.tmux = None;
            self.availability = TmuxAvailability::Unsupported;
            self.status = IntegrationStatus::Off;
            return;
        }
        self.tmux = Tmux::discover().ok();
        self.availability = match &self.tmux {
            Some(tmux) if tmux.supports_ignore_size() => TmuxAvailability::Ready,
            Some(_) => TmuxAvailability::TooOld,
            None => TmuxAvailability::Missing,
        };
        self.status = self
            .integration()
            .map(|integration| integration.status())
            .unwrap_or(IntegrationStatus::Off);
    }

    #[cfg(test)]
    pub(super) fn availability(&self) -> TmuxAvailability {
        self.availability
    }

    #[cfg(test)]
    pub(super) fn status(&self) -> &IntegrationStatus {
        &self.status
    }

    #[cfg(test)]
    pub(super) fn has_pending_change(&self) -> bool {
        self.pending.is_some()
    }

    fn integration(&self) -> Option<ShellIntegration> {
        let home = self.home.clone()?;
        // Removal never needs tmux, so a missing tmux still lets people turn this off.
        let tmux = self
            .tmux
            .as_ref()
            .and_then(startup_file_tmux_path)
            .unwrap_or_else(|| PathBuf::from("tmux"));
        Some(ShellIntegration::new(home, tmux))
    }

    fn plan(&self, kind: RemoteTerminalChange) -> Result<ChangePlan, IntegrationError> {
        let integration = self
            .integration()
            .ok_or(IntegrationError::Io(std::io::ErrorKind::Unsupported))?;
        match kind {
            RemoteTerminalChange::Enable => {
                let mut shells = integration.target_shells(self.login_shell.as_deref());
                if shells.is_empty() {
                    shells.push(Shell::Zsh);
                }
                integration.plan_enable(&shells)
            }
            RemoteTerminalChange::Disable => integration.plan_disable(),
        }
    }
}

fn integration_error_message(error: &IntegrationError) -> String {
    match error {
        IntegrationError::Changed(_) => localization::remote_terminals_changed_error(),
        IntegrationError::MalformedBlock(_) => localization::remote_terminals_malformed_error(),
        IntegrationError::Io(_) | IntegrationError::NotAFile(_) => {
            localization::remote_terminals_write_error()
        }
    }
}

fn availability_message(availability: TmuxAvailability) -> Option<String> {
    match availability {
        TmuxAvailability::Ready => None,
        TmuxAvailability::Missing => Some(localization::remote_terminals_tmux_missing()),
        TmuxAvailability::TooOld => Some(localization::remote_terminals_tmux_too_old()),
        TmuxAvailability::Unsupported => Some(localization::remote_terminals_unsupported()),
    }
}

impl TermiRustApp {
    /// Brings wrapped tabs up to date with this version of the app, once at launch.
    ///
    /// The key bindings a wrapped tab relies on are global to a tmux server, so a server an
    /// earlier version started keeps that version's behavior — a click in a tab scrolled back,
    /// for one, used to leave copy mode and drop the reader at the bottom. Updating the server
    /// alone is not enough, and was the whole of this once: every new tab sources TermiRust's
    /// tmux configuration file, so an old file put the old bindings back the next time a tab
    /// opened. The file is brought up to date first, then the server. That file is TermiRust's
    /// own and says so; the user's shell startup files still change only through a reviewed plan.
    pub(super) fn refresh_wrapped_tmux_behavior(&mut self, cx: &mut Context<Self>) {
        // Tests never reach for the developer's own tmux server or files.
        #[cfg(not(test))]
        if !matches!(self.remote_terminals.status, IntegrationStatus::Off)
            && let Some(tmux) = self.remote_terminals.tmux.clone()
        {
            let integration = self.remote_terminals.integration();
            cx.background_executor()
                .spawn(async move {
                    if let Some(integration) = integration {
                        let _ = integration.refresh_managed_config();
                    }
                    let _ = tmux.apply_wrapped_session_appearance(true);
                })
                .detach();
        }
        #[cfg(test)]
        let _ = cx;
    }

    pub(super) fn update_remote_tmux_sessions(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.saved.settings.remote_tmux_sessions = enabled;
        self.save_settings();
        if self
            .remote_devices
            .set_tmux_sessions(enabled, &self.controller_coordinator)
            .is_ok()
        {
            self.status_message = localization::remote_terminals_discovery_saved();
            self.error_message.clear();
        } else {
            self.error_message = localization::remote_devices_operation_failed();
        }
        if self.remote_terminals.verification == RemoteTerminalVerification::Passed {
            self.remote_terminals.verification = RemoteTerminalVerification::Idle;
        }
        cx.notify();
    }

    pub(super) fn review_remote_terminal_change(
        &mut self,
        kind: RemoteTerminalChange,
        cx: &mut Context<Self>,
    ) {
        self.remote_terminals.refresh();
        if kind == RemoteTerminalChange::Enable
            && self.remote_terminals.availability != TmuxAvailability::Ready
        {
            self.remote_terminals.pending = None;
            self.error_message =
                availability_message(self.remote_terminals.availability).unwrap_or_default();
            cx.notify();
            return;
        }
        match self.remote_terminals.plan(kind) {
            Ok(plan) => {
                self.remote_terminals.pending = Some(PendingRemoteTerminalChange { kind, plan });
                self.error_message.clear();
            }
            Err(error) => {
                self.remote_terminals.pending = None;
                self.error_message = integration_error_message(&error);
            }
        }
        cx.notify();
    }

    pub(super) fn apply_remote_terminal_change(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.remote_terminals.pending.take() else {
            return;
        };
        match pending.plan.apply() {
            Ok(()) => {
                // New tabs pick up the setup file; the running server is updated here, so
                // turning the setup on hides open tabs' status bar and turning it off restores it.
                // Either way the scrollback override an earlier version set is cleared.
                // Tests never reach for the developer's own tmux server.
                #[cfg(not(test))]
                if let Some(tmux) = self.remote_terminals.tmux.clone() {
                    let setup_on = pending.kind == RemoteTerminalChange::Enable;
                    cx.background_executor()
                        .spawn(async move {
                            let _ = tmux.apply_wrapped_session_appearance(setup_on);
                        })
                        .detach();
                }
                self.status_message = match pending.kind {
                    RemoteTerminalChange::Enable => localization::remote_terminals_applied_notice(),
                    RemoteTerminalChange::Disable => {
                        localization::remote_terminals_removed_notice()
                    }
                };
                self.error_message.clear();
            }
            Err(error) => self.error_message = integration_error_message(&error),
        }
        self.remote_terminals.refresh();
        cx.notify();
    }

    pub(super) fn cancel_remote_terminal_change(&mut self, cx: &mut Context<Self>) {
        self.remote_terminals.pending = None;
        cx.notify();
    }

    pub(super) fn check_remote_terminal_setup(&mut self, cx: &mut Context<Self>) {
        self.remote_terminals.refresh();
        let tmux = match (
            self.remote_terminals.availability,
            self.remote_terminals.tmux.clone(),
        ) {
            (TmuxAvailability::Ready, Some(tmux)) => tmux,
            (availability, _) => {
                self.remote_terminals.verification = RemoteTerminalVerification::Failed;
                self.error_message = availability_message(availability).unwrap_or_default();
                cx.notify();
                return;
            }
        };
        self.remote_terminals.verification = RemoteTerminalVerification::Running;
        self.status_message = localization::remote_terminals_verify_running();
        self.error_message.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { tmux.verify_listing() })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.remote_terminals.verification = if result.is_ok() {
                    RemoteTerminalVerification::Passed
                } else {
                    RemoteTerminalVerification::Failed
                };
                app.status_message.clear();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn change_background_service(&mut self, install: bool, cx: &mut Context<Self>) {
        let result = if install {
            self.remote_terminals.service.install()
        } else {
            self.remote_terminals.service.remove()
        };
        match result {
            Ok(()) => {
                self.status_message = if install {
                    localization::remote_terminals_service_installed_notice()
                } else {
                    localization::remote_terminals_service_removed_notice()
                };
                self.error_message.clear();
            }
            Err(error) if error == ServiceError::UNSUPPORTED => {
                self.error_message = localization::remote_terminals_service_unsupported();
            }
            Err(_) => self.error_message = localization::remote_terminals_service_error(),
        }
        self.remote_terminals.service_status = self.remote_terminals.service.status();
        cx.notify();
    }

    pub(super) fn render_remote_terminals_section(&self, cx: &Context<Self>) -> AnyElement {
        let state = &self.remote_terminals;
        let sharing = self.saved.settings.remote_tmux_sessions;
        let installed = !matches!(state.status, IntegrationStatus::Off);
        let status_text = match state.status {
            IntegrationStatus::Off => localization::remote_terminals_status_off(),
            IntegrationStatus::On(_) => localization::remote_terminals_status_on(),
            IntegrationStatus::Partial => localization::remote_terminals_status_partial(),
        };
        let verification_text = match state.verification {
            RemoteTerminalVerification::Idle | RemoteTerminalVerification::Running => None,
            RemoteTerminalVerification::Passed if sharing => {
                Some((localization::remote_terminals_verify_ok(), theme::success()))
            }
            RemoteTerminalVerification::Passed => Some((
                localization::remote_terminals_verify_ok_hidden(),
                theme::warning(),
            )),
            RemoteTerminalVerification::Failed => Some((
                availability_message(state.availability)
                    .unwrap_or_else(localization::remote_terminals_verify_failed),
                theme::danger(),
            )),
        };
        v_flex()
            .id("remote-terminals")
            .debug_selector(|| "remote-terminals".to_string())
            .w_full()
            .min_w_0()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child(localization::remote_terminals_title()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(localization::remote_terminals_description()),
                    ),
            )
            .child(self.settings_choice_row(
                localization::remote_terminals_discovery_label(),
                localization::remote_terminals_discovery_description(),
                self.segmented_control(
                    "remote-terminals-sharing",
                    [
                        (true, localization::remote_terminals_discovery_show()),
                        (false, localization::remote_terminals_discovery_hide()),
                    ],
                    sharing,
                    false,
                    cx,
                    |this, enabled, _, cx| this.update_remote_tmux_sessions(enabled, cx),
                ),
            ))
            .child(self.settings_subhead(
                localization::remote_terminals_wrap_label(),
                localization::remote_terminals_wrap_description(),
            ))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div()
                            .debug_selector(|| "remote-terminals-status".to_string())
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(status_text),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("remote-terminals-review-enable")
                                    .debug_selector(|| "remote-terminals-review-enable".to_string())
                                    .small()
                                    .label(localization::remote_terminals_review_enable_action())
                                    .disabled(state.availability != TmuxAvailability::Ready)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.review_remote_terminal_change(
                                            RemoteTerminalChange::Enable,
                                            cx,
                                        );
                                    })),
                            )
                            .when(installed, |this| {
                                this.child(
                                    Button::new("remote-terminals-review-disable")
                                        .debug_selector(|| {
                                            "remote-terminals-review-disable".to_string()
                                        })
                                        .small()
                                        .label(
                                            localization::remote_terminals_review_disable_action(),
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.review_remote_terminal_change(
                                                RemoteTerminalChange::Disable,
                                                cx,
                                            );
                                        })),
                                )
                            })
                            .child(
                                Button::new("remote-terminals-verify")
                                    .debug_selector(|| "remote-terminals-verify".to_string())
                                    .small()
                                    .label(localization::remote_terminals_verify_action())
                                    .disabled(
                                        state.verification == RemoteTerminalVerification::Running,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.check_remote_terminal_setup(cx);
                                    })),
                            ),
                    ),
            )
            .when_some(availability_message(state.availability), |this, message| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::warning())
                        .child(message),
                )
            })
            .when_some(state.pending.as_ref(), |this, pending| {
                this.child(self.render_remote_terminal_preview(pending, cx))
            })
            .when_some(verification_text, |this, (message, color)| {
                this.child(
                    div()
                        .debug_selector(|| "remote-terminals-verification".to_string())
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(color)
                        .child(message),
                )
            })
            .child(self.settings_subhead(
                localization::remote_terminals_service_label(),
                localization::remote_terminals_service_description(),
            ))
            .child(self.render_background_service_row(cx))
            .child(
                div()
                    .text_size(px(theme::TYPE_MICRO_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::remote_terminals_no_wrap_hint()),
            )
            .into_any_element()
    }

    fn render_background_service_row(&self, cx: &Context<Self>) -> AnyElement {
        let status = self.remote_terminals.service_status;
        if status == ServiceStatus::Unsupported {
            return div()
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .text_color(theme::text_muted())
                .child(localization::remote_terminals_service_unsupported())
                .into_any_element();
        }
        let installed = status != ServiceStatus::NotInstalled;
        h_flex()
            .items_center()
            .justify_between()
            .flex_wrap()
            .gap_3()
            .child(
                div()
                    .debug_selector(|| "remote-terminals-service-status".to_string())
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(match status {
                        ServiceStatus::Running => {
                            localization::remote_terminals_service_status_running()
                        }
                        ServiceStatus::Installed => {
                            localization::remote_terminals_service_status_installed()
                        }
                        ServiceStatus::NotInstalled | ServiceStatus::Unsupported => {
                            localization::remote_terminals_service_status_off()
                        }
                    }),
            )
            .child(
                Button::new("remote-terminals-service-toggle")
                    .debug_selector(|| "remote-terminals-service-toggle".to_string())
                    .small()
                    .label(if installed {
                        localization::remote_terminals_service_remove_action()
                    } else {
                        localization::remote_terminals_service_install_action()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_background_service(!installed, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_remote_terminal_preview(
        &self,
        pending: &PendingRemoteTerminalChange,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mono = theme::current_design_tokens().font_mono_family().0;
        let body = if pending.plan.is_empty() {
            div()
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .text_color(theme::text_muted())
                .child(localization::remote_terminals_preview_empty())
                .into_any_element()
        } else {
            v_flex()
                .gap_3()
                .children(
                    pending
                        .plan
                        .changes
                        .iter()
                        .map(|change| render_file_change(change, mono)),
                )
                .into_any_element()
        };
        let (confirm_label, destructive) = match pending.kind {
            RemoteTerminalChange::Enable => (localization::remote_terminals_apply_action(), false),
            RemoteTerminalChange::Disable => (localization::remote_terminals_remove_action(), true),
        };
        v_flex()
            .debug_selector(|| "remote-terminals-preview".to_string())
            .gap_3()
            .p(px(theme::SPACE_3))
            .rounded(px(theme::CONTROL_RADIUS))
            .border_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_medium()
                    .text_color(theme::text_main())
                    .child(localization::remote_terminals_preview_title()),
            )
            .child(body)
            .child(
                h_flex()
                    .gap_2()
                    .when(!pending.plan.is_empty(), |this| {
                        this.child(
                            Button::new("remote-terminals-apply")
                                .debug_selector(|| "remote-terminals-apply".to_string())
                                .small()
                                .when(destructive, |button| button.danger())
                                .when(!destructive, |button| button.primary())
                                .label(confirm_label)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.apply_remote_terminal_change(cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("remote-terminals-cancel")
                            .debug_selector(|| "remote-terminals-cancel".to_string())
                            .small()
                            .label(localization::remote_terminals_cancel_action())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_remote_terminal_change(cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn render_file_change(change: &FileChange, mono: &'static str) -> AnyElement {
    let badge = if change.creates() {
        localization::remote_terminals_file_created()
    } else if change.deletes() {
        localization::remote_terminals_file_deleted()
    } else {
        localization::remote_terminals_file_edited()
    };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .font_family(mono)
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_main())
                        .child(change.path.display().to_string()),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_MICRO_SIZE))
                        .text_color(theme::text_muted())
                        .child(badge),
                ),
        )
        .child(
            v_flex()
                .p(px(theme::SPACE_2))
                .rounded(px(theme::CONTROL_RADIUS))
                .bg(theme::hover())
                .font_family(mono)
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .children(change.diff().into_iter().map(|line| {
                    let row = div().w_full();
                    match &line {
                        DiffLine::Added(_) => row
                            .bg(theme::with_alpha(theme::success(), 0.12))
                            .text_color(theme::text_main())
                            .child(line.gutter_text().unwrap_or_default()),
                        DiffLine::Removed(_) => row
                            .bg(theme::with_alpha(theme::danger(), 0.12))
                            .text_color(theme::text_main())
                            .child(line.gutter_text().unwrap_or_default()),
                        DiffLine::Context(_) => row
                            .text_color(theme::text_muted())
                            .child(line.gutter_text().unwrap_or_default()),
                        DiffLine::Skipped(count) => row
                            .italic()
                            .text_color(theme::text_muted())
                            .child(localization::remote_terminals_diff_skipped(*count)),
                    }
                })),
        )
        .into_any_element()
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::startup_file_tmux_path;

    #[test]
    fn the_startup_file_keeps_the_path_tmux_was_found_at_not_the_versioned_target() {
        let fixture = tempfile::tempdir().unwrap();
        let cellar = fixture.path().join("Cellar/tmux/3.7c/bin");
        std::fs::create_dir_all(&cellar).unwrap();
        let real = cellar.join("tmux");
        std::fs::write(&real, "#!/bin/sh\necho 'tmux 3.7c'\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = fixture.path().join("tmux");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let tmux = multiplex_tmux::Tmux::at(&link).unwrap();
        assert_eq!(startup_file_tmux_path(&tmux), Some(link));
    }
}
