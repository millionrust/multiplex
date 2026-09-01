use termirust_ui_contract::{
    AccessibilityBridge, LiveRegionPoliteness, SemanticError, SemanticErrorCode, SemanticNodeId,
    SemanticPatch,
};

#[derive(Default)]
pub struct RecordingAccessibilityBridge {
    available: bool,
    generation: Option<u64>,
    pub patches: Vec<SemanticPatch>,
    pub focus: Vec<Option<SemanticNodeId>>,
    pub announcements: Vec<(SemanticNodeId, LiveRegionPoliteness)>,
}

impl RecordingAccessibilityBridge {
    pub fn available() -> Self {
        Self {
            available: true,
            ..Self::default()
        }
    }

    fn require_generation(&self, generation: u64) -> Result<(), SemanticError> {
        if self.generation == Some(generation) {
            Ok(())
        } else {
            Err(SemanticError::new(SemanticErrorCode::StaleGeneration, None))
        }
    }
}

impl AccessibilityBridge for RecordingAccessibilityBridge {
    fn is_available(&self) -> bool {
        self.available
    }

    fn apply_patch(&mut self, patch: &SemanticPatch) -> Result<(), SemanticError> {
        if !self.available {
            return Err(SemanticError::new(
                SemanticErrorCode::BridgeUnavailable,
                None,
            ));
        }
        if self
            .generation
            .is_some_and(|generation| generation > patch.generation)
        {
            return Err(SemanticError::new(SemanticErrorCode::StaleGeneration, None));
        }
        self.generation = Some(patch.generation);
        self.patches.push(patch.clone());
        Ok(())
    }

    fn set_focus(
        &mut self,
        generation: u64,
        target: Option<SemanticNodeId>,
    ) -> Result<(), SemanticError> {
        self.require_generation(generation)?;
        self.focus.push(target);
        Ok(())
    }

    fn announce(
        &mut self,
        generation: u64,
        node: SemanticNodeId,
        politeness: LiveRegionPoliteness,
    ) -> Result<(), SemanticError> {
        self.require_generation(generation)?;
        self.announcements.push((node, politeness));
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacAccessibilityBridge;

#[cfg(target_os = "macos")]
mod macos {
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::ffi::CStr;
    use std::os::raw::c_char;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
    use std::sync::{Arc, Mutex, Once, OnceLock};

    use cocoa::base::{BOOL, NO, YES, nil};
    use cocoa::foundation::{NSArray, NSDictionary, NSPoint, NSRect, NSSize, NSString};
    use objc::declare::ClassDecl;
    use objc::rc::StrongPtr;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};
    use termirust_ui_contract::{
        AccessibilityBridge, LiveRegionPoliteness, MessageId, SemanticAction,
        SemanticActionRequest, SemanticActionValue, SemanticChange, SemanticError,
        SemanticErrorCode, SemanticNode, SemanticNodeId, SemanticPatch, SemanticRelationKind,
        SemanticRole, SemanticText, SemanticValue,
    };

    type Id = *mut Object;
    type MessageResolver = Arc<dyn Fn(MessageId) -> Option<String> + Send + Sync>;
    const ELEMENT_BASE_CLASS: &str = "TermiRustAccessibilityElementBase";
    const BRIDGE_IVAR: &str = "termirustBridgeId";
    const GENERATION_IVAR: &str = "termirustGeneration";
    const NODE_IVAR: &str = "termirustNodeId";
    const ACTIONS_IVAR: &str = "termirustActionBits";
    const FOCUSED_IVAR: &str = "termirustFocused";
    const ACTION_QUEUE_CAPACITY: usize = 64;

    static NEXT_BRIDGE_ID: AtomicU64 = AtomicU64::new(1);
    static ELEMENT_BASE_CLASS_ONCE: Once = Once::new();
    static ELEMENT_CLASSES: OnceLock<Mutex<HashMap<u64, &'static Class>>> = OnceLock::new();
    static ACTION_ROUTES: OnceLock<Mutex<HashMap<u64, SyncSender<SemanticActionRequest>>>> =
        OnceLock::new();

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        fn NSAccessibilityPostNotification(element: Id, notification: Id);
        fn NSAccessibilityPostNotificationWithUserInfo(
            element: Id,
            notification: Id,
            user_info: Id,
        );
        static NSAccessibilityLayoutChangedNotification: Id;
        static NSAccessibilityFocusedUIElementChangedNotification: Id;
        static NSAccessibilityAnnouncementRequestedNotification: Id;
        static NSAccessibilityAnnouncementKey: Id;
        static NSAccessibilityPriorityKey: Id;
        static NSAccessibilityPressAction: Id;
        static NSAccessibilityIncrementAction: Id;
        static NSAccessibilityDecrementAction: Id;
        static NSAccessibilityCancelAction: Id;
    }

