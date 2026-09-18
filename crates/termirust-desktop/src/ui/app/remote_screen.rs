//! A workspace tab showing another computer's screen.
//!
//! The tab owns the session; closing the tab closes the connection. What it draws is the picture
//! the session last published, fitted to the tab, magnified and dragged by hand, with a minimap
//! once the picture is bigger than the tab and an inspector saying what the connection is doing.

use gpui::{ObjectFit, StyledImage as _, img};

use crate::controller::watch_session::{WatchInput, WatchSession, WatchState};

use super::*;

/// As far in as this Mac will magnify another computer's screen. Waiting on the gesture that
/// magnifies, with the geometry below.
#[allow(dead_code)]
pub const MAXIMUM_ZOOM: f32 = 6.0;

/// One watched computer, in a workspace tab.
pub(super) struct WorkspaceScreenState {
    pub session: WatchSession,
    pub title: String,
    pub geometry: ScreenGeometry,
}

impl std::fmt::Debug for WorkspaceScreenState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceScreenState")
            .field("title", &self.title)
            .field("geometry", &self.geometry)
            .finish()
    }
}

impl WorkspaceScreenState {
    pub fn new(session: WatchSession, title: String) -> Self {
        Self {
            session,
            title,
            geometry: ScreenGeometry::default(),
        }
    }

    /// The computer's screen, in its own pixels. What the geometry above measures against, so it
    /// arrives with the gesture that needs it.
    #[allow(dead_code)]
    pub fn size(&self) -> (u32, u32) {
        self.session.size()
    }
}

/// Where the picture sits in the tab: fitted, magnified, dragged.
///
/// Kept apart from the session so the arithmetic a person's hand depends on can be checked
/// without another computer to connect to.
#[derive(Clone, Copy, Debug)]
pub(super) struct ScreenGeometry {
    /// 1 fits the whole picture; above that it is magnified and can be dragged.
    pub zoom: f32,
    pub pan: (f32, f32),
}

impl Default for ScreenGeometry {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: (0.0, 0.0),
        }
    }
}

// Magnifying and dragging are worked out and covered by this module's tests, but no pointer
// gesture reaches them yet: the tab draws the fitted picture. The arithmetic is kept here, tested,
// rather than written again when the gesture lands.
#[allow(dead_code)]
impl ScreenGeometry {
    /// How many tab pixels one of the computer's pixels takes.
    pub fn scale(&self, size: (u32, u32), view: (f32, f32)) -> f32 {
        let (width, height) = size;
        if width == 0 || height == 0 || view.0 <= 0.0 || view.1 <= 0.0 {
            return 0.0;
        }
        (view.0 / width as f32).min(view.1 / height as f32) * self.zoom
    }

    /// What the zoom reads in the inspector.
    pub fn zoom_label(&self) -> String {
        if self.zoom <= 1.01 {
            "Fit".to_owned()
        } else {
            format!("{}%", (self.zoom * 100.0).round() as i32)
        }
    }

    /// The part of the computer's screen on show, in its own pixels, for the minimap.
    pub fn visible(&self, size: (u32, u32), view: (f32, f32)) -> (f32, f32, f32, f32) {
        let scale = self.scale(size, view);
        let (width, height) = size;
        if scale <= 0.0 {
            return (0.0, 0.0, 0.0, 0.0);
        }
        let drawn = (width as f32 * scale, height as f32 * scale);
        let origin = (
            (view.0 - drawn.0) / 2.0 + self.pan.0,
            (view.1 - drawn.1) / 2.0 + self.pan.1,
        );
        let left = (-origin.0 / scale).max(0.0);
        let top = (-origin.1 / scale).max(0.0);
        let right = (left + view.0 / scale).min(width as f32);
        let bottom = (top + view.1 / scale).min(height as f32);
        (left, top, (right - left).max(0.0), (bottom - top).max(0.0))
    }

    /// Magnifies, keeping the middle of the tab on the same part of the picture.
    pub fn set_zoom(&mut self, factor: f32, size: (u32, u32), view: (f32, f32)) {
        let clamped = factor.clamp(1.0, MAXIMUM_ZOOM);
        if (clamped - self.zoom).abs() < f32::EPSILON {
            return;
        }
        let before = self.scale(size, view);
        self.zoom = clamped;
        let after = self.scale(size, view);
        if before > 0.0 && after > 0.0 {
            self.pan = (self.pan.0 * after / before, self.pan.1 * after / before);
        }
        self.clamp(size, view);
    }

    pub fn pan_by(&mut self, delta: (f32, f32), size: (u32, u32), view: (f32, f32)) {
        if self.zoom <= 1.0 {
            return;
        }
        self.pan = (self.pan.0 + delta.0, self.pan.1 + delta.1);
        self.clamp(size, view);
    }

