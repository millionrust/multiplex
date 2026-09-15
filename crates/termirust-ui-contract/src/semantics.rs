use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::MessageId;

pub const MAX_SEMANTIC_NODES: usize = 10_000;
pub const MAX_SEMANTIC_TEXT_CHARS: usize = 1_024;
pub const MAX_SEMANTIC_RELATIONS: usize = 32;
pub const MAX_SEMANTIC_ACTION_VALUE_CHARS: usize = 4_096;
pub const SEMANTIC_UPDATE_INTERVAL: Duration = Duration::from_micros(16_667);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticNodeId(NonZeroU64);

impl SemanticNodeId {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FocusTargetId(SemanticNodeId);

impl FocusTargetId {
    pub const fn new(id: SemanticNodeId) -> Self {
        Self(id)
    }

    pub const fn semantic_id(self) -> SemanticNodeId {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticText {
    Message(MessageId),
    UserText(String),
}

impl SemanticText {
    pub fn user_text(value: impl Into<String>) -> Result<Self, SemanticError> {
        let value = value.into();
        validate_text(&value)?;
        Ok(Self::UserText(value))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticRole {
    Application,
    Landmark,
    Heading,
    List,
    ListItem,
    Button,
    TextField,
    StaticText,
    Menu,
    MenuItem,
    Dialog,
    ProgressIndicator,
    Status,
    Alert,
    Checkbox,
    RadioButton,
    Tab,
    TabList,
    Group,
}

impl SemanticRole {
    fn requires_name(self) -> bool {
        matches!(
            self,
            Self::Landmark
                | Self::Heading
                | Self::ListItem
                | Self::Button
                | Self::TextField
                | Self::Menu
                | Self::MenuItem
                | Self::Dialog
                | Self::ProgressIndicator
                | Self::Status
                | Self::Alert
                | Self::Checkbox
                | Self::RadioButton
                | Self::Tab
                | Self::TabList
        )
    }

    fn supports(self, action: SemanticAction) -> bool {
        match action {
            SemanticAction::Focus => !matches!(self, Self::Application | Self::StaticText),
            SemanticAction::Activate => matches!(
                self,
                Self::Button
                    | Self::ListItem
                    | Self::MenuItem
                    | Self::Checkbox
                    | Self::RadioButton
                    | Self::Tab
            ),
            SemanticAction::SetValue => matches!(self, Self::TextField),
            SemanticAction::Increment | SemanticAction::Decrement => {
                matches!(self, Self::ProgressIndicator)
            }
            SemanticAction::Dismiss | SemanticAction::Cancel => {
                matches!(self, Self::Dialog | Self::Menu | Self::ProgressIndicator)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticAction {
    Focus,
    Activate,
    SetValue,
    Increment,
    Decrement,
    Dismiss,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticRelationKind {
    LabelledBy,
    DescribedBy,
    Controls,
    Owns,
    ErrorMessage,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticRelation {
    pub kind: SemanticRelationKind,
    pub target: SemanticNodeId,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SemanticState {
    pub disabled: bool,
    pub selected: bool,
    pub expanded: Option<bool>,
    pub checked: Option<bool>,
    pub invalid: bool,
    pub busy: bool,
    pub hidden: bool,
    pub live: Option<LiveRegionPoliteness>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SemanticBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveRegionPoliteness {
    Polite,
    Immediate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticNode {
    pub id: SemanticNodeId,
    pub parent: Option<SemanticNodeId>,
    pub role: SemanticRole,
    pub name: Option<SemanticText>,
    pub description: Option<SemanticText>,
    pub value: Option<SemanticValue>,
    pub bounds: SemanticBounds,
    pub state: SemanticState,
    pub relations: Vec<SemanticRelation>,
    pub actions: BTreeSet<SemanticAction>,
}

impl SemanticNode {
    pub fn new(id: SemanticNodeId, role: SemanticRole) -> Self {
        Self {
            id,
            parent: None,
            role,
            name: None,
            description: None,
            value: None,
            bounds: SemanticBounds::default(),
            state: SemanticState::default(),
            relations: Vec::new(),
            actions: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticValue {
    PublicText(SemanticText),
    Boolean(bool),
    Number {
        current: i64,
        minimum: i64,
        maximum: i64,
    },
}

impl SemanticValue {
    pub fn public_user_text(value: impl Into<String>) -> Result<Self, SemanticError> {
        Ok(Self::PublicText(SemanticText::user_text(value)?))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTree {
    generation: u64,
    root: SemanticNodeId,
    nodes: BTreeMap<SemanticNodeId, SemanticNode>,
}

impl SemanticTree {
    pub fn try_new(
        generation: u64,
        root: SemanticNodeId,
        nodes: impl IntoIterator<Item = SemanticNode>,
    ) -> Result<Self, SemanticError> {
        let mut indexed = BTreeMap::new();
        for node in nodes {
            let id = node.id;
            if indexed.insert(id, node).is_some() {
                return Err(SemanticError::new(
                    SemanticErrorCode::DuplicateNode,
                    Some(id),
                ));
            }
            if indexed.len() > MAX_SEMANTIC_NODES {
                return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
            }
        }
        let tree = Self {
            generation,
            root,
            nodes: indexed,
        };
        tree.validate()?;
        Ok(tree)
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn root(&self) -> SemanticNodeId {
        self.root
    }

    pub fn nodes(&self) -> &BTreeMap<SemanticNodeId, SemanticNode> {
        &self.nodes
    }

    pub fn node(&self, id: SemanticNodeId) -> Option<&SemanticNode> {
        self.nodes.get(&id)
    }

    fn validate(&self) -> Result<(), SemanticError> {
        let Some(root) = self.nodes.get(&self.root) else {
            return Err(SemanticError::new(
                SemanticErrorCode::MissingRoot,
                Some(self.root),
            ));
        };
        if root.parent.is_some() {
            return Err(SemanticError::new(
                SemanticErrorCode::InvalidRoot,
                Some(self.root),
            ));
        }
        for node in self.nodes.values() {
            validate_node(node, &self.nodes)?;
            let mut seen = BTreeSet::new();
            let mut cursor = Some(node.id);
            while let Some(id) = cursor {
                if !seen.insert(id) {
                    return Err(SemanticError::new(SemanticErrorCode::ParentCycle, Some(id)));
                }
                cursor = self.nodes.get(&id).and_then(|candidate| candidate.parent);
            }
            if !seen.contains(&self.root) {
                return Err(SemanticError::new(
                    SemanticErrorCode::DisconnectedNode,
                    Some(node.id),
                ));
            }
        }
        Ok(())
    }
}

fn validate_node(
    node: &SemanticNode,
    nodes: &BTreeMap<SemanticNodeId, SemanticNode>,
) -> Result<(), SemanticError> {
    if node.role.requires_name() && node.name.is_none() {
        return Err(SemanticError::new(
            SemanticErrorCode::MissingName,
            Some(node.id),
        ));
    }
    if let Some(SemanticText::UserText(text)) = &node.name {
        validate_text(text)?;
    }
    if let Some(SemanticText::UserText(text)) = &node.description {
        validate_text(text)?;
    }
    match &node.value {
        Some(SemanticValue::PublicText(SemanticText::UserText(text))) => validate_text(text)?,
        Some(SemanticValue::Number {
            current,
            minimum,
            maximum,
        }) if minimum > maximum || current < minimum || current > maximum => {
            return Err(SemanticError::new(
                SemanticErrorCode::InvalidValue,
                Some(node.id),
            ));
        }
        _ => {}
    }
    if node.relations.len() > MAX_SEMANTIC_RELATIONS {
        return Err(SemanticError::new(
            SemanticErrorCode::ResourceLimit,
            Some(node.id),
        ));
    }
    let mut relations = BTreeSet::new();
    for relation in &node.relations {
        if !nodes.contains_key(&relation.target) {
            return Err(SemanticError::new(
                SemanticErrorCode::MissingRelationTarget,
                Some(node.id),
            ));
        }
        if !relations.insert(*relation) {
            return Err(SemanticError::new(
                SemanticErrorCode::DuplicateRelation,
                Some(node.id),
            ));
        }
    }
    if node
        .actions
        .iter()
        .any(|action| !node.role.supports(*action))
    {
        return Err(SemanticError::new(
            SemanticErrorCode::UnsupportedAction,
            Some(node.id),
        ));
    }
    Ok(())
}

fn validate_text(text: &str) -> Result<(), SemanticError> {
    if text.chars().count() > MAX_SEMANTIC_TEXT_CHARS
        || text.chars().any(|character| character.is_control())
    {
        return Err(SemanticError::new(SemanticErrorCode::UnsafeText, None));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticErrorCode {
    DuplicateNode,
    MissingRoot,
    InvalidRoot,
    MissingParent,
    ParentCycle,
    DisconnectedNode,
    MissingName,
    MissingRelationTarget,
    DuplicateRelation,
    UnsupportedAction,
    StaleGeneration,
    StaleNode,
    ReusedNode,
    IdentityChanged,
    InvalidValue,
    UnsafeText,
    ResourceLimit,
    BridgeUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticError {
    pub code: SemanticErrorCode,
    pub node: Option<SemanticNodeId>,
}

impl SemanticError {
    pub const fn new(code: SemanticErrorCode, node: Option<SemanticNodeId>) -> Self {
        Self { code, node }
    }
}

impl fmt::Display for SemanticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "semantic contract error: {:?}", self.code)
    }
}

impl std::error::Error for SemanticError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticChange {
    Removed(SemanticNodeId),
    Added(SemanticNode),
    Updated(SemanticNode),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticPatch {
    pub generation: u64,
    pub root: SemanticNodeId,
    pub changes: Vec<SemanticChange>,
}

#[derive(Default)]
pub struct SemanticDiffer {
    previous: Option<SemanticTree>,
    retired: BTreeSet<SemanticNodeId>,
}

impl SemanticDiffer {
    pub fn diff(&mut self, next: SemanticTree) -> Result<SemanticPatch, SemanticError> {
        let generation_changed = self
            .previous
            .as_ref()
            .is_some_and(|previous| previous.generation != next.generation);
        if generation_changed {
            self.previous = None;
            self.retired.clear();
        }
        let mut changes = Vec::new();
        if let Some(previous) = &self.previous {
            for id in previous.nodes.keys() {
                if !next.nodes.contains_key(id) {
                    self.retired.insert(*id);
                    changes.push(SemanticChange::Removed(*id));
                }
            }
        }
        for (id, node) in &next.nodes {
            match self.previous.as_ref().and_then(|tree| tree.nodes.get(id)) {
                Some(previous) if previous.role != node.role || previous.parent != node.parent => {
                    return Err(SemanticError::new(
                        SemanticErrorCode::IdentityChanged,
                        Some(*id),
                    ));
                }
                Some(previous) if previous != node => {
                    changes.push(SemanticChange::Updated(node.clone()));
                }
                Some(_) => {}
                None if self.retired.contains(id) => {
                    return Err(SemanticError::new(SemanticErrorCode::ReusedNode, Some(*id)));
                }
                None => changes.push(SemanticChange::Added(node.clone())),
            }
        }
        let patch = SemanticPatch {
            generation: next.generation,
            root: next.root,
            changes,
        };
        self.previous = Some(next);
        Ok(patch)
    }

    pub fn current(&self) -> Option<&SemanticTree> {
        self.previous.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticUrgency {
    Normal,
    Final,
    Error,
    Security,
}

impl SemanticUrgency {
    const fn immediate(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

#[derive(Default)]
pub struct SemanticUpdateCoalescer {
    last_emitted: Option<Instant>,
    pending: Option<SemanticTree>,
}

impl SemanticUpdateCoalescer {
    pub fn submit(
        &mut self,
        tree: SemanticTree,
        urgency: SemanticUrgency,
        now: Instant,
    ) -> Option<SemanticTree> {
        if urgency.immediate()
            || self
                .last_emitted
                .is_none_or(|last| now.saturating_duration_since(last) >= SEMANTIC_UPDATE_INTERVAL)
        {
            self.pending = None;
            self.last_emitted = Some(now);
            return Some(tree);
        }
        self.pending = Some(tree);
        None
    }

    pub fn flush(&mut self, now: Instant) -> Option<SemanticTree> {
        if self.pending.is_some()
            && self
                .last_emitted
                .is_none_or(|last| now.saturating_duration_since(last) >= SEMANTIC_UPDATE_INTERVAL)
        {
            self.last_emitted = Some(now);
            return self.pending.take();
        }
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticActionRequest {
    pub generation: u64,
    pub node: SemanticNodeId,
    pub action: SemanticAction,
    pub value: Option<SemanticActionValue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticActionValue {
    Text(String),
    Boolean(bool),
    Number(i64),
}

impl SemanticActionValue {
    pub fn text(value: impl Into<String>) -> Result<Self, SemanticError> {
        let value = value.into();
        if value.chars().count() > MAX_SEMANTIC_ACTION_VALUE_CHARS
            || value.chars().any(|character| character == '\0')
        {
            return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
        }
        Ok(Self::Text(value))
    }
}

pub struct SemanticActionRouter<Command> {
    generation: u64,
    routes: BTreeMap<(SemanticNodeId, SemanticAction), Command>,
}

impl<Command> SemanticActionRouter<Command> {
    pub fn try_new(
        tree: &SemanticTree,
        routes: impl IntoIterator<Item = ((SemanticNodeId, SemanticAction), Command)>,
    ) -> Result<Self, SemanticError> {
        let mut indexed = BTreeMap::new();
        for (key @ (id, action), command) in routes {
            let Some(node) = tree.node(id) else {
                return Err(SemanticError::new(SemanticErrorCode::StaleNode, Some(id)));
            };
            if !node.actions.contains(&action) || indexed.insert(key, command).is_some() {
                return Err(SemanticError::new(
                    SemanticErrorCode::UnsupportedAction,
                    Some(id),
                ));
            }
        }
        Ok(Self {
            generation: tree.generation,
            routes: indexed,
        })
    }

    pub fn resolve(&self, request: SemanticActionRequest) -> Result<&Command, SemanticError> {
        if request.generation != self.generation {
            return Err(SemanticError::new(
                SemanticErrorCode::StaleGeneration,
                Some(request.node),
            ));
        }
        let value_is_valid = matches!(
            (&request.action, &request.value),
            (SemanticAction::SetValue, Some(SemanticActionValue::Text(_)))
                | (
                    SemanticAction::SetValue,
                    Some(SemanticActionValue::Boolean(_))
                )
                | (
                    SemanticAction::SetValue,
                    Some(SemanticActionValue::Number(_))
                )
                | (
                    SemanticAction::Focus
                        | SemanticAction::Activate
                        | SemanticAction::Increment
                        | SemanticAction::Decrement
                        | SemanticAction::Dismiss
                        | SemanticAction::Cancel,
                    None
                )
        );
        if !value_is_valid {
            return Err(SemanticError::new(
                SemanticErrorCode::UnsupportedAction,
                Some(request.node),
            ));
        }
        self.routes
            .get(&(request.node, request.action))
            .ok_or_else(|| {
                SemanticError::new(SemanticErrorCode::UnsupportedAction, Some(request.node))
            })
    }
}

pub trait AccessibilityBridge {
    fn is_available(&self) -> bool;
    fn apply_patch(&mut self, patch: &SemanticPatch) -> Result<(), SemanticError>;
    fn set_focus(
        &mut self,
        generation: u64,
        target: Option<SemanticNodeId>,
    ) -> Result<(), SemanticError>;
    fn announce(
        &mut self,
        generation: u64,
        node: SemanticNodeId,
        politeness: LiveRegionPoliteness,
    ) -> Result<(), SemanticError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FocusTarget {
    pub id: FocusTargetId,
    pub parent: Option<FocusTargetId>,
    pub order: u32,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusSnapshot {
    pub target: FocusTargetId,
    pub ancestors: Vec<FocusTargetId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FocusState {
    WindowInactive,
    Focused(FocusTargetId),
    Modal {
        owner: FocusTargetId,
        prior: FocusSnapshot,
        current: FocusTargetId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusMove {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusEscape {
    NoModal,
    InvokeSafeAction(FocusTargetId),
}

#[derive(Clone)]
struct ModalFrame {
    owner: FocusTargetId,
    prior: FocusSnapshot,
    current: FocusTargetId,
}

pub struct FocusManager {
    targets: BTreeMap<FocusTargetId, FocusTarget>,
    default: FocusTargetId,
    focused: Option<FocusTargetId>,
    modals: Vec<ModalFrame>,
    active: bool,
}

impl FocusManager {
    pub fn try_new(
        default: FocusTargetId,
        targets: impl IntoIterator<Item = FocusTarget>,
    ) -> Result<Self, FocusError> {
        let targets = index_focus_targets(targets)?;
        if !targets.get(&default).is_some_and(|target| target.enabled) {
            return Err(FocusError::InvalidDefault);
        }
        validate_focus_parents(&targets)?;
        Ok(Self {
            targets,
            default,
            focused: Some(default),
            modals: Vec::new(),
            active: true,
        })
    }

    pub fn state(&self) -> FocusState {
        if !self.active {
            return FocusState::WindowInactive;
        }
        if let Some(modal) = self.modals.last() {
            return FocusState::Modal {
                owner: modal.owner,
                prior: modal.prior.clone(),
                current: modal.current,
            };
        }
        FocusState::Focused(self.focused.unwrap_or(self.default))
    }

    pub fn set_window_active(&mut self, active: bool) {
        self.active = active;
    }

    pub fn focus(&mut self, target: FocusTargetId) -> Result<(), FocusError> {
        if !self.is_focusable(target) || !self.in_active_scope(target) {
            return Err(FocusError::UnavailableTarget);
        }
        self.focused = Some(target);
        if let Some(modal) = self.modals.last_mut() {
            modal.current = target;
        }
        Ok(())
    }

    pub fn open_modal(
        &mut self,
        owner: FocusTargetId,
        initial: FocusTargetId,
    ) -> Result<(), FocusError> {
        if !self.is_focusable(initial) || !self.is_descendant_or_self(initial, owner) {
            return Err(FocusError::UnavailableTarget);
        }
        if let Some(parent_modal) = self.modals.last()
            && !self.is_descendant_or_self(owner, parent_modal.owner)
        {
            return Err(FocusError::ModalScopeViolation);
        }
        let prior_target = self.focused.unwrap_or(self.default);
        let prior = self.snapshot(prior_target);
        self.modals.push(ModalFrame {
            owner,
            prior,
            current: initial,
        });
        self.focused = Some(initial);
        Ok(())
    }

    pub fn close_modal(&mut self, owner: FocusTargetId) -> Result<FocusTargetId, FocusError> {
        let Some(frame) = self.modals.pop() else {
            return Err(FocusError::NoModal);
        };
        if frame.owner != owner {
            self.modals.push(frame);
            return Err(FocusError::ModalScopeViolation);
        }
        let restored = self.restore_target(&frame.prior);
        self.focused = Some(restored);
        if let Some(parent) = self.modals.last_mut() {
            parent.current = restored;
        }
        Ok(restored)
    }

    pub fn escape(&self) -> FocusEscape {
        self.modals.last().map_or(FocusEscape::NoModal, |modal| {
            FocusEscape::InvokeSafeAction(modal.owner)
        })
    }

    pub fn move_focus(&mut self, direction: FocusMove) -> Result<FocusTargetId, FocusError> {
        let scope = self.modals.last().map(|modal| modal.owner);
        let mut ordered = self
            .targets
            .values()
            .filter(|target| {
                target.enabled
                    && scope.is_none_or(|owner| self.is_descendant_or_self(target.id, owner))
            })
            .map(|target| (target.order, target.id))
            .collect::<Vec<_>>();
        ordered.sort_unstable();
        if ordered.is_empty() {
            return Err(FocusError::UnavailableTarget);
        }
        let current = self.focused.unwrap_or(self.default);
        let index = ordered
            .iter()
            .position(|(_, id)| *id == current)
            .unwrap_or(0);
        let next = match direction {
            FocusMove::Forward => (index + 1) % ordered.len(),
            FocusMove::Backward => (index + ordered.len() - 1) % ordered.len(),
        };
        let target = ordered[next].1;
        self.focus(target)?;
        Ok(target)
    }

    pub fn replace_targets(
        &mut self,
        targets: impl IntoIterator<Item = FocusTarget>,
    ) -> Result<FocusTargetId, FocusError> {
        let targets = index_focus_targets(targets)?;
        validate_focus_parents(&targets)?;
        if !targets
            .get(&self.default)
            .is_some_and(|target| target.enabled)
        {
            return Err(FocusError::InvalidDefault);
        }
        self.targets = targets;
        let current = self.focused.unwrap_or(self.default);
        let resolved = if self.is_focusable(current) && self.in_active_scope(current) {
            current
        } else if let Some(modal) = self.modals.last() {
            self.first_in_scope(modal.owner).unwrap_or(self.default)
        } else {
            self.default
        };
        self.focused = Some(resolved);
        if let Some(modal) = self.modals.last_mut() {
            modal.current = resolved;
        }
        Ok(resolved)
    }

    fn snapshot(&self, target: FocusTargetId) -> FocusSnapshot {
        let mut ancestors = Vec::new();
        let mut cursor = self.targets.get(&target).and_then(|item| item.parent);
        while let Some(id) = cursor {
            ancestors.push(id);
            cursor = self.targets.get(&id).and_then(|item| item.parent);
        }
        FocusSnapshot { target, ancestors }
    }

    fn restore_target(&self, snapshot: &FocusSnapshot) -> FocusTargetId {
        std::iter::once(snapshot.target)
            .chain(snapshot.ancestors.iter().copied())
            .find(|target| self.is_focusable(*target) && self.in_active_scope(*target))
            .unwrap_or(self.default)
    }

    fn first_in_scope(&self, owner: FocusTargetId) -> Option<FocusTargetId> {
        self.targets
            .values()
            .filter(|target| target.enabled && self.is_descendant_or_self(target.id, owner))
            .min_by_key(|target| (target.order, target.id))
            .map(|target| target.id)
    }

    fn is_focusable(&self, target: FocusTargetId) -> bool {
        self.targets.get(&target).is_some_and(|item| item.enabled)
    }

    fn in_active_scope(&self, target: FocusTargetId) -> bool {
        self.modals
            .last()
            .is_none_or(|modal| self.is_descendant_or_self(target, modal.owner))
    }

    fn is_descendant_or_self(&self, target: FocusTargetId, owner: FocusTargetId) -> bool {
        let mut cursor = Some(target);
        while let Some(id) = cursor {
            if id == owner {
                return true;
            }
            cursor = self.targets.get(&id).and_then(|item| item.parent);
        }
        false
    }
}

fn index_focus_targets(
    targets: impl IntoIterator<Item = FocusTarget>,
) -> Result<BTreeMap<FocusTargetId, FocusTarget>, FocusError> {
    let mut indexed = BTreeMap::new();
    for target in targets {
        if indexed.insert(target.id, target).is_some() {
            return Err(FocusError::DuplicateTarget);
        }
        if indexed.len() > MAX_SEMANTIC_NODES {
            return Err(FocusError::ResourceLimit);
        }
    }
    Ok(indexed)
}

fn validate_focus_parents(
    targets: &BTreeMap<FocusTargetId, FocusTarget>,
) -> Result<(), FocusError> {
    for target in targets.values() {
        let mut seen = BTreeSet::new();
        let mut cursor = target.parent;
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return Err(FocusError::ParentCycle);
            }
            cursor = Some(targets.get(&id).ok_or(FocusError::MissingParent)?.parent).flatten();
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusError {
    DuplicateTarget,
    MissingParent,
    ParentCycle,
    InvalidDefault,
    UnavailableTarget,
    ModalScopeViolation,
    NoModal,
    ResourceLimit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u64) -> SemanticNodeId {
        SemanticNodeId::new(NonZeroU64::new(value).unwrap())
    }

    fn focus(value: u64) -> FocusTargetId {
        FocusTargetId::new(id(value))
    }

    fn node(value: u64, parent: Option<u64>, role: SemanticRole) -> SemanticNode {
        let mut node = SemanticNode::new(id(value), role);
        node.parent = parent.map(id);
        if role.requires_name() {
            node.name = Some(SemanticText::user_text(format!("node {value}")).unwrap());
        }
        node
    }

    fn basic_tree(generation: u64, button_name: &str) -> SemanticTree {
        let root = node(1, None, SemanticRole::Application);
        let mut button = node(2, Some(1), SemanticRole::Button);
        button.name = Some(SemanticText::user_text(button_name).unwrap());
        button.actions.insert(SemanticAction::Activate);
        SemanticTree::try_new(generation, id(1), [root, button]).unwrap()
    }

    #[test]
    fn semantics_reject_duplicate_cycle_missing_name_relation_and_action() {
        let root = node(1, None, SemanticRole::Application);
        assert_eq!(
            SemanticTree::try_new(1, id(1), [root.clone(), root])
                .unwrap_err()
                .code,
            SemanticErrorCode::DuplicateNode
        );

        let mut cyclic_root = node(1, Some(2), SemanticRole::Application);
        let child = node(2, Some(1), SemanticRole::Group);
        assert_eq!(
            SemanticTree::try_new(1, id(1), [cyclic_root.clone(), child])
                .unwrap_err()
                .code,
            SemanticErrorCode::InvalidRoot
        );
        cyclic_root.parent = None;

        let missing_name = SemanticNode::new(id(2), SemanticRole::Button);
        assert_eq!(
            SemanticTree::try_new(1, id(1), [cyclic_root.clone(), missing_name])
                .unwrap_err()
                .code,
            SemanticErrorCode::MissingName
        );

        let mut invalid_action = node(2, Some(1), SemanticRole::StaticText);
        invalid_action.actions.insert(SemanticAction::Activate);
        assert_eq!(
            SemanticTree::try_new(1, id(1), [cyclic_root, invalid_action])
                .unwrap_err()
                .code,
            SemanticErrorCode::UnsupportedAction
        );
    }

    #[test]
    fn semantics_reject_control_text_and_bound_user_text() {
        assert_eq!(
            SemanticText::user_text("visible\u{1b}[31m")
                .unwrap_err()
                .code,
            SemanticErrorCode::UnsafeText
        );
        assert_eq!(
            SemanticText::user_text("x".repeat(MAX_SEMANTIC_TEXT_CHARS + 1))
                .unwrap_err()
                .code,
            SemanticErrorCode::UnsafeText
        );
    }

    #[test]
    fn deterministic_diff_rejects_same_generation_id_reuse() {
        let mut differ = SemanticDiffer::default();
        let first = basic_tree(9, "Run");
        let first_patch = differ.diff(first).unwrap();
        assert_eq!(first_patch.changes.len(), 2);

        let root_only =
            SemanticTree::try_new(9, id(1), [node(1, None, SemanticRole::Application)]).unwrap();
        assert_eq!(
            differ.diff(root_only).unwrap().changes,
            [SemanticChange::Removed(id(2))]
        );
        assert_eq!(
            differ
                .diff(basic_tree(9, "Different identity"))
                .unwrap_err()
                .code,
            SemanticErrorCode::ReusedNode
        );

        let next_generation = differ.diff(basic_tree(10, "New window")).unwrap();
        assert_eq!(next_generation.changes.len(), 2);
    }

    #[test]
    fn deterministic_diff_rejects_stable_id_identity_changes() {
        let mut differ = SemanticDiffer::default();
        differ.diff(basic_tree(11, "Run")).unwrap();

        let root = node(1, None, SemanticRole::Application);
        let changed_role = node(2, Some(1), SemanticRole::Checkbox);
        let changed = SemanticTree::try_new(11, id(1), [root, changed_role]).unwrap();
        assert_eq!(
            differ.diff(changed).unwrap_err().code,
            SemanticErrorCode::IdentityChanged
        );
    }

    #[test]
    fn action_router_fails_closed_for_stale_generation_and_unsupported_action() {
        let tree = basic_tree(4, "Run");
        let router =
            SemanticActionRouter::try_new(&tree, [((id(2), SemanticAction::Activate), "run")])
                .unwrap();
        assert_eq!(
            router
                .resolve(SemanticActionRequest {
                    generation: 4,
                    node: id(2),
                    action: SemanticAction::Activate,
                    value: None,
                })
                .unwrap(),
            &"run"
        );
        assert_eq!(
            router
                .resolve(SemanticActionRequest {
                    generation: 3,
                    node: id(2),
                    action: SemanticAction::Activate,
                    value: None,
                })
                .unwrap_err()
                .code,
            SemanticErrorCode::StaleGeneration
        );
        assert_eq!(
            router
                .resolve(SemanticActionRequest {
                    generation: 4,
                    node: id(2),
                    action: SemanticAction::Activate,
                    value: Some(SemanticActionValue::Boolean(true)),
                })
                .unwrap_err()
                .code,
            SemanticErrorCode::UnsupportedAction
        );
        assert_eq!(
            SemanticActionValue::text("x".repeat(MAX_SEMANTIC_ACTION_VALUE_CHARS + 1))
                .unwrap_err()
                .code,
            SemanticErrorCode::ResourceLimit
        );
    }

    #[test]
    fn coalescer_bounds_normal_updates_and_never_delays_critical_state() {
        let start = Instant::now();
        let mut coalescer = SemanticUpdateCoalescer::default();
        assert!(
            coalescer
                .submit(basic_tree(1, "one"), SemanticUrgency::Normal, start)
                .is_some()
        );
        assert!(
            coalescer
                .submit(
                    basic_tree(1, "two"),
                    SemanticUrgency::Normal,
                    start + Duration::from_millis(1),
                )
                .is_none()
        );
        assert!(
            coalescer
                .submit(
                    basic_tree(1, "error"),
                    SemanticUrgency::Error,
                    start + Duration::from_millis(2),
                )
                .is_some()
        );
        assert!(coalescer.flush(start + Duration::from_secs(1)).is_none());
    }

    fn focus_targets() -> Vec<FocusTarget> {
        vec![
            FocusTarget {
                id: focus(1),
                parent: None,
                order: 0,
                enabled: true,
            },
            FocusTarget {
                id: focus(2),
                parent: None,
                order: 1,
                enabled: true,
            },
            FocusTarget {
                id: focus(10),
                parent: None,
                order: 10,
                enabled: true,
            },
            FocusTarget {
                id: focus(11),
                parent: Some(focus(10)),
                order: 11,
                enabled: true,
            },
            FocusTarget {
                id: focus(12),
                parent: Some(focus(10)),
                order: 12,
                enabled: true,
            },
        ]
    }

    #[test]
    fn modal_focus_traps_tabs_and_restores_prior_identity() {
        let mut manager = FocusManager::try_new(focus(1), focus_targets()).unwrap();
        manager.focus(focus(2)).unwrap();
        manager.open_modal(focus(10), focus(11)).unwrap();
        assert_eq!(manager.move_focus(FocusMove::Forward).unwrap(), focus(12));
        assert_eq!(manager.move_focus(FocusMove::Forward).unwrap(), focus(10));
        assert_eq!(manager.escape(), FocusEscape::InvokeSafeAction(focus(10)));
        assert_eq!(manager.close_modal(focus(10)).unwrap(), focus(2));
        assert_eq!(manager.state(), FocusState::Focused(focus(2)));
    }

    #[test]
    fn modal_close_restores_nearest_live_ancestor_then_default() {
        let mut targets = focus_targets();
        targets.push(FocusTarget {
            id: focus(3),
            parent: Some(focus(2)),
            order: 2,
            enabled: true,
        });
        let mut manager = FocusManager::try_new(focus(1), targets.clone()).unwrap();
        manager.focus(focus(3)).unwrap();
        manager.open_modal(focus(10), focus(11)).unwrap();
        targets.retain(|target| target.id != focus(3));
        manager.replace_targets(targets).unwrap();
        assert_eq!(manager.close_modal(focus(10)).unwrap(), focus(2));

        manager.open_modal(focus(10), focus(11)).unwrap();
        let only_default_and_modal = focus_targets()
            .into_iter()
            .filter(|target| target.id != focus(2))
            .collect::<Vec<_>>();
        manager.replace_targets(only_default_and_modal).unwrap();
        assert_eq!(manager.close_modal(focus(10)).unwrap(), focus(1));
    }

    #[test]
    fn rerender_preserves_focus_identity_despite_order_change() {
        let mut manager = FocusManager::try_new(focus(1), focus_targets()).unwrap();
        manager.focus(focus(2)).unwrap();
        let changed = focus_targets()
            .into_iter()
            .map(|mut target| {
                target.order = 100_u32.saturating_sub(target.order);
                target
            })
            .collect::<Vec<_>>();
        assert_eq!(manager.replace_targets(changed).unwrap(), focus(2));
        manager.set_window_active(false);
        assert_eq!(manager.state(), FocusState::WindowInactive);
    }
}
