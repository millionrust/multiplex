//! The rail beside a workspace: saved hosts to go to or open, and new local
//! terminals and agents, each of which can also be dragged onto a split pane or
//! the canvas.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, DragMoveEvent, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled, Window,
    div, px,
};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Icon, IconName, StyledExt as _, h_flex, v_flex};
use multiplex_ui_contract::MessageId;

use super::motion::MotionSpeed;
use super::split_tree::SplitEdge;
use super::{MAX_SPLIT_PANES, MultiplexApp, SplitNode};
use crate::models::{
    AgentProvider, ConnectRequest, ConnectionKind, HostProfile, WorkspaceLayoutMode,
};
use crate::ui::app::types::DropZone;
use crate::ui::localization;
use crate::ui::theme;

/// Something the rail can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RailItem {
    Host(String),
    LocalTerminal,
    ClaudeAgent,
}

/// A rail item being dragged onto a pane or the canvas.
#[derive(Clone)]
pub(super) struct RailDrag {
    pub(super) item: RailItem,
    label: String,
}

pub(super) struct RailDragPreview {
    label: String,
}

impl gpui::Render for RailDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("rail-drag-preview")
            .px(px(theme::SPACE_3))
            .py(px(theme::SPACE_2))
            .rounded(px(theme::SPACE_2))
            .bg(theme::terminal_panel())
            .border_1()
            .border_color(theme::accent())
            .shadow_lg()
            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
            .font_semibold()
            .text_color(theme::text_on_dark())
            .child(self.label.clone())
    }
}

impl MultiplexApp {
    /// How far the workspace body is pushed right by the rail, as last drawn.
    pub(super) fn workspace_rail_width(&self) -> f32 {
        self.host_rail_width.get()
    }

    /// The width the rail takes in a window `window_width` wide.
    fn rail_width_for(&self, window_width: f32) -> f32 {
        if self.active_workspace().is_none() || window_width < theme::WORKSPACE_HOST_RAIL_MIN_WINDOW
        {
            0.0
        } else if self.host_rail_open {
            theme::WORKSPACE_HOST_RAIL_WIDTH
        } else {
            theme::WORKSPACE_HOST_RAIL_COLLAPSED_WIDTH
        }
    }

    /// The pane of the active workspace connected to `profile`, if one is.
    pub(super) fn open_pane_for_profile(&self, profile: &HostProfile) -> Option<u64> {
        let workspace = self.active_workspace()?;
        workspace.pane_ids.iter().copied().find(|pane_id| {
            self.pane(*pane_id).is_some_and(|pane| {
                !pane.closed
                    && pane.request.kind == ConnectionKind::Ssh
                    && pane.request.host.eq_ignore_ascii_case(&profile.host)
                    && pane.request.port == profile.port
                    && pane.request.username == profile.username
            })
        })
    }