    pub fn fit(&mut self) {
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
    }

    /// Keeps the picture from being dragged off the tab.
    fn clamp(&mut self, size: (u32, u32), view: (f32, f32)) {
        let scale = self.scale(size, view);
        let (width, height) = size;
        if scale <= 0.0 {
            return;
        }
        let slack = (
            ((width as f32 * scale - view.0) / 2.0).max(0.0),
            ((height as f32 * scale - view.1) / 2.0).max(0.0),
        );
        self.pan = (
            self.pan.0.clamp(-slack.0, slack.0),
            self.pan.1.clamp(-slack.1, slack.1),
        );
    }
}

impl TermiRustApp {
    /// Opens a workspace tab watching `address`, at the computer's full detail.
    ///
    /// The tab owns the session, so closing the tab closes the connection: watching somebody
    /// else's screen should never outlive the window showing it.
    pub(super) fn open_remote_screen(
        &mut self,
        address: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((computer, store)) = self.remote_devices.watched_computer(address) else {
            return;
        };
        let Some(session) = WatchSession::start(&computer, &store, false) else {
            self.error_message = localization::watched_computers_watch_failed();
            cx.notify();
            return;
        };
        let title = computer.display_name.clone();
        let workspace_id = self.next_workspace_id();
        self.workspaces.push(WorkspaceTab {
            id: workspace_id,
            title: title.clone(),
            project_directory: None,
            pane_ids: Vec::new(),
            active_pane_id: 0,
            unread_events: 0,
            layout: None,
            layout_mode: WorkspaceLayoutMode::Split,
            canvas: CanvasWorkspaceState::default(),
            view_mode: WorkspaceViewMode::Screen,
            sftp: None,
            screen: Some(WorkspaceScreenState::new(session, title)),
            search_visible: false,
            search_query: String::new(),
            search_results: Vec::new(),
            active_search_index: None,
            broadcast_input: false,
            pending_connect: None,
            pending_connect_mode: ConnectDialogMode::Username,
            pending_connect_protocol: ConnectProtocol::Ssh,
            connect_failure: None,
        });
        self.active_workspace_id = Some(workspace_id);
        self.show_editor_panel = false;
        self.error_message.clear();
        let _ = window;
        cx.notify();
    }

    /// The tab's picture, its minimap, and the inspector beside it.
    pub(super) fn render_workspace_screen_view(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(screen) = self
            .active_workspace()
            .and_then(|workspace| workspace.screen.as_ref())
        else {
            return v_flex().flex_1().bg(theme::terminal_bg());
        };
        let picture = screen.session.picture();
        let state = screen.session.state();
        let (width, height) = screen.session.size();
        v_flex().flex_1().min_w_0().min_h_0().child(
            h_flex()
                .id("remote-screen")
                .debug_selector(|| "remote-screen".to_string())
                .flex_1()
                .min_w_0()
                .min_h_0()
                .bg(theme::terminal_bg())
                .child(
                    div()
                        .id("remote-screen-stage")
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when_some(picture, |this, picture| {
                            this.child(
                                img(picture)
                                    .object_fit(ObjectFit::Contain)
                                    .w_full()
                                    .h_full(),
                            )
                        }),
                )
                .child(self.render_remote_screen_inspector(screen, &state, width, height, cx)),
        )
    }

