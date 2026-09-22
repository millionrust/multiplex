//! What the interface shows about updates: a check shortly after launch and every few hours, a
//! background download when this copy can install it, "Restart to Update" at the right end of
//! the top bar once it is ready, and the controls in Settings → About.

use std::time::Duration;

use gpui::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};

use super::MultiplexApp;
use crate::ui::{localization, theme};
use crate::update::{self, Staged, Version};

/// Long enough that the first check never slows the window opening.
const FIRST_CHECK: Duration = Duration::from_secs(20);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug)]
pub(super) enum UpdateStatus {
    /// A development or test build, or updates switched off in the environment.
    Off,
    Idle,
    Checking,
    UpToDate,
    Downloading(Version),
    Ready(Staged),
    /// Newer, but this copy cannot install it: point at the release page.
    Manual {
        version: Version,
        page: String,
    },
    Failed,
}

pub(super) struct UpdateState {
    pub(super) status: UpdateStatus,
}

impl UpdateState {
    pub(super) fn open() -> Self {
        let status = if !update::updates_enabled() {
            UpdateStatus::Off
        } else if let Some(staged) = update::staging_dir().and_then(|dir| update::staged(&dir)) {
            UpdateStatus::Ready(staged)
        } else {
            UpdateStatus::Idle
        };
        Self { status }
    }

    fn busy(&self) -> bool {
        matches!(
            self.status,
            UpdateStatus::Off
                | UpdateStatus::Checking
                | UpdateStatus::Downloading(_)
                | UpdateStatus::Ready(_)
        )
    }
}

enum Found {
    Current,
    Download(update::Offer),
    Manual(update::Offer),
}