    pub struct MacAccessibilityBridge {
        bridge_id: u64,
        generation: Option<u64>,
        root: Option<SemanticNodeId>,
        parent_view: StrongPtr,
        resolver: MessageResolver,
        models: BTreeMap<SemanticNodeId, SemanticNode>,
        elements: BTreeMap<SemanticNodeId, StrongPtr>,
        focused: Option<SemanticNodeId>,
    }

    impl MacAccessibilityBridge {
        pub fn bounded_action_channel() -> (
            SyncSender<SemanticActionRequest>,
            Receiver<SemanticActionRequest>,
        ) {
            mpsc::sync_channel(ACTION_QUEUE_CAPACITY)
        }

        pub fn attach_to_key_window(
            action_sender: SyncSender<SemanticActionRequest>,
            resolver: MessageResolver,
        ) -> Result<Self, SemanticError> {
            let parent_view = key_window_content_view()?;
            let bridge_id = NEXT_BRIDGE_ID.fetch_add(1, Ordering::Relaxed);
            action_routes()
                .lock()
                .map_err(|_| bridge_unavailable())?
                .insert(bridge_id, action_sender);
            Ok(Self {
                bridge_id,
                generation: None,
                root: None,
                parent_view,
                resolver,
                models: BTreeMap::new(),
                elements: BTreeMap::new(),
                focused: None,
            })
        }

        fn require_generation(&self, generation: u64) -> Result<(), SemanticError> {
            if self.generation == Some(generation) {
                Ok(())
            } else {
                Err(SemanticError::new(SemanticErrorCode::StaleGeneration, None))
            }
        }

        fn rebuild_native_tree(&mut self) -> Result<(), SemanticError> {
            for (node_id, node) in &self.models {
                let element = self.elements.get(node_id).ok_or_else(|| {
                    SemanticError::new(SemanticErrorCode::StaleNode, Some(*node_id))
                })?;
                configure_element(
                    **element,
                    self.bridge_id,
                    self.generation.unwrap_or_default(),
                    node,
                    &self.resolver,
                )?;
            }

            for (node_id, node) in &self.models {
                let element = **self.elements.get(node_id).ok_or_else(|| {
                    SemanticError::new(SemanticErrorCode::StaleNode, Some(*node_id))
                })?;
                let parent = match node.parent {
                    Some(parent) => **self.elements.get(&parent).ok_or_else(|| {
                        SemanticError::new(SemanticErrorCode::MissingParent, Some(*node_id))
                    })?,
                    None => *self.parent_view,
                };
                let children = self
                    .models
                    .values()
                    .filter(|candidate| {
                        candidate.parent == Some(*node_id) && !candidate.state.hidden
                    })
                    .filter_map(|candidate| self.elements.get(&candidate.id).map(|child| **child))
                    .collect::<Vec<_>>();
                let child_array = unsafe { NSArray::arrayWithObjects(nil, &children) };
                unsafe {
                    let _: () = msg_send![element, setAccessibilityParent: parent];
                    let _: () = msg_send![element, setAccessibilityChildren: child_array];
                    let _: () =
                        msg_send![element, setAccessibilityChildrenInNavigationOrder: child_array];
                }
                configure_relations(element, node, &self.elements);
            }

            let root = self
                .root
                .and_then(|root| self.elements.get(&root))
                .map(|root| **root)
                .ok_or_else(|| SemanticError::new(SemanticErrorCode::MissingRoot, self.root))?;
            let roots = unsafe { NSArray::arrayWithObjects(nil, &[root]) };
            unsafe {
                let _: () = msg_send![*self.parent_view, setAccessibilityChildren: roots];
                let _: () =
                    msg_send![*self.parent_view, setAccessibilityChildrenInNavigationOrder: roots];
                NSAccessibilityPostNotification(
                    *self.parent_view,
                    NSAccessibilityLayoutChangedNotification,
                );
            }
            Ok(())
        }
    }

    impl Drop for MacAccessibilityBridge {
        fn drop(&mut self) {
            if let Ok(mut routes) = action_routes().lock() {
                routes.remove(&self.bridge_id);
            }
            unsafe {
                let empty = NSArray::arrayWithObjects(nil, &[]);
                let _: () = msg_send![*self.parent_view, setAccessibilityChildren: empty];
            }
        }
    }

    impl AccessibilityBridge for MacAccessibilityBridge {
        fn is_available(&self) -> bool {
            true
        }

