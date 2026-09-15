use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use termirust_ui_contract::{
    AccessibilityBridge, SemanticActionRequest, SemanticActionRouter, SemanticActionValue,
    SemanticDiffer, SemanticErrorCode, ShellAccessibilityCommand, ShellRegionId,
    ShellSemanticSnapshot, product_dialog_safe_semantic_node, shell_palette_input_semantic_node,
    shell_region_semantic_node,
};

#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::mpsc::Receiver;
#[cfg(target_os = "macos")]
use termirust_accessibility_macos::MacAccessibilityBridge;

pub struct ShellAccessibilityEvent {
    pub command: ShellAccessibilityCommand,
    pub value: Option<SemanticActionValue>,
}

pub struct ShellAccessibilityAdapter {
    differ: SemanticDiffer,
    router: Option<SemanticActionRouter<ShellAccessibilityCommand>>,
    last_palette_open: bool,
    last_product_dialog_open: bool,
    semantic_generation: u64,
    last_shape: Option<u64>,
    pub last_error: Option<SemanticErrorCode>,
    #[cfg(target_os = "macos")]
    bridge: Option<MacAccessibilityBridge>,
    #[cfg(target_os = "macos")]
    actions: Receiver<SemanticActionRequest>,
}

impl ShellAccessibilityAdapter {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        {
            let (sender, actions) = MacAccessibilityBridge::bounded_action_channel();
            let bridge = MacAccessibilityBridge::attach_to_key_window(
                sender,
                Arc::new(crate::ui::localization::message_id),
            )
            .ok();
            Self {
                differ: SemanticDiffer::default(),
                router: None,
                last_palette_open: false,
                last_product_dialog_open: false,
                semantic_generation: 1,
                last_shape: None,
                last_error: bridge
                    .as_ref()
                    .is_none()
                    .then_some(SemanticErrorCode::BridgeUnavailable),
                bridge,
                actions,
            }
        }
        #[cfg(not(target_os = "macos"))]
        Self {
            differ: SemanticDiffer::default(),
            router: None,
            last_palette_open: false,
            last_product_dialog_open: false,
            semantic_generation: 1,
            last_shape: None,
            last_error: None,
        }
    }

    pub fn sync(&mut self, mut snapshot: ShellSemanticSnapshot) {
        let shape = semantic_shape(&snapshot);
        if self.last_shape.is_some_and(|previous| previous != shape) {
            self.semantic_generation = self.semantic_generation.wrapping_add(1).max(1);
        }
        self.last_shape = Some(shape);
        snapshot.generation = self.semantic_generation;
        let product_dialog_open = snapshot
            .product_session
            .as_ref()
            .is_some_and(|product| product.dialog.is_some());
        let result = (|| {
            let tree = snapshot.try_tree()?;
            let router = snapshot.try_router(&tree)?;
            let patch = self.differ.diff(tree)?;
            #[cfg(target_os = "macos")]
            if let Some(bridge) = self.bridge.as_mut() {
                bridge.apply_patch(&patch)?;
                if self.last_palette_open != snapshot.palette_open {
                    let focus = if snapshot.palette_open {
                        shell_palette_input_semantic_node()
                    } else {
                        shell_region_semantic_node(ShellRegionId::Content)
                    };
                    bridge.set_focus(snapshot.generation.max(1), Some(focus))?;
                } else if !snapshot.palette_open
                    && self.last_product_dialog_open != product_dialog_open
                {
                    let focus = if product_dialog_open {
                        product_dialog_safe_semantic_node()
                    } else {
                        shell_region_semantic_node(ShellRegionId::Content)
                    };
                    bridge.set_focus(snapshot.generation.max(1), Some(focus))?;
                }
            }
            self.router = Some(router);
            self.last_palette_open = snapshot.palette_open;
            self.last_product_dialog_open = product_dialog_open;
            Ok::<_, termirust_ui_contract::SemanticError>(())
        })();
        self.last_error = result.err().map(|error| error.code);
    }

    pub fn drain(&mut self) -> Vec<ShellAccessibilityEvent> {
        #[cfg(target_os = "macos")]
        {
            let mut events = Vec::new();
            for _ in 0..64 {
                let Ok(request) = self.actions.try_recv() else {
                    break;
                };
                let value = request.value.clone();
                if let Some(router) = self.router.as_ref()
                    && let Ok(command) = router.resolve(request)
                {
                    events.push(ShellAccessibilityEvent {
                        command: *command,
                        value,
                    });
                }
            }
            events
        }
        #[cfg(not(target_os = "macos"))]
        Vec::new()
    }
}

