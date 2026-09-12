use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::StyledExt as _;
use gpui_component::scroll::ScrollableElement as _;
use termirust_ui_contract::{
    AccessibilityBridge, AccessibilityLabBridgeErrorArgs, AccessibilityLabCommand,
    AccessibilityLabConfiguration, AccessibilityLabModel, AccessibilityLabNode,
    AccessibilityLabProgressValueArgs, ColorValue, Count, FocusMove, FocusState,
    LiveRegionPoliteness, Locale, Localizer, MessageId, SemanticAction, SemanticActionRequest,
    SemanticActionValue, SemanticDiffer, SemanticErrorCode, SemanticNodeId, Text, ThemeKind,
};

use super::bridge::MacAccessibilityBridge;

const SYSTEM_TOKENS: termirust_ui_contract::DesignTokens =
    termirust_ui_contract::DesignTokens::new(ThemeKind::System);
const POLL_INTERVAL: Duration =
    Duration::from_millis(SYSTEM_TOKENS.motion_accessibility_poll(false).0 as u64);

pub fn run() {
    let configuration = configuration_from_environment();
    let title = Localizer::try_new(configuration.locale.tag())
        .unwrap_or_else(|_| Localizer::english())
        .format_static(MessageId::AccessibilityLabTitle)
        .unwrap_or_else(|_| "TermiRust".to_string());
    let application = Application::new().with_assets(crate::assets::Assets);
    application.run(move |cx| {
        gpui_component::init(cx);
        let bounds = Bounds::centered(
            None,
            size(
                px(SYSTEM_TOKENS.layout_accessibility_lab_window_width().0),
                px(SYSTEM_TOKENS.layout_accessibility_lab_window_height().0),
            ),
            cx,
        );
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.clone().into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_min_size: Some(size(
                    px(SYSTEM_TOKENS.layout_accessibility_lab_minimum_width().0),
                    px(SYSTEM_TOKENS.layout_accessibility_lab_minimum_height().0),
                )),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| AccessibilityHarness::new(window, cx)),
        )
        .expect("accessibility laboratory window should open");
        cx.activate(true);
    });
}

struct AccessibilityHarness {
    model: AccessibilityLabModel,
    localizer: Localizer,
    focus_handle: FocusHandle,
    differ: SemanticDiffer,
    bridge: Option<MacAccessibilityBridge>,
    actions: Receiver<SemanticActionRequest>,
    bridge_error: Option<SemanticErrorCode>,
}

