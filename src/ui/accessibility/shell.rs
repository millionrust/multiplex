use termirust_ui_contract::{
    AccessibilityBridge, SemanticActionRequest, SemanticActionRouter, SemanticActionValue,
    SemanticDiffer, SemanticErrorCode, ShellAccessibilityCommand, ShellRegionId,
    ShellSemanticSnapshot, shell_palette_input_semantic_node, shell_region_semantic_node,
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
            last_error: None,
        }
    }

    pub fn sync(&mut self, snapshot: ShellSemanticSnapshot) {
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
                }
            }
            self.router = Some(router);
            self.last_palette_open = snapshot.palette_open;
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

impl Default for ShellAccessibilityAdapter {
    fn default() -> Self {
        Self::new()
    }
}