impl MultiplexApp {
    /// Checks shortly after launch and then every few hours, for as long as the window lives.
    pub(super) fn start_update_checks(&mut self, cx: &mut Context<Self>) {
        if matches!(self.updates.status, UpdateStatus::Off) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_CHECK).await;
            loop {
                if this
                    .update(cx, |this, cx| this.check_for_updates(cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
    }

    pub(super) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if self.updates.busy() {
            return;
        }
        let automatic = !self.saved.settings.manual_updates;
        self.updates.status = UpdateStatus::Checking;
        cx.notify();
        let check = cx.background_executor().spawn(async move {
            match update::check()? {
                None => Ok(Found::Current),
                Some(offer) if automatic && offer.package.is_some() => Ok(Found::Download(offer)),
                Some(offer) => Ok(Found::Manual(offer)),
            }
        });
        cx.spawn(async move |this, cx| {
            let found: Result<Found, update::UpdateError> = check.await;
            let offer = match found {
                Ok(Found::Download(offer)) => offer,
                other => {
                    let _ = this.update(cx, |this, cx| {
                        this.updates.status = match other {
                            Ok(Found::Manual(offer)) => UpdateStatus::Manual {
                                version: offer.version,
                                page: offer.page,
                            },
                            Ok(_) => UpdateStatus::UpToDate,
                            Err(_) => UpdateStatus::Failed,
                        };
                        cx.notify();
                    });
                    return;
                }
            };
            let Some(dir) = update::staging_dir() else {
                let _ = this.update(cx, |this, cx| {
                    this.updates.status = UpdateStatus::Failed;
                    cx.notify();
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.updates.status = UpdateStatus::Downloading(offer.version);
                cx.notify();
            });
            let page = offer.page.clone();
            let version = offer.version;
            let downloaded = cx
                .background_executor()
                .spawn(async move { update::download(&offer, &dir) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.updates.status = match downloaded {
                    Ok(staged) => UpdateStatus::Ready(staged),
                    // The package did not arrive whole; the page still has it.
                    Err(update::UpdateError::ChecksumMismatch) => {
                        UpdateStatus::Manual { version, page }
                    }
                    Err(_) => UpdateStatus::Failed,
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Hands the staged package to the updater and quits; the updater opens the new version.
    pub(super) fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        let UpdateStatus::Ready(staged) = &self.updates.status else {
            return;
        };
        if update::launch_apply(staged).is_err() {
            self.updates.status = UpdateStatus::Failed;
            cx.notify();
            return;
        }
        cx.quit();
    }

    fn set_manual_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        self.saved.settings.manual_updates = manual;
        self.save_settings();
        cx.notify();
    }

    /// "Restart to Update" or "Update available" at the right end of the top bar, when either
    /// applies.
    pub(super) fn render_update_chrome_button(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let (label, ready) = match &self.updates.status {
            UpdateStatus::Ready(_) => (localization::update_restart_action(), true),
            UpdateStatus::Manual { .. } => (localization::update_available_action(), false),
            _ => return None,
        };
        Some(
            div()
                .id("chrome-update")
                .debug_selector(|| "chrome-update".to_string())
                .flex_shrink_0()
                .h(px(theme::SHELL_TOOLBAR_BUTTON_SIZE))
                .px(px(theme::SPACE_4))
                .rounded(px(theme::CARD_RADIUS))
                .flex()
                .items_center()
                .cursor_pointer()
                .bg(if ready {
                    theme::accent()
                } else {
                    theme::with_alpha(theme::accent(), 0.18)
                })
                .hover(|style| style.opacity(0.9))
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .font_medium()
                .text_color(if ready {
                    theme::library_bg()
                } else {
                    theme::accent()
                })
                .child(label)
                .on_click(cx.listener(|this, _, _, cx| match &this.updates.status {
                    UpdateStatus::Ready(_) => this.restart_to_update(cx),
                    UpdateStatus::Manual { page, .. } => cx.open_url(page),
                    _ => {}
                }))
                .into_any_element(),
        )
    }

    /// The update rows of Settings → About.
    pub(super) fn render_update_settings(&self, cx: &Context<Self>) -> Div {
        let status = &self.updates.status;
        let message = match status {
            UpdateStatus::Off => Some(localization::update_status_development()),
            UpdateStatus::Idle => None,
            UpdateStatus::Checking => Some(localization::update_status_checking()),
            UpdateStatus::UpToDate => Some(localization::update_status_current()),
            UpdateStatus::Downloading(version) => Some(localization::update_status_downloading(
                &version.to_string(),
            )),
            UpdateStatus::Ready(staged) => Some(localization::update_status_ready(
                &staged.version.to_string(),
            )),
            UpdateStatus::Manual { version, .. } => {
                Some(localization::update_status_manual(&version.to_string()))
            }
            UpdateStatus::Failed => Some(localization::update_status_failed()),
        };
        let off = matches!(status, UpdateStatus::Off);
        v_flex()
            .gap_2()
            .pt_2()
            .when_some(message, |this, message| {
                this.child(
                    div()
                        .debug_selector(|| "about-update-status".to_string())
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_muted())
                        .child(message),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("about-check-updates")
                            .debug_selector(|| "about-check-updates".to_string())
                            .small()
                            .label(localization::update_check_action())
                            .disabled(self.updates.busy())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.check_for_updates(cx);
                            })),
                    )
                    .when(matches!(status, UpdateStatus::Ready(_)), |this| {
                        this.child(
                            Button::new("about-restart-update")
                                .debug_selector(|| "about-restart-update".to_string())
                                .small()
                                .primary()
                                .label(localization::update_restart_action())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.restart_to_update(cx);
                                })),
                        )
                    })
                    .when_some(
                        match status {
                            UpdateStatus::Manual { page, .. } => Some(page.clone()),
                            _ => None,
                        },
                        |this, page| {
                            this.child(
                                Button::new("about-open-release")
                                    .debug_selector(|| "about-open-release".to_string())
                                    .small()
                                    .label(localization::update_open_page_action())
                                    .on_click(move |_, _, cx| cx.open_url(&page)),
                            )
                        },
                    ),
            )
            .when(!off, |this| {
                this.child(self.settings_choice_row(
                    localization::update_automatic_label(),
                    localization::update_automatic_description(),
                    self.segmented_control(
                        "about-automatic-updates",
                        [
                            (false, localization::update_automatic_on()),
                            (true, localization::update_automatic_off()),
                        ],
                        self.saved.settings.manual_updates,
                        false,
                        cx,
                        |this, manual, _, cx| this.set_manual_updates(manual, cx),
                    ),
                ))
            })
    }
}