impl AccessibilityHarness {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let configuration = configuration_from_environment();
        let localizer =
            Localizer::try_new(configuration.locale.tag()).unwrap_or_else(|_| Localizer::english());
        let model = AccessibilityLabModel::try_new(configuration)
            .expect("validated accessibility laboratory configuration");
        let (sender, actions) = MacAccessibilityBridge::bounded_action_channel();
        let resolver_localizer = localizer;
        let bridge_result = MacAccessibilityBridge::attach_to_key_window(
            sender,
            Arc::new(move |id| resolver_localizer.format_static(id).ok()),
        );
        let (bridge, bridge_error) = match bridge_result {
            Ok(bridge) => (Some(bridge), None),
            Err(error) => {
                eprintln!("[accessibility-lab] bridge unavailable: {:?}", error.code);
                (None, Some(error.code))
            }
        };
        let mut harness = Self {
            model,
            localizer,
            focus_handle: cx.focus_handle().tab_stop(true),
            differ: SemanticDiffer::default(),
            bridge,
            actions,
            bridge_error,
        };
        harness.sync_bridge();
        harness.focus_handle.focus(window);

        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if cx
                    .update(|_, cx| {
                        let _ = this.update(cx, |harness, cx| {
                            harness.process_native_actions(cx);
                        });
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        harness
    }

    fn text(&self, id: MessageId) -> String {
        self.localizer
            .format_static(id)
            .unwrap_or_else(|_| format!("[{}]", id.key()))
    }

    fn process_native_actions(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for _ in 0..64 {
            let Ok(request) = self.actions.try_recv() else {
                break;
            };
            changed |= self.apply_request(request);
        }
        if changed {
            self.sync_bridge();
            cx.notify();
        }
    }

    fn dispatch(
        &mut self,
        node: AccessibilityLabNode,
        action: SemanticAction,
        value: Option<SemanticActionValue>,
        cx: &mut Context<Self>,
    ) {
        let changed = self.apply_request(SemanticActionRequest {
            generation: self.model.generation(),
            node: node.semantic_id(),
            action,
            value,
        });
        if changed {
            self.sync_bridge();
            cx.notify();
        }
    }

    fn apply_request(&mut self, request: SemanticActionRequest) -> bool {
        let node = request.node;
        let Ok(command) = self.model.execute(request) else {
            return false;
        };
        if matches!(command, AccessibilityLabCommand::AnnounceStatus) {
            self.announce(
                AccessibilityLabNode::Status.semantic_id(),
                LiveRegionPoliteness::Polite,
            );
        } else if matches!(command, AccessibilityLabCommand::ConfirmReferenceAction) {
            self.announce(node, LiveRegionPoliteness::Immediate);
        }
        true
    }

    fn sync_bridge(&mut self) {
        let result = (|| {
            let tree = self.model.tree()?;
            let patch = self.differ.diff(tree)?;
            let bridge = self.bridge.as_mut().ok_or_else(|| {
                termirust_ui_contract::SemanticError::new(
                    SemanticErrorCode::BridgeUnavailable,
                    None,
                )
            })?;
            bridge.apply_patch(&patch)?;
            bridge.set_focus(self.model.generation(), current_focus(&self.model))
        })();
        self.bridge_error = result.err().map(|error| error.code);
        if let Some(error) = self.bridge_error {
            eprintln!("[accessibility-lab] bridge update failed: {error:?}");
        }
    }

    fn announce(&mut self, node: SemanticNodeId, politeness: LiveRegionPoliteness) {
        if let Some(bridge) = self.bridge.as_mut()
            && let Err(error) = bridge.announce(self.model.generation(), node, politeness)
        {
            self.bridge_error = Some(error.code);
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key == "tab" {
            let direction = if event.keystroke.modifiers.shift {
                FocusMove::Backward
            } else {
                FocusMove::Forward
            };
            if self.model.move_focus(direction).is_ok() {
                self.sync_bridge();
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        let focused = current_focus(&self.model);
        if key == "escape" && self.model.dialog_open() {
            self.dispatch(
                AccessibilityLabNode::Dialog,
                SemanticAction::Cancel,
                None,
                cx,
            );
            cx.stop_propagation();
            return;
        }
        if focused == Some(AccessibilityLabNode::Progress.semantic_id()) {
            let action = match key {
                "up" | "right" => Some(SemanticAction::Increment),
                "down" | "left" => Some(SemanticAction::Decrement),
                "escape" => Some(SemanticAction::Cancel),
                _ => None,
            };
            if let Some(action) = action {
                self.dispatch(AccessibilityLabNode::Progress, action, None, cx);
                cx.stop_propagation();
                return;
            }
        }
        if focused == Some(AccessibilityLabNode::Field.semantic_id()) {
            let mut value = self.model.field_value().to_string();
            if key == "backspace" {
                value.pop();
            } else if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.function
                && let Some(character) = event.keystroke.key_char.as_ref()
            {
                value.push_str(character);
            } else {
                return;
            }
            self.dispatch(
                AccessibilityLabNode::Field,
                SemanticAction::SetValue,
                SemanticActionValue::text(value).ok(),
                cx,
            );
            cx.stop_propagation();
            return;
        }
        if matches!(key, "enter" | "space")
            && let Some(node) = focused.and_then(node_for_id)
            && !matches!(
                node,
                AccessibilityLabNode::Progress | AccessibilityLabNode::Field
            )
        {
            self.dispatch(node, SemanticAction::Activate, None, cx);
            cx.stop_propagation();
        }
    }

    fn control(
        &self,
        id: &'static str,
        node: AccessibilityLabNode,
        label: String,
        tone: ControlTone,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let selected = current_focus(&self.model) == Some(node.semantic_id());
        div()
            .id(id)
            .h(px(self.scaled(42.)))
            .min_w(px(self.scaled(120.)))
            .px(px(self.scaled(14.)))
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(if selected {
                color(self.tokens().color_focus())
            } else {
                color(self.tokens().color_border_default())
            })
            .bg(match tone {
                ControlTone::Primary => color(self.tokens().color_action_primary()),
                ControlTone::Danger => color(self.tokens().color_status_error()),
                ControlTone::Neutral => color(self.tokens().color_bg_elevated()),
            })
            .text_color(color(match tone {
                ControlTone::Primary => self.tokens().color_action_primary_text(),
                ControlTone::Danger | ControlTone::Neutral => self.tokens().color_text_primary(),
            }))
            .cursor_pointer()
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.dispatch(node, SemanticAction::Activate, None, cx);
            }))
    }

    fn tokens(&self) -> termirust_ui_contract::DesignTokens {
        termirust_ui_contract::DesignTokens::new(self.model.configuration().theme)
    }

    fn scaled(&self, value: f32) -> f32 {
        value * f32::from(self.model.configuration().text_scale_percent) / 100.0
    }
}

impl Focusable for AccessibilityHarness {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AccessibilityHarness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = self.tokens();
        let field_selected =
            current_focus(&self.model) == Some(AccessibilityLabNode::Field.semantic_id());
        let list_first_selected =
            current_focus(&self.model) == Some(AccessibilityLabNode::ListFirst.semantic_id());
        let list_second_selected =
            current_focus(&self.model) == Some(AccessibilityLabNode::ListSecond.semantic_id());
        let field_error = self.model.field_value().trim().is_empty();
        let scale = self.scaled(1.0);