    fn rail_request(&self, item: &RailItem) -> anyhow::Result<Option<ConnectRequest>> {
        match item {
            RailItem::Host(profile_id) => {
                let profile = self
                    .saved
                    .profiles
                    .iter()
                    .find(|profile| &profile.id == profile_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!(localization::static_message(
                            MessageId::AgentCanvasCopyThatSavedHostNoLongerExists
                        ))
                    })?;
                self.connect_request_for_saved_canvas_host(profile)
                    .map(Some)
            }
            RailItem::LocalTerminal => {
                let mut config = self.saved.settings.default_local_shell.clone();
                if let Some(directory) = self
                    .active_workspace()
                    .and_then(|workspace| workspace.project_directory.clone())
                {
                    config.cwd = Some(directory);
                }
                Ok(Some(ConnectRequest::local_shell_with_config(0, config)))
            }
            RailItem::ClaudeAgent => Ok(None),
        }
    }

    /// A click on a rail item: go to a host that is already open, or open the
    /// item next to the focused pane.
    pub(super) fn activate_rail_item(
        &mut self,
        item: RailItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let RailItem::Host(profile_id) = &item
            && let Some(pane_id) = self
                .saved
                .profiles
                .iter()
                .find(|profile| &profile.id == profile_id)
                .cloned()
                .and_then(|profile| self.open_pane_for_profile(&profile))
        {
            self.go_to_pane(pane_id, window, cx);
            return;
        }
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        if workspace.layout_mode == WorkspaceLayoutMode::Canvas {
            self.canvas_add_anchor = None;
            self.open_rail_item_on_canvas(item, window, cx);
            return;
        }
        let target = workspace.active_pane_id;
        self.open_rail_item_in_split(item, target, SplitEdge::Right, window, cx);
    }

    /// Show `pane_id`: focus it in the split, or fly to its node on the canvas.
    fn go_to_pane(&mut self, pane_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let workspace_id = workspace.id;
        let in_split = workspace.layout_mode == WorkspaceLayoutMode::Split
            && workspace
                .layout
                .as_ref()
                .is_some_and(|layout| layout.contains(pane_id));
        if in_split {
            if self.zoomed_pane().is_some_and(|zoomed| zoomed != pane_id) {
                self.begin_layout_transition(MotionSpeed::Quick);
                self.zoomed_panes.insert(workspace_id, pane_id);
                self.sync_terminal_layout(window, cx);
            }
            self.activate_pane(pane_id, window, cx);
            if let Some(pane) = self.pane(pane_id) {
                pane.terminal_focus.focus(window);
            }
            cx.notify();
            return;
        }
        if workspace.layout_mode == WorkspaceLayoutMode::Split {
            self.set_workspace_layout_mode(WorkspaceLayoutMode::Canvas, window, cx);
        }
        let node_id = self.active_workspace().and_then(|workspace| {
            workspace
                .canvas
                .nodes
                .iter()
                .find(|node| node.kind.pane_id() == Some(pane_id))
                .map(|node| node.id.clone())
        });
        if let Some(node_id) = node_id {
            self.focus_canvas_activity_node(node_id.clone(), window, cx);
            self.fly_to_canvas_node(&node_id, window, cx);
        }
    }

    /// Open a rail item as a new pane on `edge` of `target` in the active split.
    pub(super) fn open_rail_item_in_split(
        &mut self,
        item: RailItem,
        target: u64,
        edge: SplitEdge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = match self.rail_request(&item) {
            Ok(Some(request)) => request,
            Ok(None) => {
                // Agents live on the canvas.
                self.set_workspace_layout_mode(WorkspaceLayoutMode::Canvas, window, cx);
                self.open_agent_creation(AgentProvider::ClaudeCode, window, cx);
                return;
            }
            Err(error) => {
                self.error_message = error.to_string();
                cx.notify();
                return;
            }
        };
        self.open_request_in_split(request, target, edge, window, cx);
    }

    /// Start `request` as a new pane beside `target`, gliding the others aside.
    pub(super) fn open_request_in_split(
        &mut self,
        mut request: ConnectRequest,
        target: u64,
        edge: SplitEdge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = self.workspace_id_for_pane(target) else {
            return;
        };
        let full = self
            .workspace(workspace_id)
            .and_then(|workspace| workspace.layout.as_ref())
            .is_some_and(|layout| layout.leaf_count() >= MAX_SPLIT_PANES);
        if full {
            self.error_message = localization::workspace_split_cap_error(MAX_SPLIT_PANES);
            cx.notify();
            return;
        }
        request.session_id = self.next_session_id();
        let title = request.title.clone();
        let new_pane_id = self.spawn_pane(request, window, cx);
        if self.active_workspace_id == Some(workspace_id) {
            self.begin_layout_transition(MotionSpeed::Quick);
        }
        self.zoomed_panes.remove(&workspace_id);
        let (axis, new_first) = edge.axis_and_order();
        if let Some(workspace) = self.workspace_mut(workspace_id) {
            let inserted = workspace.layout.as_mut().is_some_and(|layout| {
                layout.split_leaf(target, &SplitNode::Leaf(new_pane_id), axis, new_first)
            });
            if !inserted {
                workspace.layout = Some(SplitNode::Leaf(new_pane_id));
            }
            workspace.sync_pane_ids();
            workspace.active_pane_id = new_pane_id;
            if workspace.title.trim().is_empty() {
                workspace.title = title;
            }
        }
        self.error_message.clear();
        self.sync_terminal_layout(window, cx);
        if let Some(pane) = self.pane(new_pane_id) {
            pane.terminal_focus.focus(window);
        }
        self.persist_runtime_state();
        cx.notify();
    }

    /// Place a rail item on the canvas: where it was dropped or double-clicked
    /// when that is known, otherwise in the middle of the view.
    pub(super) fn open_rail_item_on_canvas(
        &mut self,
        item: RailItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            RailItem::Host(profile_id) => self.add_saved_host_to_canvas(&profile_id, window, cx),
            RailItem::LocalTerminal => self.add_local_terminal_to_canvas(window, cx),
            RailItem::ClaudeAgent => {
                self.open_agent_creation(AgentProvider::ClaudeCode, window, cx)
            }
        }
    }

    /// Point the split's drop preview at the edge of `pane_id` under a dragged
    /// rail item, warning when the split has no room.
    pub(super) fn update_rail_drop_target(
        &mut self,
        pane_id: u64,
        event: &DragMoveEvent<RailDrag>,
        cx: &mut Context<Self>,
    ) {
        let zone = event
            .bounds
            .contains(&event.event.position)
            .then(|| super::workspace::drop_zone_at(event.bounds, event.event.position, false));
        let full = zone.is_some()
            && self
                .workspace_id_for_pane(pane_id)
                .and_then(|workspace_id| self.workspace(workspace_id))
                .and_then(|workspace| workspace.layout.as_ref())
                .is_some_and(|layout| layout.leaf_count() >= MAX_SPLIT_PANES);
        self.set_split_drop_target(pane_id, zone, full, cx);
    }

    pub(super) fn drop_rail_item_on_pane(
        &mut self,
        item: RailItem,
        target: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let zone = self
            .split_drop_target
            .and_then(|(pane_id, zone)| (pane_id == target).then_some(zone));
        let full = self.split_drop_full;
        self.split_drop_target = None;
        self.split_drop_full = false;
        self.split_drop_preview_from = None;
        if full {
            self.error_message = localization::workspace_split_cap_error(MAX_SPLIT_PANES);
            cx.notify();
            return;
        }
        let edge = match zone {
            Some(DropZone::Left) => SplitEdge::Left,
            Some(DropZone::Top) => SplitEdge::Top,
            Some(DropZone::Bottom) => SplitEdge::Bottom,
            Some(DropZone::Right | DropZone::Center) | None => SplitEdge::Right,
        };
        self.open_rail_item_in_split(item, target, edge, window, cx);
    }

    /// A rail item let go over the canvas lands where it was dropped.
    pub(super) fn drop_rail_item_on_canvas(
        &mut self,
        item: RailItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position = window.mouse_position();
        let screen = self.canvas_local_point(position);
        self.canvas_add_anchor = self
            .active_workspace()
            .map(|workspace| (screen, workspace.canvas.transform.screen_to_world(screen)));
        self.open_rail_item_on_canvas(item, window, cx);
    }

    fn toggle_host_rail(&mut self, cx: &mut Context<Self>) {
        self.host_rail_open = !self.host_rail_open;
        cx.notify();
    }

    /// The rail, or nothing when the window is too narrow for it. Records the
    /// width it takes so the workspace body can lay itself out beside it.
    pub(super) fn render_host_rail(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let width = self.rail_width_for(f32::from(window.viewport_size().width));
        self.host_rail_width.set(width);
        if width == 0.0 {
            return None;
        }
        let toggle = div()
            .id("host-rail-toggle")
            .debug_selector(|| "host-rail-toggle".to_string())
            .size(px(theme::SPLIT_PANE_HEADER_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::SPACE_1))
            .text_color(theme::text_muted_dark())
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(theme::with_alpha(theme::hover(), 0.5))
                    .text_color(theme::text_on_dark())
            })
            .tooltip({
                let open = self.host_rail_open;
                move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(localization::static_message(if open {
                        MessageId::HostRailCollapse
                    } else {
                        MessageId::HostRailExpand
                    }))
                    .build(window, cx)
                }
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_host_rail(cx)))
            .child(
                Icon::new(if self.host_rail_open {
                    IconName::PanelLeftClose
                } else {
                    IconName::PanelLeftOpen
                })
                .size(px(theme::ICON_SIZE_DEFAULT)),
            );
        let frame = v_flex()
            .id("host-rail")
            .debug_selector(|| "host-rail".to_string())
            .flex_none()
            .w(px(width))
            .h_full()
            .bg(theme::terminal_panel())
            .border_r_1()
            .border_color(theme::border_dark());
        if !self.host_rail_open {
            return Some(
                frame
                    .items_center()
                    .pt(px(theme::SPACE_3))
                    .child(toggle)
                    .into_any_element(),
            );
        }

        let mut profiles: Vec<&HostProfile> = self.saved.profiles.iter().collect();
        profiles.sort_by(|a, b| {
            b.favorite.cmp(&a.favorite).then_with(|| {
                a.display_name()
                    .to_lowercase()
                    .cmp(&b.display_name().to_lowercase())
            })
        });
        let mut hosts = v_flex().gap(px(theme::BORDER_HAIRLINE));
        if profiles.is_empty() {
            hosts = hosts.child(
                div()
                    .px(px(theme::SPACE_3))
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted_dark())
                    .child(localization::static_message(MessageId::HostRailEmpty)),
            );
        }
        for profile in profiles {
            let name = profile.display_name();
            let dot = match profile.color_tag {
                Some(tag) => gpui::rgb(tag.rgb_hex()).into(),
                None => theme::host_chip_color(&name),
            };
            let group = if profile.group.trim().is_empty() {
                profile.tags.first().cloned().unwrap_or_default()
            } else {
                profile.group.trim().to_string()
            };
            let open = self.open_pane_for_profile(profile).is_some();
            hosts = hosts.child(self.render_rail_row(
                RailItem::Host(profile.id.clone()),
                SharedString::from(format!("host-rail-host-{}", profile.id)),
                name,
                Some(dot),
                None,
                Some(group),
                open,
                cx,
            ));
        }
        let new_items = v_flex()
            .gap(px(theme::BORDER_HAIRLINE))
            .child(self.render_rail_row(
                RailItem::LocalTerminal,
                "host-rail-local".into(),
                localization::static_message(MessageId::HostRailLocalTerminal),
                None,
                Some(IconName::SquareTerminal),
                None,
                false,
                cx,
            ))
            .child(self.render_rail_row(
                RailItem::ClaudeAgent,
                "host-rail-agent".into(),
                localization::static_message(MessageId::HostRailClaudeAgent),
                None,
                Some(IconName::Bot),
                None,
                false,
                cx,
            ));
        let heading = |message| {
            div()
                .px(px(theme::SPACE_3))
                .pt(px(theme::SPACE_4))
                .pb(px(theme::SPACE_2))
                .text_size(px(theme::TYPE_NANO_SIZE))
                .font_semibold()
                .text_color(theme::text_muted_dark())
                .child(localization::static_message(message).to_uppercase())
        };
        Some(
            frame
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .pr(px(theme::SPACE_2))
                        .child(heading(MessageId::HostRailHostsHeading))
                        .child(toggle),
                )
                .child(
                    v_flex()
                        .id("host-rail-scroll")
                        .flex_1()
                        .min_h_0()
                        .px(px(theme::SPACE_2))
                        .overflow_y_scrollbar()
                        .child(hosts)
                        .child(heading(MessageId::HostRailNewHeading))
                        .child(new_items)
                        .child(
                            div()
                                .px(px(theme::SPACE_3))
                                .pt(px(theme::SPACE_5))
                                .pb(px(theme::SPACE_4))
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted_dark())
                                .child(localization::static_message(MessageId::HostRailTip)),
                        ),
                )
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_rail_row(
        &self,
        item: RailItem,
        id: SharedString,
        label: String,
        dot: Option<gpui::Hsla>,
        icon: Option<IconName>,
        detail: Option<String>,
        open: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let drag = RailDrag {
            item: item.clone(),
            label: label.clone(),
        };
        let is_host = matches!(item, RailItem::Host(_));
        let selector = id.to_string();
        h_flex()
            .id(id)
            .debug_selector(move || selector.clone())
            .h(px(theme::WORKSPACE_HOST_RAIL_ROW_HEIGHT))
            .px(px(theme::SPACE_3))
            .gap(px(theme::SPACE_3))
            .items_center()
            .rounded(px(theme::SPACE_2))
            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
            .text_color(theme::text_secondary())
            .cursor_grab()
            .hover(|style| {
                style
                    .bg(theme::with_alpha(theme::hover(), 0.5))
                    .text_color(theme::text_on_dark())
            })
            .when(is_host, |row| {
                row.tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(localization::static_message(if open {
                        MessageId::HostRailOpenTooltip
                    } else {
                        MessageId::HostRailNewTooltip
                    }))
                    .build(window, cx)
                })
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_rail_item(item.clone(), window, cx);
            }))
            .on_drag(drag, |drag: &RailDrag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| RailDragPreview {
                    label: drag.label.clone(),
                })
            })
            .when_some(dot, |row, color| {
                row.child(
                    div()
                        .flex_none()
                        .size(px(theme::WORKSPACE_HOST_RAIL_DOT))
                        .rounded_full()
                        .bg(color),
                )
            })
            .when_some(icon, |row, icon| {
                row.child(Icon::new(icon).size(px(theme::ICON_SIZE_DEFAULT)))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(label),
            )
            .when(open, |row| {
                row.child(
                    div()
                        .flex_none()
                        .size(px(theme::WORKSPACE_HOST_RAIL_DOT))
                        .rounded_full()
                        .bg(theme::success()),
                )
            })
            .when_some(detail.filter(|detail| !detail.is_empty()), |row, detail| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(theme::TYPE_NANO_SIZE))
                        .font_family(self.terminal_font_family(cx))
                        .text_color(theme::text_muted_dark())
                        .child(detail),
                )
            })
    }
}
