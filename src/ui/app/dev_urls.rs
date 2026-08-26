use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, Div, FocusHandle, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, Stateful, Styled, Window, div, px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use termirust_domain::{
    DevUrlCandidate, HostInstanceId, HostedSessionId, LocalDevUrl, OpenUrlError,
};

use super::{SessionPane, TermiRustApp};
use crate::platform_open_url::{PlatformOpenUrl, system_platform_open_url};
use crate::ui::{localization, theme};

pub(super) struct DevUrlUiState {
    platform: Box<dyn PlatformOpenUrl>,
    pending: Option<PendingDevUrlOpen>,
    last_error: Option<(u64, OpenUrlError)>,
    confirmation_focus: Option<FocusHandle>,
}

#[derive(Clone)]
struct PendingDevUrlOpen {
    pane_id: u64,
    session_id: HostedSessionId,
    host_instance: HostInstanceId,
    candidate_id: u64,
    exact_url: LocalDevUrl,
}

#[derive(Clone)]
struct DevUrlHeaderProjection {
    candidate_id: u64,
    label: String,
    additional_count: usize,
    enabled: bool,
}

impl DevUrlUiState {
    pub(super) fn open_default(cx: &mut Context<TermiRustApp>) -> Self {
        Self {
            platform: system_platform_open_url(),
            pending: None,
            last_error: None,
            confirmation_focus: Some(cx.focus_handle()),
        }
    }

    pub(super) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn dispatch(&mut self, url: &LocalDevUrl) -> Result<(), OpenUrlError> {
        self.platform.open(url)
    }
}

impl TermiRustApp {
    pub(super) fn request_dev_url_open(
        &mut self,
        pane_id: u64,
        candidate_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let resolved = self.resolve_dev_url(pane_id, candidate_id);
        match resolved {
            Ok((session_id, host_instance, url)) if url.requires_confirmation() => {
                self.dev_url_ui.pending = Some(PendingDevUrlOpen {
                    pane_id,
                    session_id,
                    host_instance,
                    candidate_id,
                    exact_url: url,
                });
                self.dev_url_ui.last_error = None;
                self.error_message.clear();
                if let Some(focus) = self.dev_url_ui.confirmation_focus.as_ref() {
                    focus.focus(window);
                }
                cx.on_next_frame(window, |this, window, _| {
                    if this.dev_url_ui.has_pending() {
                        window.focus_next();
                    }
                });
            }
            Ok((_, _, url)) => self.dispatch_dev_url(pane_id, &url),
            Err(error) => self.report_dev_url_error(pane_id, error),
        }
        cx.notify();
    }

    pub(super) fn confirm_dev_url_open(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.dev_url_ui.pending.take() else {
            return;
        };
        match self.resolve_dev_url(pending.pane_id, pending.candidate_id) {
            Ok((session_id, host_instance, url))
                if session_id == pending.session_id
                    && host_instance == pending.host_instance
                    && url == pending.exact_url =>
            {
                self.dispatch_dev_url(pending.pane_id, &url);
            }
            Ok(_) => self.report_dev_url_error(pending.pane_id, OpenUrlError::Invalidated),
            Err(error) => self.report_dev_url_error(pending.pane_id, error),
        }
        cx.notify();
    }