    /// What the connection is doing, in words rather than a graph: Stage A has no round-trip
    /// time or loss to draw.
    fn render_remote_screen_inspector(
        &self,
        screen: &WorkspaceScreenState,
        state: &WatchState,
        width: u32,
        height: u32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let displays = screen.session.displays();
        let control = screen.session.control();
        v_flex()
            .id("remote-screen-inspector")
            .debug_selector(|| "remote-screen-inspector".to_string())
            .flex_none()
            .w(px(theme::SCREEN_PANEL_WIDTH))
            .h_full()
            .gap_2()
            .p_3()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::library_bg())
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(screen.title.clone()),
            )
            .child(inspector_row(
                localization::remote_screen_inspector_state(),
                describe(state),
            ))
            .child(inspector_row(
                localization::remote_screen_inspector_zoom(),
                screen.geometry.zoom_label(),
            ))
            .child(inspector_row(
                localization::remote_screen_inspector_size(),
                format!("{width} × {height}"),
            ))
            .child(inspector_row(
                localization::remote_screen_inspector_pictures(),
                screen.session.pictures().to_string(),
            ))
            .child(inspector_row(
                localization::remote_screen_inspector_displays(),
                displays.len().to_string(),
            ))
            .child(inspector_row(
                localization::remote_screen_inspector_control(),
                match control {
                    termirust_screen_protocol::ControlHolder::You => {
                        localization::remote_screen_control_this()
                    }
                    termirust_screen_protocol::ControlHolder::AnotherDevice => {
                        localization::remote_screen_control_another()
                    }
                    termirust_screen_protocol::ControlHolder::Nobody => {
                        localization::remote_screen_control_nobody()
                    }
                },
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        gpui_component::button::Button::new("remote-screen-fit")
                            .xsmall()
                            .ghost()
                            .label(localization::remote_screen_fit_action())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(screen) = this
                                    .active_workspace_mut()
                                    .and_then(|workspace| workspace.screen.as_mut())
                                {
                                    screen.geometry.fit();
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        gpui_component::button::Button::new("remote-screen-control")
                            .xsmall()
                            .ghost()
                            .label(
                                if control == termirust_screen_protocol::ControlHolder::You {
                                    localization::remote_screen_give_back_control()
                                } else {
                                    localization::remote_screen_take_control()
                                },
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(screen) = this
                                    .active_workspace()
                                    .and_then(|workspace| workspace.screen.as_ref())
                                {
                                    let holding = screen.session.control()
                                        == termirust_screen_protocol::ControlHolder::You;
                                    screen.session.send(if holding {
                                        WatchInput::ReleaseControl
                                    } else {
                                        WatchInput::RequestControl
                                    });
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_MICRO_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::remote_screen_stage_a_note()),
            )
            .into_any_element()
    }
}

fn describe(state: &WatchState) -> String {
    match state {
        WatchState::Connecting => localization::remote_screen_connecting(),
        WatchState::Watching => localization::remote_screen_watching(),
        WatchState::Ended(reason) => reason.clone(),
    }
}

fn inspector_row(label: String, value: String) -> Div {
    h_flex()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_size(px(theme::TYPE_MICRO_SIZE))
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .text_size(px(theme::TYPE_MICRO_SIZE))
                .text_color(theme::text_main())
                .child(value),
        )
}

#[cfg(test)]
mod tests {
    use super::{MAXIMUM_ZOOM, ScreenGeometry};

    /// The geometry a person's hand depends on, without needing another computer to connect to.
    fn geometry() -> ScreenGeometry {
        ScreenGeometry::default()
    }

    const COMPUTER: (u32, u32) = (1000, 500);
    const TAB: (f32, f32) = (500.0, 500.0);

    #[test]
    fn a_whole_screen_is_fitted_and_reads_as_fit() {
        let geometry = geometry();
        assert_eq!(geometry.scale(COMPUTER, TAB), 0.5);
        assert_eq!(geometry.zoom_label(), "Fit");
    }

    #[test]
    fn a_screen_with_no_size_yet_never_divides_by_zero() {
        assert_eq!(geometry().scale((0, 0), TAB), 0.0);
        assert_eq!(geometry().visible((0, 0), TAB), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn magnifying_stops_at_the_limits() {
        let mut geometry = geometry();
        geometry.set_zoom(0.1, COMPUTER, TAB);
        assert_eq!(geometry.zoom, 1.0, "never smaller than the whole screen");
        geometry.set_zoom(1000.0, COMPUTER, TAB);
        assert_eq!(geometry.zoom, MAXIMUM_ZOOM);
    }

    #[test]
    fn the_picture_cannot_be_dragged_off_the_tab() {
        let mut geometry = geometry();
        geometry.set_zoom(2.0, COMPUTER, TAB);
        geometry.pan_by((10_000.0, 10_000.0), COMPUTER, TAB);
        // At twice fit the picture is 1000x500 in a 500x500 tab: 250 of slack sideways, none
        // vertically beyond what fitting already gave.
        assert_eq!(geometry.pan, (250.0, 0.0));
    }

    #[test]
    fn dragging_does_nothing_while_the_whole_screen_is_shown() {
        let mut geometry = geometry();
        geometry.pan_by((100.0, 100.0), COMPUTER, TAB);
        assert_eq!(geometry.pan, (0.0, 0.0));
    }

    #[test]
    fn the_minimap_shows_which_part_of_the_computer_is_on_show() {
        let mut geometry = geometry();
        geometry.set_zoom(2.0, COMPUTER, TAB);
        let (left, _, width, height) = geometry.visible(COMPUTER, TAB);
        assert_eq!(left, 250.0, "the middle of the computer's screen");
        assert_eq!(width, 500.0);
        assert_eq!(height, 500.0);
        assert_eq!(geometry.zoom_label(), "200%");
    }

    #[test]
    fn the_whole_screen_is_visible_at_fit() {
        let (_, _, width, height) = geometry().visible(COMPUTER, TAB);
        assert_eq!((width, height), (1000.0, 500.0));
    }
}