fn semantic_shape(snapshot: &ShellSemanticSnapshot) -> u64 {
    let mut hasher = DefaultHasher::new();
    snapshot.inspector_visible.hash(&mut hasher);
    snapshot.palette_open.hash(&mut hasher);
    snapshot.palette_result_count.hash(&mut hasher);
    if let Some(product) = snapshot.product_session.as_ref() {
        product.screen.hash(&mut hasher);
        for row in &product.rows {
            row.id.hash(&mut hasher);
            row.parent.hash(&mut hasher);
            row.disabled.hash(&mut hasher);
        }
        for control in &product.controls {
            control.action.hash(&mut hasher);
            control.role.hash(&mut hasher);
            control.in_dialog.hash(&mut hasher);
            control.disabled.hash(&mut hasher);
        }
        product
            .dialog
            .map(|dialog| {
                (
                    dialog.kind,
                    dialog.target,
                    dialog.revision,
                    dialog.confirm_enabled,
                )
            })
            .hash(&mut hasher);
    }
    if let Some(preset_runtime) = snapshot.preset_runtime.as_ref() {
        preset_runtime.screen.hash(&mut hasher);
        preset_runtime.state.hash(&mut hasher);
        for row in &preset_runtime.rows {
            row.id.hash(&mut hasher);
            row.parent.hash(&mut hasher);
            row.disabled.hash(&mut hasher);
            row.risky.hash(&mut hasher);
            row.stale.hash(&mut hasher);
        }
        for control in &preset_runtime.controls {
            control.action.hash(&mut hasher);
            control.role.hash(&mut hasher);
            control.disabled.hash(&mut hasher);
            control.invalid.hash(&mut hasher);
        }
    }
    if let Some(terminal) = snapshot.terminal.as_ref() {
        terminal.terminal.session_id.hash(&mut hasher);
        terminal.input_authorized.hash(&mut hasher);
        terminal.recording_friendly.hash(&mut hasher);
        matches!(
            terminal.terminal.lifecycle,
            termirust_ui_contract::TerminalLifecycle::Gap
                | termirust_ui_contract::TerminalLifecycle::Offline
                | termirust_ui_contract::TerminalLifecycle::Backpressured
                | termirust_ui_contract::TerminalLifecycle::Error
                | termirust_ui_contract::TerminalLifecycle::PermissionDenied
        )
        .hash(&mut hasher);
        match terminal.focus_mode {
            termirust_ui_contract::TerminalFocusMode::Chrome => 0_u8,
            termirust_ui_contract::TerminalFocusMode::Input => 1,
            termirust_ui_contract::TerminalFocusMode::AccessibleReview => 2,
        }
        .hash(&mut hasher);
        terminal.announcement.is_some().hash(&mut hasher);
        terminal
            .announcement
            .is_some_and(|announcement| {
                matches!(
                    announcement,
                    termirust_ui_contract::TerminalAnnouncement::Attention
                        | termirust_ui_contract::TerminalAnnouncement::Gap
                )
            })
            .hash(&mut hasher);
        if terminal.focus_mode == termirust_ui_contract::TerminalFocusMode::AccessibleReview
            && !terminal.recording_friendly
        {
            termirust_ui_contract::terminal_semantic_chunk_count(&terminal.terminal.text)
                .hash(&mut hasher);
        }
    }
    hasher.finish()
}