    pub(super) fn cancel_dev_url_open(&mut self, cx: &mut Context<Self>) {
        if self.dev_url_ui.pending.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn dismiss_dev_url(
        &mut self,
        pane_id: u64,
        candidate_id: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(projection) = self.dev_url_projection_mut(pane_id)
            && projection.dismiss(candidate_id)
        {
            if self.dev_url_ui.pending.as_ref().is_some_and(|pending| {
                pending.pane_id == pane_id && pending.candidate_id == candidate_id
            }) {
                self.dev_url_ui.pending = None;
            }
            if self
                .dev_url_ui
                .last_error
                .is_some_and(|(error_pane_id, _)| error_pane_id == pane_id)
            {
                self.dev_url_ui.last_error = None;
            }
            cx.notify();
        }
    }

    pub(super) fn clear_dev_urls(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        if let Some(projection) = self.dev_url_projection_mut(pane_id) {
            projection.clear();
            if self
                .dev_url_ui
                .pending
                .as_ref()
                .is_some_and(|pending| pending.pane_id == pane_id)
            {
                self.dev_url_ui.pending = None;
            }
            if self
                .dev_url_ui
                .last_error
                .is_some_and(|(error_pane_id, _)| error_pane_id == pane_id)
            {
                self.dev_url_ui.last_error = None;
            }
            cx.notify();
        }
    }

    fn resolve_dev_url(
        &self,
        pane_id: u64,
        candidate_id: u64,
    ) -> Result<(HostedSessionId, HostInstanceId, LocalDevUrl), OpenUrlError> {
        let attached = self
            .pane(pane_id)
            .and_then(|pane| pane.app_attached.as_ref())
            .ok_or(OpenUrlError::SessionUnavailable)?;
        let host_instance = attached
            .dev_urls
            .host_instance()
            .ok_or(OpenUrlError::SessionUnavailable)?;
        let url = attached.dev_urls.resolve_for_open(
            attached.hosted_session_id,
            host_instance,
            candidate_id,
        )?;
        Ok((attached.hosted_session_id, host_instance, url))
    }

    fn dev_url_projection_mut(
        &mut self,
        pane_id: u64,
    ) -> Option<&mut termirust_client::DevUrlProjection> {
        self.pane_mut(pane_id)
            .and_then(|pane| pane.app_attached.as_mut())
            .map(|attached| &mut attached.dev_urls)
    }

    fn dispatch_dev_url(&mut self, pane_id: u64, url: &LocalDevUrl) {
        match self.dev_url_ui.dispatch(url) {
            Ok(()) => {
                if self
                    .dev_url_ui
                    .last_error
                    .is_some_and(|(error_pane_id, _)| error_pane_id == pane_id)
                {
                    self.dev_url_ui.last_error = None;
                }
                self.status_message = localization::dev_url_opened();
                self.error_message.clear();
            }
            Err(error) => self.report_dev_url_error(pane_id, error),
        }
    }

    fn report_dev_url_error(&mut self, pane_id: u64, error: OpenUrlError) {
        self.dev_url_ui.last_error = Some((pane_id, error));
        self.error_message = dev_url_error_message(error);
    }

    fn app_attached_pane_for_session(&self, session_id: HostedSessionId) -> Option<&SessionPane> {
        self.panes.iter().find(|pane| {
            pane.app_attached
                .as_ref()
                .is_some_and(|attached| attached.hosted_session_id == session_id)
        })
    }

    fn dev_url_header_projection(&self, pane: &SessionPane) -> Option<DevUrlHeaderProjection> {
        let attached = pane.app_attached.as_ref()?;
        let candidate = attached.dev_urls.latest()?;
        let recording_friendly = self.activity_center.policy().recording_friendly;
        let (label, _, _) = dev_url_display(candidate, recording_friendly);
        Some(DevUrlHeaderProjection {
            candidate_id: candidate.id,
            label,
            additional_count: attached.dev_urls.candidates().count().saturating_sub(1),
            enabled: attached.dev_urls.host_available(),
        })
    }

    pub(super) fn render_dev_url_header(
        &self,
        pane: &SessionPane,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let pane_id = pane.id;
        let projection = self.dev_url_header_projection(pane)?;
        let candidate_id = projection.candidate_id;
        Some(
            h_flex()
                .id(("dev-url-header", pane_id))
                .debug_selector(|| "dev-url-header".to_string())
                .min_w_0()
                .gap_1()
                .child(
                    Button::new(("dev-url-open-latest", pane_id))
                        .debug_selector(|| "dev-url-open-latest".to_string())
                        .small()
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .label(projection.label)
                        .disabled(!projection.enabled)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_dev_url_open(pane_id, candidate_id, window, cx);
                        })),
                )
                .when(projection.additional_count > 0, |this| {
                    this.child(
                        div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted_dark())
                            .child(localization::dev_url_count(projection.additional_count)),
                    )
                })
                .child(
                    Button::new(("dev-url-dismiss-latest", pane_id))
                        .debug_selector(|| "dev-url-dismiss-latest".to_string())
                        .small()
                        .ghost()
                        .icon(IconName::Close)
                        .tooltip(localization::dev_url_dismiss_action())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dismiss_dev_url(pane_id, candidate_id, cx);
                        })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_dev_url_inspector(
        &self,
        session_id: HostedSessionId,
        cx: &Context<Self>,
    ) -> AnyElement {
        let pane = self.app_attached_pane_for_session(session_id);
        let pane_id = pane.map(|pane| pane.id);
        let recording_friendly = self.activity_center.policy().recording_friendly;
        let (candidates, partial, available) = pane
            .and_then(|pane| pane.app_attached.as_ref())
            .map(|attached| {
                (
                    attached
                        .dev_urls
                        .candidates()
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>(),
                    attached.dev_urls.is_partial(),
                    attached.dev_urls.host_available(),
                )
            })
            .unwrap_or_default();

        v_flex()
            .id(("dev-url-inspector", session_id.as_uuid().as_u128() as u64))
            .debug_selector(|| "dev-url-inspector".to_string())
            .gap(px(theme::SPACE_2))
            .pt(px(theme::SPACE_2))
            .border_t_1()
            .border_color(theme::soft_border())
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE_2))
                    .child(
                        div()
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(localization::dev_url_inspector_title()),
                    )
                    .when_some(pane_id, |this, pane_id| {
                        this.child(
                            Button::new(("dev-url-clear", pane_id))
                                .debug_selector(|| "dev-url-clear".to_string())
                                .small()
                                .ghost()
                                .label(localization::dev_url_clear_action())
                                .disabled(candidates.is_empty())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.clear_dev_urls(pane_id, cx);
                                })),
                        )
                    }),
            )
            .when(candidates.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::dev_url_empty()),
                )
            })
            .when(partial, |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::warning())
                        .child(localization::dev_url_partial()),
                )
            })
            .when(!available && !candidates.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::warning())
                        .child(localization::dev_url_stale()),
                )
            })
            .when_some(
                self.dev_url_ui
                    .last_error
                    .and_then(|(error_pane_id, error)| {
                        (Some(error_pane_id) == pane_id).then_some(error)
                    }),
                |this, error| {
                    this.child(
                        div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::danger())
                            .child(dev_url_error_message(error)),
                    )
                },
            )
            .children(candidates.into_iter().filter_map(|candidate| {
                pane_id.map(|pane_id| {
                    self.render_dev_url_inspector_row(
                        pane_id,
                        candidate,
                        available,
                        recording_friendly,
                        cx,
                    )
                })
            }))
            .into_any_element()
    }

    fn render_dev_url_inspector_row(
        &self,
        pane_id: u64,
        candidate: DevUrlCandidate,
        available: bool,
        recording_friendly: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let candidate_id = candidate.id;
        let (label, path, hidden_parameters) = dev_url_display(&candidate, recording_friendly);

        h_flex()
            .id(("dev-url-row", candidate_id))
            .debug_selector(|| "dev-url-row".to_string())
            .items_start()
            .gap(px(theme::SPACE_2))
            .py_1()
            .border_b_1()
            .border_color(theme::soft_border())
            .child(
                Icon::new(IconName::ExternalLink)
                    .size(px(theme::SPACE_4))
                    .text_color(if available {
                        theme::accent()
                    } else {
                        theme::text_muted()
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .font_medium()
                            .text_color(theme::text_main())
                            .truncate()
                            .child(label),
                    )
                    .when_some(path, |this, path| {
                        this.child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .truncate()
                                .child(path),
                        )
                    })
                    .when(hidden_parameters, |this| {
                        this.child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::warning())
                                .child(localization::dev_url_hidden_parameters()),
                        )
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        Button::new(("dev-url-open", candidate_id))
                            .debug_selector(|| "dev-url-open".to_string())
                            .small()
                            .ghost()
                            .icon(IconName::ExternalLink)
                            .label(localization::common_open())
                            .disabled(!available)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.request_dev_url_open(pane_id, candidate_id, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("dev-url-dismiss", candidate_id))
                            .debug_selector(|| "dev-url-dismiss".to_string())
                            .small()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip(localization::dev_url_dismiss_action())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.dismiss_dev_url(pane_id, candidate_id, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_dev_url_confirmation(&self, cx: &Context<Self>) -> Stateful<Div> {
        let pending = self
            .dev_url_ui
            .pending
            .as_ref()
            .expect("confirmation renders only while a URL is pending");
        let confirmation_focus = self
            .dev_url_ui
            .confirmation_focus
            .as_ref()
            .expect("application URL confirmation focus must exist");
        div()
            .id("dev-url-confirm-overlay")
            .debug_selector(|| "dev-url-confirm-overlay".to_string())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(theme::SPACE_4))
            .bg(theme::modal_scrim())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel_dev_url_open(cx)),
            )
            .child(
                v_flex()
                    .id("dev-url-confirm-dialog")
                    .track_focus(confirmation_focus)
                    .tab_group()
                    .tab_stop(false)
                    .w(relative(0.96))
                    .max_w(px(theme::DIALOG_MAX_WIDTH))
                    .max_h(relative(0.92))
                    .overflow_y_scrollbar()
                    .gap(px(theme::SPACE_4))
                    .p(px(theme::SPACE_5))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(theme::warning())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(localization::dev_url_confirm_title()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::warning())
                            .child(localization::dev_url_confirm_warning()),
                    )
                    .child(
                        div()
                            .w_full()
                            .overflow_x_scrollbar()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .font_family("monospace")
                            .text_color(theme::text_main())
                            .child(localization::dev_url_confirm_exact(
                                pending.exact_url.as_str().to_string(),
                            )),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(px(theme::SPACE_2))
                            .child(
                                Button::new("dev-url-confirm-cancel")
                                    .debug_selector(|| "dev-url-confirm-cancel".to_string())
                                    .label(localization::common_cancel())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_dev_url_open(cx);
                                    })),
                            )
                            .child(
                                Button::new("dev-url-confirm-open")
                                    .debug_selector(|| "dev-url-confirm-open".to_string())
                                    .primary()
                                    .icon(IconName::ExternalLink)
                                    .label(localization::common_open())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_dev_url_open(cx);
                                    })),
                            ),
                    ),
            )
    }
}