        fn apply_patch(&mut self, patch: &SemanticPatch) -> Result<(), SemanticError> {
            if self
                .generation
                .is_some_and(|generation| generation > patch.generation)
            {
                return Err(SemanticError::new(SemanticErrorCode::StaleGeneration, None));
            }
            if self.generation != Some(patch.generation) {
                self.models.clear();
                self.elements.clear();
            }
            self.generation = Some(patch.generation);
            self.root = Some(patch.root);
            for change in &patch.changes {
                match change {
                    SemanticChange::Removed(id) => {
                        self.models.remove(id);
                        self.elements.remove(id);
                    }
                    SemanticChange::Added(node) => {
                        self.elements
                            .insert(node.id, create_element(&node.actions)?);
                        self.models.insert(node.id, node.clone());
                    }
                    SemanticChange::Updated(node) => {
                        if !self.elements.contains_key(&node.id) {
                            return Err(SemanticError::new(
                                SemanticErrorCode::StaleNode,
                                Some(node.id),
                            ));
                        }
                        if self.models.get(&node.id).is_some_and(|current| {
                            action_bits(&current.actions) != action_bits(&node.actions)
                        }) {
                            self.elements
                                .insert(node.id, create_element(&node.actions)?);
                        }
                        self.models.insert(node.id, node.clone());
                    }
                }
            }
            self.rebuild_native_tree()
        }

        fn set_focus(
            &mut self,
            generation: u64,
            target: Option<SemanticNodeId>,
        ) -> Result<(), SemanticError> {
            self.require_generation(generation)?;
            if self.focused != target
                && let Some(previous) = self.focused
                && let Some(previous) = self.elements.get(&previous)
            {
                unsafe { set_model_accessibility_focused(**previous, NO) };
            }
            let element = match target {
                Some(target) => {
                    let element = **self.elements.get(&target).ok_or_else(|| {
                        SemanticError::new(SemanticErrorCode::StaleNode, Some(target))
                    })?;
                    unsafe { set_model_accessibility_focused(element, YES) };
                    element
                }
                None => *self.parent_view,
            };
            self.focused = target;
            unsafe {
                NSAccessibilityPostNotification(
                    element,
                    NSAccessibilityFocusedUIElementChangedNotification,
                );
            }
            Ok(())
        }

        fn announce(
            &mut self,
            generation: u64,
            node: SemanticNodeId,
            politeness: LiveRegionPoliteness,
        ) -> Result<(), SemanticError> {
            self.require_generation(generation)?;
            let model = self
                .models
                .get(&node)
                .ok_or_else(|| SemanticError::new(SemanticErrorCode::StaleNode, Some(node)))?;
            let element = **self
                .elements
                .get(&node)
                .ok_or_else(|| SemanticError::new(SemanticErrorCode::StaleNode, Some(node)))?;
            let announcement = resolve_text(model.name.as_ref(), &self.resolver)
                .or_else(|| resolve_text(model.description.as_ref(), &self.resolver))
                .ok_or_else(|| SemanticError::new(SemanticErrorCode::MissingName, Some(node)))?;
            unsafe {
                let announcement = NSString::alloc(nil).init_str(&announcement);
                let priority: Id = msg_send![class!(NSNumber), numberWithInteger: match politeness {
                    LiveRegionPoliteness::Polite => 50_i64,
                    LiveRegionPoliteness::Immediate => 90_i64,
                }];
                let values = NSArray::arrayWithObjects(nil, &[announcement, priority]);
                let keys = NSArray::arrayWithObjects(
                    nil,
                    &[NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey],
                );
                let user_info = NSDictionary::dictionaryWithObjects_forKeys_(nil, values, keys);
                NSAccessibilityPostNotificationWithUserInfo(
                    element,
                    NSAccessibilityAnnouncementRequestedNotification,
                    user_info,
                );
                let _: () = msg_send![announcement, release];
            }
            Ok(())
        }
    }

    fn action_routes() -> &'static Mutex<HashMap<u64, SyncSender<SemanticActionRequest>>> {
        ACTION_ROUTES.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn create_element(actions: &BTreeSet<SemanticAction>) -> Result<StrongPtr, SemanticError> {
        let class = element_class(action_bits(actions))?;
        unsafe {
            let allocated: Id = msg_send![class, alloc];
            let element: Id = msg_send![allocated, init];
            if element.is_null() {
                Err(bridge_unavailable())
            } else {
                Ok(StrongPtr::new(element))
            }
        }
    }