impl Default for ShellAccessibilityAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_ui_contract::{
        AccessibleRowId, DestructiveActionKind, DestructiveActionPresentation, MessageId,
        PresetRuntimeAction, PresetRuntimeControl, PresetRuntimeControlRole, PresetRuntimeScreen,
        PresetRuntimeSemanticSnapshot, PresetRuntimeSurfaceState, ProductControlRole,
        ProductSessionAction, ProductSessionControl, ProductSessionScreen,
        ProductSessionSemanticSnapshot, ProductSessionSurfaceState, TerminalAccessibilityBuffer,
        TerminalAnnouncement, TerminalFocusMode, TerminalLifecycle, TerminalSemanticSnapshot,
    };

    fn product_snapshot(control_disabled: bool, confirm_enabled: bool) -> ShellSemanticSnapshot {
        ShellSemanticSnapshot {
            product_session: Some(ProductSessionSemanticSnapshot {
                screen: ProductSessionScreen::Projects,
                state: ProductSessionSurfaceState::Ready,
                rows: Vec::new(),
                controls: vec![ProductSessionControl {
                    action: ProductSessionAction::AddProject,
                    parent: None,
                    role: ProductControlRole::Button,
                    name: MessageId::ProjectsAddAction,
                    value: None,
                    selected: false,
                    disabled: control_disabled,
                    in_dialog: false,
                }],
                dialog: Some(DestructiveActionPresentation {
                    kind: DestructiveActionKind::RemoveProject,
                    target: AccessibleRowId::project(7),
                    revision: 3,
                    confirm_enabled,
                }),
                recording_friendly: false,
            }),
            ..ShellSemanticSnapshot::default()
        }
    }

    #[test]
    fn semantic_generation_shape_tracks_action_and_confirmation_availability() {
        let baseline = semantic_shape(&product_snapshot(false, false));
        assert_ne!(baseline, semantic_shape(&product_snapshot(true, false)));
        assert_ne!(baseline, semantic_shape(&product_snapshot(false, true)));
    }

    #[test]
    fn semantic_generation_shape_tracks_preset_runtime_action_availability() {
        let snapshot = |disabled| ShellSemanticSnapshot {
            preset_runtime: Some(PresetRuntimeSemanticSnapshot {
                screen: PresetRuntimeScreen::PresetsAndRuntimes,
                state: PresetRuntimeSurfaceState::Ready,
                rows: Vec::new(),
                controls: vec![PresetRuntimeControl {
                    action: PresetRuntimeAction::SavePreset,
                    parent: None,
                    role: PresetRuntimeControlRole::Button,
                    name: MessageId::PresetSaveAction,
                    value: None,
                    selected: false,
                    disabled,
                    invalid: disabled,
                }],
                recording_friendly: false,
            }),
            ..ShellSemanticSnapshot::default()
        };
        assert_ne!(
            semantic_shape(&snapshot(false)),
            semantic_shape(&snapshot(true))
        );
    }

    #[test]
    fn semantic_generation_shape_tracks_terminal_role_and_chunk_structure() {
        let snapshot = |text: &[u8], lifecycle, announcement| {
            let mut buffer = TerminalAccessibilityBuffer::new(7, "Terminal");
            buffer.append(text, Some(1));
            buffer.set_lifecycle(lifecycle);
            ShellSemanticSnapshot {
                terminal: Some(TerminalSemanticSnapshot {
                    terminal: buffer.snapshot(),
                    focus_mode: TerminalFocusMode::AccessibleReview,
                    input_authorized: true,
                    recording_friendly: false,
                    announcement,
                }),
                ..ShellSemanticSnapshot::default()
            }
        };
        let baseline = semantic_shape(&snapshot(b"short", TerminalLifecycle::Live, None));
        assert_ne!(
            baseline,
            semantic_shape(&snapshot(&vec![b'x'; 1_025], TerminalLifecycle::Live, None))
        );
        assert_ne!(
            baseline,
            semantic_shape(&snapshot(b"short", TerminalLifecycle::Error, None))
        );
        assert_ne!(
            semantic_shape(&snapshot(
                b"short",
                TerminalLifecycle::Live,
                Some(TerminalAnnouncement::OutputAvailable { bytes: 1 })
            )),
            semantic_shape(&snapshot(
                b"short",
                TerminalLifecycle::Live,
                Some(TerminalAnnouncement::Attention)
            ))
        );
    }
}