fn dev_url_error_message(error: OpenUrlError) -> String {
    match error {
        OpenUrlError::Invalidated => localization::dev_url_error_invalidated(),
        OpenUrlError::StaleHost => localization::dev_url_error_stale_host(),
        OpenUrlError::SessionUnavailable => localization::dev_url_error_session_unavailable(),
        OpenUrlError::BrowserUnavailable => localization::dev_url_error_browser_unavailable(),
        OpenUrlError::PermissionDenied => localization::dev_url_error_permission(),
        OpenUrlError::DispatchFailed => localization::dev_url_error_dispatch(),
    }
}

fn dev_url_display(
    candidate: &DevUrlCandidate,
    recording_friendly: bool,
) -> (String, Option<String>, bool) {
    if recording_friendly {
        return (localization::dev_url_chip_masked(), None, false);
    }
    (
        localization::dev_url_chip(candidate.display_origin.clone()),
        Some(localization::dev_url_path(
            candidate.normalized_url.path_label().to_string(),
        )),
        candidate.has_hidden_query,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct RecordingPlatform {
        calls: Arc<Mutex<Vec<String>>>,
        result: Result<(), OpenUrlError>,
    }

    impl PlatformOpenUrl for RecordingPlatform {
        fn open(&mut self, url: &LocalDevUrl) -> Result<(), OpenUrlError> {
            self.calls.lock().unwrap().push(url.as_str().to_string());
            self.result
        }
    }

    #[test]
    fn platform_dispatch_is_exact_and_never_retried() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut state = DevUrlUiState {
            platform: Box::new(RecordingPlatform {
                calls: Arc::clone(&calls),
                result: Err(OpenUrlError::DispatchFailed),
            }),
            pending: None,
            last_error: None,
            confirmation_focus: None,
        };
        let url = LocalDevUrl::parse("http://localhost:4317/private?secret=canary").unwrap();
        assert_eq!(state.dispatch(&url), Err(OpenUrlError::DispatchFailed));
        assert_eq!(calls.lock().unwrap().as_slice(), [url.as_str()]);
        assert!(state.last_error.is_none());
    }

    #[test]
    fn user_facing_errors_never_include_url_material() {
        for error in [
            OpenUrlError::Invalidated,
            OpenUrlError::StaleHost,
            OpenUrlError::SessionUnavailable,
            OpenUrlError::BrowserUnavailable,
            OpenUrlError::PermissionDenied,
            OpenUrlError::DispatchFailed,
        ] {
            let message = dev_url_error_message(error);
            assert!(!message.contains("localhost"));
            assert!(!message.contains("canary"));
        }
    }

    #[test]
    fn recording_friendly_projection_masks_every_url_derived_label() {
        let url = LocalDevUrl::parse("http://localhost:4317/private?secret=canary").unwrap();
        let candidate = DevUrlCandidate {
            id: 1,
            session_id: HostedSessionId::new(),
            host_instance: HostInstanceId::new(),
            output_sequence: termirust_domain::OutputSequence::new(3),
            display_origin: url.display_origin().to_string(),
            has_hidden_query: url.has_hidden_query(),
            normalized_url: url,
        };
        let (label, path, hidden_parameters) = dev_url_display(&candidate, true);
        assert_eq!(label, localization::dev_url_chip_masked());
        assert!(path.is_none());
        assert!(!hidden_parameters);
        assert!(!label.contains("localhost"));
        assert!(!label.contains("private"));
        assert!(!label.contains("canary"));
    }
}