    fn register_element_base_class() {
        ELEMENT_BASE_CLASS_ONCE.call_once(|| unsafe {
            let superclass = class!(NSAccessibilityElement);
            let Some(mut declaration) = ClassDecl::new(ELEMENT_BASE_CLASS, superclass) else {
                return;
            };
            declaration.add_ivar::<u64>(BRIDGE_IVAR);
            declaration.add_ivar::<u64>(GENERATION_IVAR);
            declaration.add_ivar::<u64>(NODE_IVAR);
            declaration.add_ivar::<u64>(ACTIONS_IVAR);
            declaration.add_ivar::<u64>(FOCUSED_IVAR);
            declaration.add_method(
                sel!(accessibilityActionNames),
                accessibility_action_names as extern "C" fn(&Object, Sel) -> Id,
            );
            declaration.add_method(
                sel!(isAccessibilitySelectorAllowed:),
                is_accessibility_selector_allowed as extern "C" fn(&Object, Sel, Sel) -> BOOL,
            );
            declaration.add_method(
                sel!(accessibilityFocused),
                accessibility_focused as extern "C" fn(&Object, Sel) -> BOOL,
            );
            declaration.register();
        });
    }

    fn element_class(actions: u64) -> Result<&'static Class, SemanticError> {
        register_element_base_class();
        let superclass = Class::get(ELEMENT_BASE_CLASS).ok_or_else(bridge_unavailable)?;
        let classes = ELEMENT_CLASSES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut classes = classes.lock().map_err(|_| bridge_unavailable())?;
        if let Some(class) = classes.get(&actions) {
            return Ok(*class);
        }
        let name = format!("TermiRustAccessibilityElement{actions:016x}");
        let mut declaration = ClassDecl::new(&name, superclass).ok_or_else(bridge_unavailable)?;
        unsafe {
            if actions & (1_u64 << (SemanticAction::Activate as u8)) != 0 {
                declaration.add_method(
                    sel!(accessibilityPerformPress),
                    perform_press as extern "C" fn(&Object, Sel) -> BOOL,
                );
            }
            if actions & (1_u64 << (SemanticAction::Focus as u8)) != 0 {
                declaration.add_method(
                    sel!(setAccessibilityFocused:),
                    set_focused as extern "C" fn(&mut Object, Sel, BOOL),
                );
            }
            if actions & (1_u64 << (SemanticAction::SetValue as u8)) != 0 {
                declaration.add_method(
                    sel!(setAccessibilityValue:),
                    set_value as extern "C" fn(&mut Object, Sel, Id),
                );
            }
            if actions & (1_u64 << (SemanticAction::Increment as u8)) != 0 {
                declaration.add_method(
                    sel!(accessibilityPerformIncrement),
                    perform_increment as extern "C" fn(&Object, Sel),
                );
            }
            if actions & (1_u64 << (SemanticAction::Decrement as u8)) != 0 {
                declaration.add_method(
                    sel!(accessibilityPerformDecrement),
                    perform_decrement as extern "C" fn(&Object, Sel),
                );
            }
            if actions
                & ((1_u64 << (SemanticAction::Cancel as u8))
                    | (1_u64 << (SemanticAction::Dismiss as u8)))
                != 0
            {
                declaration.add_method(
                    sel!(accessibilityPerformCancel),
                    perform_cancel as extern "C" fn(&Object, Sel) -> BOOL,
                );
            }
        }
        let class = declaration.register();
        classes.insert(actions, class);
        Ok(class)
    }

    fn configure_element(
        element: Id,
        bridge_id: u64,
        generation: u64,
        node: &SemanticNode,
        resolver: &MessageResolver,
    ) -> Result<(), SemanticError> {
        let role = ns_string(role_name(node.role));
        let identifier = ns_string(&format!("termirust.ax.{generation}.{}", node.id.get()));
        let label = node
            .name
            .as_ref()
            .map(|text| {
                resolve_text(Some(text), resolver)
                    .map(|text| ns_string(&text))
                    .ok_or_else(|| {
                        SemanticError::new(SemanticErrorCode::MissingName, Some(node.id))
                    })
            })
            .transpose()?;
        let help = node
            .description
            .as_ref()
            .map(|text| {
                resolve_text(Some(text), resolver)
                    .map(|text| ns_string(&text))
                    .ok_or_else(|| {
                        SemanticError::new(SemanticErrorCode::MissingName, Some(node.id))
                    })
            })
            .transpose()?;
        let public_text_value = match node.value.as_ref() {
            Some(SemanticValue::PublicText(text)) => {
                Some(ns_string(&resolve_text(Some(text), resolver).ok_or_else(
                    || SemanticError::new(SemanticErrorCode::InvalidValue, Some(node.id)),
                )?))
            }
            _ => None,
        };
        let bounds = NSRect::new(
            NSPoint::new(f64::from(node.bounds.x), f64::from(node.bounds.y)),
            NSSize::new(f64::from(node.bounds.width), f64::from(node.bounds.height)),
        );
        unsafe {
            (*element).set_ivar(BRIDGE_IVAR, bridge_id);
            (*element).set_ivar(GENERATION_IVAR, generation);
            (*element).set_ivar(NODE_IVAR, node.id.get());
            (*element).set_ivar(ACTIONS_IVAR, action_bits(&node.actions));
            let _: () = msg_send![element, setAccessibilityElement: if node.state.hidden { NO } else { YES }];
            let _: () = msg_send![element, setAccessibilityRole: *role];
            let _: () = msg_send![element, setAccessibilityIdentifier: *identifier];
            let _: () = msg_send![element, setAccessibilityFrameInParentSpace: bounds];
            let _: () = msg_send![element, setAccessibilityEnabled: if node.state.disabled { NO } else { YES }];
            let _: () = msg_send![element, setAccessibilitySelected: if node.state.selected { YES } else { NO }];
            if let Some(expanded) = node.state.expanded {
                let _: () =
                    msg_send![element, setAccessibilityExpanded: if expanded { YES } else { NO }];
            }
            if let Some(checked) = node.state.checked {
                let value: Id =
                    msg_send![class!(NSNumber), numberWithBool: if checked { YES } else { NO }];
                set_model_accessibility_value(element, value);
            }
            match &node.value {
                Some(SemanticValue::PublicText(_)) => {
                    set_model_accessibility_value(
                        element,
                        **public_text_value
                            .as_ref()
                            .expect("public semantic text was resolved"),
                    );
                }
                Some(SemanticValue::Boolean(value)) => {
                    let value: Id =
                        msg_send![class!(NSNumber), numberWithBool: if *value { YES } else { NO }];
                    set_model_accessibility_value(element, value);
                }
                Some(SemanticValue::Number {
                    current,
                    minimum,
                    maximum,
                }) => {
                    let current: Id = msg_send![class!(NSNumber), numberWithLongLong: *current];
                    let minimum: Id = msg_send![class!(NSNumber), numberWithLongLong: *minimum];
                    let maximum: Id = msg_send![class!(NSNumber), numberWithLongLong: *maximum];
                    set_model_accessibility_value(element, current);
                    let _: () = msg_send![element, setAccessibilityMinValue: minimum];
                    let _: () = msg_send![element, setAccessibilityMaxValue: maximum];
                }
                None => {}
            }
            let _: () = msg_send![element, setAccessibilityLabel: label.as_ref().map_or(nil, |value| **value)];
            let _: () = msg_send![element, setAccessibilityHelp: help.as_ref().map_or(nil, |value| **value)];
        }
        Ok(())
    }

    unsafe fn set_model_accessibility_value(element: Id, value: Id) {
        // The subclass setter handles edits originating from assistive technology.
        // Bypass it while publishing the authoritative model value so the inherited
        // NSAccessibilityElement storage remains readable through AXValue.
        let _: () = unsafe {
            msg_send![super(element, class!(NSAccessibilityElement)), setAccessibilityValue: value]
        };
    }

    unsafe fn set_model_accessibility_focused(element: Id, focused: BOOL) {
        unsafe {
            (*element).set_ivar(FOCUSED_IVAR, u64::from(focused == YES));
        }
    }

    #[cfg(test)]
    pub(super) fn stored_model_text_value_for_test(value: &str) -> Option<String> {
        let actions = BTreeSet::from([SemanticAction::SetValue]);
        let element = create_element(&actions).ok()?;
        let value = ns_string(value);
        unsafe {
            set_model_accessibility_value(*element, *value);
            let stored: Id = msg_send![*element, accessibilityValue];
            if stored.is_null() {
                return None;
            }
            let utf8: *const c_char = msg_send![stored, UTF8String];
            (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
        }
    }

    #[cfg(test)]
    pub(super) fn stored_model_focus_for_test(focused: bool) -> bool {
        let actions = BTreeSet::from([SemanticAction::Focus]);
        let element = create_element(&actions).expect("native accessibility element");
        unsafe {
            set_model_accessibility_focused(*element, if focused { YES } else { NO });
            let stored: BOOL = msg_send![*element, accessibilityFocused];
            stored == YES
        }
    }

    #[cfg(test)]
    pub(super) fn native_action_names_for_test(
        actions: &std::collections::BTreeSet<SemanticAction>,
    ) -> Vec<String> {
        let element = create_element(actions).expect("native accessibility element");
        unsafe {
            (**element).set_ivar(ACTIONS_IVAR, action_bits(actions));
            let names: Id = msg_send![*element, accessibilityActionNames];
            let count: usize = msg_send![names, count];
            (0..count)
                .filter_map(|index| {
                    let name: Id = msg_send![names, objectAtIndex: index];
                    let utf8: *const c_char = msg_send![name, UTF8String];
                    (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
                })
                .collect()
        }
    }

    #[cfg(test)]
    pub(super) fn native_action_selector_permissions_for_test(
        actions: &std::collections::BTreeSet<SemanticAction>,
    ) -> [bool; 4] {
        let element = create_element(actions).expect("native accessibility element");
        unsafe {
            (**element).set_ivar(ACTIONS_IVAR, action_bits(actions));
            [
                msg_send![*element, isAccessibilitySelectorAllowed: sel!(accessibilityPerformPress)],
                msg_send![*element, isAccessibilitySelectorAllowed: sel!(accessibilityPerformIncrement)],
                msg_send![*element, isAccessibilitySelectorAllowed: sel!(accessibilityPerformDecrement)],
                msg_send![*element, isAccessibilitySelectorAllowed: sel!(accessibilityPerformCancel)],
            ]
            .map(|allowed: BOOL| allowed == YES)
        }
    }

    fn configure_relations(
        element: Id,
        node: &SemanticNode,
        elements: &BTreeMap<SemanticNodeId, StrongPtr>,
    ) {
        let related = |kind| {
            node.relations
                .iter()
                .filter(|relation| relation.kind == kind)
                .filter_map(|relation| elements.get(&relation.target).map(|target| **target))
                .collect::<Vec<_>>()
        };
        unsafe {
            let labels = related(SemanticRelationKind::LabelledBy);
            let label_array = NSArray::arrayWithObjects(nil, &labels);
            let _: () = msg_send![element, setAccessibilityLabelUIElements: label_array];
            let linked = [
                related(SemanticRelationKind::Controls),
                related(SemanticRelationKind::Owns),
                related(SemanticRelationKind::DescribedBy),
                related(SemanticRelationKind::ErrorMessage),
            ]
            .concat();
            let linked_array = NSArray::arrayWithObjects(nil, &linked);
            let _: () = msg_send![element, setAccessibilityLinkedUIElements: linked_array];
        }
    }

    fn ns_string(value: &str) -> StrongPtr {
        unsafe { StrongPtr::new(NSString::alloc(nil).init_str(value)) }
    }

    fn resolve_text(text: Option<&SemanticText>, resolver: &MessageResolver) -> Option<String> {
        match text? {
            SemanticText::Message(message) => resolver(*message),
            SemanticText::UserText(text) => Some(text.clone()),
        }
    }

    pub(super) fn role_name(role: SemanticRole) -> &'static str {
        match role {
            SemanticRole::Application | SemanticRole::Landmark | SemanticRole::Group => "AXGroup",
            SemanticRole::Heading => "AXHeading",
            SemanticRole::List => "AXList",
            SemanticRole::ListItem => "AXRow",
            SemanticRole::Button => "AXButton",
            SemanticRole::TextField => "AXTextField",
            SemanticRole::StaticText => "AXStaticText",
            SemanticRole::Menu => "AXMenu",
            SemanticRole::MenuItem => "AXMenuItem",
            SemanticRole::Dialog => "AXDialog",
            SemanticRole::ProgressIndicator => "AXProgressIndicator",
            SemanticRole::Status => "AXStaticText",
            SemanticRole::Alert => "AXGroup",
            SemanticRole::Checkbox => "AXCheckBox",
            SemanticRole::RadioButton => "AXRadioButton",
            SemanticRole::Tab => "AXRadioButton",
            SemanticRole::TabList => "AXTabGroup",
        }
    }

    pub(super) fn action_bits(actions: &std::collections::BTreeSet<SemanticAction>) -> u64 {
        actions
            .iter()
            .fold(0_u64, |bits, action| bits | 1_u64 << (*action as u8))
    }

    fn dispatch(this: &Object, action: SemanticAction, value: Option<SemanticActionValue>) -> bool {
        let (bridge_id, generation, node_id, actions) = unsafe {
            (
                *this.get_ivar::<u64>(BRIDGE_IVAR),
                *this.get_ivar::<u64>(GENERATION_IVAR),
                *this.get_ivar::<u64>(NODE_IVAR),
                *this.get_ivar::<u64>(ACTIONS_IVAR),
            )
        };
        if actions & (1_u64 << (action as u8)) == 0 {
            return false;
        }
        let Some(node) = std::num::NonZeroU64::new(node_id).map(SemanticNodeId::new) else {
            return false;
        };
        let Ok(routes) = action_routes().lock() else {
            return false;
        };
        let Some(sender) = routes.get(&bridge_id) else {
            return false;
        };
        match sender.try_send(SemanticActionRequest {
            generation,
            node,
            action,
            value,
        }) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    extern "C" fn perform_press(this: &Object, _: Sel) -> BOOL {
        if dispatch(this, SemanticAction::Activate, None) {
            YES
        } else {
            NO
        }
    }

    extern "C" fn accessibility_action_names(this: &Object, _: Sel) -> Id {
        let actions = unsafe { *this.get_ivar::<u64>(ACTIONS_IVAR) };
        let mut names = Vec::with_capacity(4);
        unsafe {
            if actions & (1_u64 << (SemanticAction::Activate as u8)) != 0 {
                names.push(NSAccessibilityPressAction);
            }
            if actions & (1_u64 << (SemanticAction::Increment as u8)) != 0 {
                names.push(NSAccessibilityIncrementAction);
            }
            if actions & (1_u64 << (SemanticAction::Decrement as u8)) != 0 {
                names.push(NSAccessibilityDecrementAction);
            }
            if actions
                & ((1_u64 << (SemanticAction::Cancel as u8))
                    | (1_u64 << (SemanticAction::Dismiss as u8)))
                != 0
            {
                names.push(NSAccessibilityCancelAction);
            }
            NSArray::arrayWithObjects(nil, &names)
        }
    }

    extern "C" fn is_accessibility_selector_allowed(this: &Object, _: Sel, selector: Sel) -> BOOL {
        let actions = unsafe { *this.get_ivar::<u64>(ACTIONS_IVAR) };
        let supports = |action: SemanticAction| actions & (1_u64 << (action as u8)) != 0;
        if selector == sel!(accessibilityPerformPress) {
            return if supports(SemanticAction::Activate) {
                YES
            } else {
                NO
            };
        }
        if selector == sel!(setAccessibilityFocused:) {
            return if supports(SemanticAction::Focus) {
                YES
            } else {
                NO
            };
        }
        if selector == sel!(setAccessibilityValue:) {
            return if supports(SemanticAction::SetValue) {
                YES
            } else {
                NO
            };
        }
        if selector == sel!(accessibilityPerformIncrement) {
            return if supports(SemanticAction::Increment) {
                YES
            } else {
                NO
            };
        }
        if selector == sel!(accessibilityPerformDecrement) {
            return if supports(SemanticAction::Decrement) {
                YES
            } else {
                NO
            };
        }
        if selector == sel!(accessibilityPerformCancel) {
            return if supports(SemanticAction::Cancel) || supports(SemanticAction::Dismiss) {
                YES
            } else {
                NO
            };
        }
        unsafe {
            msg_send![super(this, class!(NSAccessibilityElement)), isAccessibilitySelectorAllowed: selector]
        }
    }

    extern "C" fn set_focused(this: &mut Object, _: Sel, focused: BOOL) {
        if focused == YES {
            let _ = dispatch(this, SemanticAction::Focus, None);
        }
    }

    extern "C" fn accessibility_focused(this: &Object, _: Sel) -> BOOL {
        if unsafe { *this.get_ivar::<u64>(FOCUSED_IVAR) } == 0 {
            NO
        } else {
            YES
        }
    }

    extern "C" fn set_value(this: &mut Object, _: Sel, value: Id) {
        if value.is_null() {
            return;
        }
        let is_string: BOOL = unsafe { msg_send![value, isKindOfClass: class!(NSString)] };
        if is_string != YES {
            return;
        }
        let utf8: *const c_char = unsafe { msg_send![value, UTF8String] };
        if utf8.is_null() {
            return;
        }
        let Ok(text) = unsafe { CStr::from_ptr(utf8) }.to_str() else {
            return;
        };
        let Ok(value) = SemanticActionValue::text(text) else {
            return;
        };
        let _ = dispatch(this, SemanticAction::SetValue, Some(value));
    }

    extern "C" fn perform_increment(this: &Object, _: Sel) {
        let _ = dispatch(this, SemanticAction::Increment, None);
    }

    extern "C" fn perform_decrement(this: &Object, _: Sel) {
        let _ = dispatch(this, SemanticAction::Decrement, None);
    }

    extern "C" fn perform_cancel(this: &Object, _: Sel) -> BOOL {
        if dispatch(this, SemanticAction::Cancel, None)
            || dispatch(this, SemanticAction::Dismiss, None)
        {
            YES
        } else {
            NO
        }
    }

    fn key_window_content_view() -> Result<StrongPtr, SemanticError> {
        unsafe {
            let is_main: BOOL = msg_send![class!(NSThread), isMainThread];
            if is_main != YES {
                return Err(bridge_unavailable());
            }
            let application: Id = msg_send![class!(NSApplication), sharedApplication];
            let mut window: Id = msg_send![application, keyWindow];
            if window.is_null() {
                let windows: Id = msg_send![application, windows];
                let count: usize = msg_send![windows, count];
                if count > 0 {
                    window = msg_send![windows, objectAtIndex: 0_usize];
                }
            }
            if window.is_null() {
                return Err(bridge_unavailable());
            }
            let view: Id = msg_send![window, contentView];
            if view.is_null() {
                Err(bridge_unavailable())
            } else {
                Ok(StrongPtr::retain(view))
            }
        }
    }

    fn bridge_unavailable() -> SemanticError {
        SemanticError::new(SemanticErrorCode::BridgeUnavailable, None)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use termirust_ui_contract::{
        AccessibilityBridge, SemanticChange, SemanticNodeId, SemanticPatch,
    };

    use super::RecordingAccessibilityBridge;
    #[cfg(target_os = "macos")]
    use super::macos::{action_bits, role_name};

    fn id(value: u64) -> SemanticNodeId {
        SemanticNodeId::new(NonZeroU64::new(value).unwrap())
    }

    #[test]
    fn recording_bridge_rejects_unavailable_and_stale_updates() {
        let patch = SemanticPatch {
            generation: 3,
            root: id(1),
            changes: Vec::<SemanticChange>::new(),
        };
        let mut unavailable = RecordingAccessibilityBridge::default();
        assert!(unavailable.apply_patch(&patch).is_err());

        let mut bridge = RecordingAccessibilityBridge::available();
        bridge.apply_patch(&patch).unwrap();
        bridge.set_focus(3, Some(id(1))).unwrap();
        assert!(bridge.set_focus(2, Some(id(1))).is_err());
        let stale = SemanticPatch {
            generation: 2,
            ..patch
        };
        assert!(bridge.apply_patch(&stale).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_role_and_action_mapping_is_deterministic() {
        use std::collections::BTreeSet;

        use termirust_ui_contract::{SemanticAction, SemanticRole};

        assert_eq!(role_name(SemanticRole::Button), "AXButton");
        assert_eq!(role_name(SemanticRole::Dialog), "AXDialog");
        assert_eq!(role_name(SemanticRole::TextField), "AXTextField");

        let actions = BTreeSet::from([SemanticAction::Focus, SemanticAction::Activate]);
        assert_eq!(
            action_bits(&actions),
            (1_u64 << SemanticAction::Focus as u8) | (1_u64 << SemanticAction::Activate as u8)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_action_channel_is_bounded() {
        use termirust_ui_contract::{SemanticAction, SemanticActionRequest};

        let (sender, receiver) = super::MacAccessibilityBridge::bounded_action_channel();
        let request = SemanticActionRequest {
            generation: 1,
            node: id(1),
            action: SemanticAction::Activate,
            value: None,
        };
        for _ in 0..64 {
            sender.try_send(request.clone()).unwrap();
        }
        assert!(sender.try_send(request).is_err());
        assert_eq!(receiver.try_iter().count(), 64);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_model_text_value_is_readable_through_ax_value() {
        assert_eq!(
            super::macos::stored_model_text_value_for_test("Temporary label").as_deref(),
            Some("Temporary label")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_model_focus_is_readable_through_ax_focused() {
        assert!(super::macos::stored_model_focus_for_test(true));
        assert!(!super::macos::stored_model_focus_for_test(false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_action_names_only_advertise_supported_actions() {
        use std::collections::BTreeSet;

        use termirust_ui_contract::SemanticAction;

        let button = BTreeSet::from([SemanticAction::Focus, SemanticAction::Activate]);
        assert_eq!(
            super::macos::native_action_names_for_test(&button),
            ["AXPress"]
        );
        assert_eq!(
            super::macos::native_action_selector_permissions_for_test(&button),
            [true, false, false, false]
        );

        let progress = BTreeSet::from([
            SemanticAction::Focus,
            SemanticAction::Increment,
            SemanticAction::Decrement,
            SemanticAction::Cancel,
        ]);
        assert_eq!(
            super::macos::native_action_names_for_test(&progress),
            ["AXIncrement", "AXDecrement", "AXCancel"]
        );
        assert_eq!(
            super::macos::native_action_selector_permissions_for_test(&progress),
            [false, true, true, true]
        );
    }
}