        div()
            .id("accessibility-lab-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.handle_key(event, cx);
            }))
            .size_full()
            .overflow_y_scrollbar()
            .bg(color(tokens.color_bg_canvas()))
            .text_color(color(tokens.color_text_primary()))
            .font_family(tokens.font_ui_family().0)
            .child(
                div()
                    .w_full()
                    .max_w(px(tokens.layout_accessibility_lab_content_maximum().0))
                    .mx_auto()
                    .p(px(tokens.space_6().0 * scale))
                    .flex()
                    .flex_col()
                    .gap(px(tokens.type_heading().size * scale))
                    .child(
                        self.control(
                            "ax-skip",
                            AccessibilityLabNode::Skip,
                            self.text(MessageId::AccessibilityLabSkipAction),
                            ControlTone::Neutral,
                            cx,
                        )
                        .ml_auto(),
                    )
                    .child(
                        div()
                            .text_size(px(tokens.type_title().size * scale))
                            .font_semibold()
                            .child(self.text(MessageId::AccessibilityLabTitle)),
                    )
                    .child(
                        div()
                            .text_size(px(tokens.type_heading_small().size * scale))
                            .text_color(color(tokens.color_text_muted()))
                            .child(self.text(MessageId::AccessibilityLabDescription)),
                    )
                    .child(
                        div()
                            .w_full()
                            .py(px(
                                tokens.space_accessibility_lab_section_vertical().0 * scale
                            ))
                            .border_t_1()
                            .border_color(color(tokens.color_border_default()))
                            .flex()
                            .flex_col()
                            .gap(px(tokens.space_accessibility_lab_section_gap().0 * scale))
                            .child(self.text(MessageId::AccessibilityLabList))
                            .child(self.control(
                                "ax-list-first",
                                AccessibilityLabNode::ListFirst,
                                self.text(MessageId::AccessibilityLabListFirst),
                                if list_first_selected {
                                    ControlTone::Primary
                                } else {
                                    ControlTone::Neutral
                                },
                                cx,
                            ))
                            .child(self.control(
                                "ax-list-second",
                                AccessibilityLabNode::ListSecond,
                                self.text(MessageId::AccessibilityLabListSecond),
                                if list_second_selected {
                                    ControlTone::Primary
                                } else {
                                    ControlTone::Neutral
                                },
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .w_full()
                            .py(px(
                                tokens.space_accessibility_lab_section_vertical().0 * scale
                            ))
                            .border_t_1()
                            .border_color(color(tokens.color_border_default()))
                            .flex()
                            .flex_col()
                            .gap(px(tokens.space_3().0 * scale))
                            .child(self.text(MessageId::AccessibilityLabField))
                            .child(
                                div()
                                    .id("ax-field")
                                    .h(px(tokens.control_height_large().0 * scale))
                                    .px(px(tokens.space_4().0 * scale))
                                    .flex()
                                    .items_center()
                                    .border_1()
                                    .border_color(if field_selected {
                                        color(tokens.color_focus())
                                    } else if field_error {
                                        color(tokens.color_status_error())
                                    } else {
                                        color(tokens.color_border_default())
                                    })
                                    .bg(color(tokens.color_bg_surface()))
                                    .child(self.model.field_value().to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dispatch(
                                            AccessibilityLabNode::Field,
                                            SemanticAction::Focus,
                                            None,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(tokens.type_body_small().size * scale))
                                    .text_color(color(tokens.color_text_muted()))
                                    .child(self.text(MessageId::AccessibilityLabFieldHelp)),
                            )
                            .when(field_error, |this| {
                                this.child(
                                    div()
                                        .text_size(px(tokens.type_body().size * scale))
                                        .text_color(color(tokens.color_status_error()))
                                        .child(self.text(MessageId::AccessibilityLabFieldError)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .py(px(
                                tokens.space_accessibility_lab_section_vertical().0 * scale
                            ))
                            .border_t_1()
                            .border_color(color(tokens.color_border_default()))
                            .flex()
                            .flex_wrap()
                            .gap(px(tokens.space_accessibility_lab_section_gap().0 * scale))
                            .child(self.control(
                                "ax-menu-item",
                                AccessibilityLabNode::MenuItem,
                                self.text(MessageId::AccessibilityLabMenuItem),
                                ControlTone::Neutral,
                                cx,
                            ))
                            .child(self.control(
                                "ax-destructive",
                                AccessibilityLabNode::Destructive,
                                self.text(MessageId::AccessibilityLabDestructiveAction),
                                ControlTone::Danger,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .w_full()
                            .py(px(
                                tokens.space_accessibility_lab_section_vertical().0 * scale
                            ))
                            .border_t_1()
                            .border_color(color(tokens.color_border_default()))
                            .flex()
                            .flex_col()
                            .gap(px(tokens.space_accessibility_lab_section_gap().0 * scale))
                            .child(
                                self.localizer
                                    .format(&AccessibilityLabProgressValueArgs::new(
                                        Text::new(self.text(MessageId::AccessibilityLabProgress)),
                                        Count(self.model.progress() as u64),
                                    )),
                            )
                            .child(
                                div()
                                    .h(px(tokens
                                        .layout_accessibility_lab_progress_track_height()
                                        .0
                                        * scale))
                                    .w_full()
                                    .bg(color(tokens.color_bg_elevated()))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(self.model.progress() as f32 / 100.0))
                                            .bg(color(tokens.color_action_primary())),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(tokens.space_3().0 * scale))
                                    .child(progress_control(
                                        "ax-progress-down",
                                        "-",
                                        scale,
                                        cx,
                                        |this, cx| {
                                            this.dispatch(
                                                AccessibilityLabNode::Progress,
                                                SemanticAction::Decrement,
                                                None,
                                                cx,
                                            );
                                        },
                                    ))
                                    .child(progress_control(
                                        "ax-progress-up",
                                        "+",
                                        scale,
                                        cx,
                                        |this, cx| {
                                            this.dispatch(
                                                AccessibilityLabNode::Progress,
                                                SemanticAction::Increment,
                                                None,
                                                cx,
                                            );
                                        },
                                    ))
                                    .child(progress_control(
                                        "ax-progress-cancel",
                                        self.text(MessageId::CommonCancel),
                                        scale,
                                        cx,
                                        |this, cx| {
                                            this.dispatch(
                                                AccessibilityLabNode::Progress,
                                                SemanticAction::Cancel,
                                                None,
                                                cx,
                                            );
                                        },
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .py(px(
                                tokens.space_accessibility_lab_status_vertical().0 * scale
                            ))
                            .border_t_1()
                            .border_color(color(tokens.color_border_default()))
                            .child(self.text(MessageId::AccessibilityLabStatusReady)),
                    )
                    .child(
                        div()
                            .text_color(color(tokens.color_text_muted()))
                            .child(self.text(MessageId::AccessibilityLabDisabledAction))
                            .child(" - ")
                            .child(self.text(MessageId::AccessibilityLabDisabledReason)),
                    )
                    .when_some(self.bridge_error, |this, error| {
                        this.child(div().text_color(color(tokens.color_status_error())).child(
                            self.localizer.format(&AccessibilityLabBridgeErrorArgs::new(
                                Text::new(format!("{error:?}")),
                            )),
                        ))
                    }),
            )
            .when(self.model.dialog_open(), |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(px(tokens.type_heading().size * scale))
                        .bg(color(tokens.color_overlay_scrim()))
                        .child(
                            div()
                                .w_full()
                                .max_w(px(
                                    tokens.layout_accessibility_lab_dialog_maximum().0 * scale
                                ))
                                .p(px(tokens.space_accessibility_lab_dialog_padding().0 * scale))
                                .bg(color(tokens.color_bg_elevated()))
                                .border_1()
                                .border_color(color(tokens.color_focus()))
                                .flex()
                                .flex_col()
                                .gap(px(tokens.space_accessibility_lab_dialog_gap().0 * scale))
                                .child(
                                    div()
                                        .text_size(px(tokens.type_activity_title().size * scale))
                                        .font_semibold()
                                        .child(
                                            self.text(
                                                MessageId::AccessibilityLabDestructiveConfirm,
                                            ),
                                        ),
                                )
                                .child(self.text(MessageId::AccessibilityLabDialogDescription))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .justify_end()
                                        .gap(px(
                                            tokens.space_accessibility_lab_section_gap().0 * scale
                                        ))
                                        .child(self.control(
                                            "ax-safe-default",
                                            AccessibilityLabNode::SafeDefault,
                                            self.text(MessageId::AccessibilityLabSafeDefault),
                                            ControlTone::Primary,
                                            cx,
                                        ))
                                        .child(self.control(
                                            "ax-confirm",
                                            AccessibilityLabNode::Confirm,
                                            self.text(MessageId::AccessibilityLabConfirmAction),
                                            ControlTone::Danger,
                                            cx,
                                        )),
                                ),
                        ),
                )
            })
    }
}

#[derive(Clone, Copy)]
enum ControlTone {
    Primary,
    Danger,
    Neutral,
}

fn progress_control(
    id: &'static str,
    label: impl Into<SharedString>,
    scale: f32,
    cx: &Context<AccessibilityHarness>,
    callback: impl Fn(&mut AccessibilityHarness, &mut Context<AccessibilityHarness>) + 'static,
) -> Stateful<Div> {
    let tokens = termirust_ui_contract::DesignTokens::new(ThemeKind::System);
    div()
        .id(id)
        .h(px(tokens
            .layout_accessibility_lab_progress_control_height()
            .0
            * scale))
        .min_w(px(tokens.target_touch_minimum().0 * scale))
        .px(px(tokens.space_4().0 * scale))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .cursor_pointer()
        .child(label.into())
        .on_click(cx.listener(move |this, _, _, cx| callback(this, cx)))
}

fn current_focus(model: &AccessibilityLabModel) -> Option<SemanticNodeId> {
    match model.focus_state() {
        FocusState::WindowInactive => None,
        FocusState::Focused(target) => Some(target.semantic_id()),
        FocusState::Modal { current, .. } => Some(current.semantic_id()),
    }
}

fn node_for_id(id: SemanticNodeId) -> Option<AccessibilityLabNode> {
    [
        AccessibilityLabNode::Skip,
        AccessibilityLabNode::ListFirst,
        AccessibilityLabNode::ListSecond,
        AccessibilityLabNode::Field,
        AccessibilityLabNode::MenuItem,
        AccessibilityLabNode::Progress,
        AccessibilityLabNode::Destructive,
        AccessibilityLabNode::SafeDefault,
        AccessibilityLabNode::Confirm,
    ]
    .into_iter()
    .find(|node| node.semantic_id() == id)
}

fn configuration_from_environment() -> AccessibilityLabConfiguration {
    let locale = std::env::var("TERMIRUST_AX_LOCALE")
        .ok()
        .and_then(|value| Locale::ALL.into_iter().find(|locale| locale.tag() == value))
        .unwrap_or(Locale::EnUs);
    let theme = match std::env::var("TERMIRUST_AX_THEME").as_deref() {
        Ok("light") => ThemeKind::Light,
        Ok("high-contrast") => ThemeKind::HighContrast,
        Ok("recording-friendly") => ThemeKind::RecordingFriendly,
        _ => ThemeKind::Dark,
    };
    let text_scale_percent = std::env::var("TERMIRUST_AX_SCALE")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| (100..=200).contains(value))
        .unwrap_or(100);
    let reduced_motion = std::env::var("TERMIRUST_AX_REDUCED_MOTION").as_deref() == Ok("1");
    AccessibilityLabConfiguration {
        locale,
        theme,
        text_scale_percent,
        reduced_motion,
    }
}

fn color(value: ColorValue) -> Hsla {
    Hsla {
        a: f32::from(value.alpha) / 255.0,
        ..rgb((u32::from(value.red) << 16) | (u32::from(value.green) << 8) | u32::from(value.blue))
            .into()
    }
}
