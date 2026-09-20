use std::collections::BTreeSet;
use std::num::NonZeroU64;

use crate::{
    FocusManager, FocusMove, FocusTarget, FocusTargetId, Locale, MessageId, SemanticAction,
    SemanticActionRequest, SemanticActionRouter, SemanticActionValue, SemanticBounds,
    SemanticError, SemanticNode, SemanticNodeId, SemanticRelation, SemanticRelationKind,
    SemanticRole, SemanticText, SemanticTree, SemanticValue, ThemeKind,
};

const ROOT: u64 = 1;
const SKIP: u64 = 2;
const LANDMARK: u64 = 3;
const HEADING: u64 = 4;
const LIST: u64 = 5;
const LIST_FIRST: u64 = 6;
const LIST_SECOND: u64 = 7;
const FIELD_LABEL: u64 = 8;
const FIELD: u64 = 9;
const FIELD_HELP: u64 = 10;
const FIELD_ERROR: u64 = 11;
const MENU: u64 = 12;
const MENU_ITEM: u64 = 13;
const PROGRESS: u64 = 14;
const STATUS: u64 = 15;
const DISABLED: u64 = 16;
const DESTRUCTIVE: u64 = 17;
const DIALOG: u64 = 20;
const SAFE_DEFAULT: u64 = 21;
const CONFIRM: u64 = 22;
const SECRET_CANARY: &str = "TERMIRUST_AX_SECRET_CANARY_7bd50a";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessibilityLabNode {
    Skip,
    ListFirst,
    ListSecond,
    Field,
    Menu,
    MenuItem,
    Progress,
    Status,
    Disabled,
    Destructive,
    Dialog,
    SafeDefault,
    Confirm,
}

impl AccessibilityLabNode {
    pub const fn semantic_id(self) -> SemanticNodeId {
        id_const(match self {
            Self::Skip => SKIP,
            Self::ListFirst => LIST_FIRST,
            Self::ListSecond => LIST_SECOND,
            Self::Field => FIELD,
            Self::Menu => MENU,
            Self::MenuItem => MENU_ITEM,
            Self::Progress => PROGRESS,
            Self::Status => STATUS,
            Self::Disabled => DISABLED,
            Self::Destructive => DESTRUCTIVE,
            Self::Dialog => DIALOG,
            Self::SafeDefault => SAFE_DEFAULT,
            Self::Confirm => CONFIRM,
        })
    }
}

fn id(value: u64) -> SemanticNodeId {
    SemanticNodeId::new(NonZeroU64::new(value).expect("laboratory IDs are nonzero"))
}

fn focus(value: u64) -> FocusTargetId {
    FocusTargetId::new(id(value))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessibilityLabConfiguration {
    pub locale: Locale,
    pub theme: ThemeKind,
    pub text_scale_percent: u16,
    pub reduced_motion: bool,
}

impl Default for AccessibilityLabConfiguration {
    fn default() -> Self {
        Self {
            locale: Locale::EnUs,
            theme: ThemeKind::Dark,
            text_scale_percent: 100,
            reduced_motion: false,
        }
    }
}

impl AccessibilityLabConfiguration {
    pub fn validate(self) -> Result<Self, SemanticError> {
        if !(100..=200).contains(&self.text_scale_percent) {
            return Err(SemanticError::new(
                crate::SemanticErrorCode::InvalidValue,
                None,
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessibilityLabCommand {
    FocusTarget(SemanticNodeId),
    SkipToControls,
    SelectFirst,
    SelectSecond,
    SetField,
    CloseMenu,
    AnnounceStatus,
    IncrementProgress,
    DecrementProgress,
    CancelProgress,
    OpenConfirmation,
    CloseConfirmation,
    ConfirmReferenceAction,
}

pub struct AccessibilityLabModel {
    configuration: AccessibilityLabConfiguration,
    generation: u64,
    selected_second: bool,
    field_value: String,
    field_invalid: bool,
    dialog_open: bool,
    progress: i64,
    focus: FocusManager,
    sensitive_fixture: String,
}

impl AccessibilityLabModel {
    pub fn try_new(configuration: AccessibilityLabConfiguration) -> Result<Self, SemanticError> {
        let configuration = configuration.validate()?;
        let focus = FocusManager::try_new(focus(SKIP), focus_targets(false))
            .map_err(|_| SemanticError::new(crate::SemanticErrorCode::InvalidValue, None))?;
        Ok(Self {
            configuration,
            generation: 1,
            selected_second: false,
            field_value: String::new(),
            field_invalid: true,
            dialog_open: false,
            progress: 40,
            focus,
            sensitive_fixture: SECRET_CANARY.to_string(),
        })
    }

    pub const fn configuration(&self) -> AccessibilityLabConfiguration {
        self.configuration
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn dialog_open(&self) -> bool {
        self.dialog_open
    }

    pub const fn progress(&self) -> i64 {
        self.progress
    }

    pub fn field_value(&self) -> &str {
        &self.field_value
    }

    pub fn focus_state(&self) -> crate::FocusState {
        self.focus.state()
    }

    pub fn tree(&self) -> Result<SemanticTree, SemanticError> {
        let scale = u32::from(self.configuration.text_scale_percent);
        let mut nodes = vec![
            node(ROOT, None, SemanticRole::Application, None, 0, scale),
            action_node(
                SKIP,
                Some(ROOT),
                SemanticRole::Button,
                MessageId::AccessibilityLabSkipAction,
                [SemanticAction::Activate, SemanticAction::Focus],
                1,
                scale,
            ),
            named_node(
                LANDMARK,
                Some(ROOT),
                SemanticRole::Landmark,
                MessageId::AccessibilityLabLandmark,
                2,
                scale,
            ),
            named_node(
                HEADING,
                Some(LANDMARK),
                SemanticRole::Heading,
                MessageId::AccessibilityLabTitle,
                3,
                scale,
            ),
            named_node(
                LIST,
                Some(LANDMARK),
                SemanticRole::List,
                MessageId::AccessibilityLabList,
                4,
                scale,
            ),
            action_node(
                LIST_FIRST,
                Some(LIST),
                SemanticRole::ListItem,
                MessageId::AccessibilityLabListFirst,
                [SemanticAction::Activate, SemanticAction::Focus],
                5,
                scale,
            ),
            action_node(
                LIST_SECOND,
                Some(LIST),
                SemanticRole::ListItem,
                MessageId::AccessibilityLabListSecond,
                [SemanticAction::Activate, SemanticAction::Focus],
                6,
                scale,
            ),
            named_node(
                FIELD_LABEL,
                Some(LANDMARK),
                SemanticRole::StaticText,
                MessageId::AccessibilityLabField,
                7,
                scale,
            ),
            action_node(
                FIELD,
                Some(LANDMARK),
                SemanticRole::TextField,
                MessageId::AccessibilityLabField,
                [SemanticAction::Focus, SemanticAction::SetValue],
                8,
                scale,
            ),
            named_node(
                FIELD_HELP,
                Some(LANDMARK),
                SemanticRole::StaticText,
                MessageId::AccessibilityLabFieldHelp,
                9,
                scale,
            ),
            named_node(
                FIELD_ERROR,
                Some(LANDMARK),
                SemanticRole::Alert,
                MessageId::AccessibilityLabFieldError,
                10,
                scale,
            ),
            action_node(
                MENU,
                Some(LANDMARK),
                SemanticRole::Menu,
                MessageId::AccessibilityLabMenu,
                [SemanticAction::Cancel],
                11,
                scale,
            ),
            action_node(
                MENU_ITEM,
                Some(MENU),
                SemanticRole::MenuItem,
                MessageId::AccessibilityLabMenuItem,
                [SemanticAction::Activate, SemanticAction::Focus],
                12,
                scale,
            ),
            action_node(
                PROGRESS,
                Some(LANDMARK),
                SemanticRole::ProgressIndicator,
                MessageId::AccessibilityLabProgress,
                [
                    SemanticAction::Focus,
                    SemanticAction::Increment,
                    SemanticAction::Decrement,
                    SemanticAction::Cancel,
                ],
                13,
                scale,
            ),
            named_node(
                STATUS,
                Some(LANDMARK),
                SemanticRole::Status,
                MessageId::AccessibilityLabStatusReady,
                14,
                scale,
            ),
            named_node(
                DISABLED,
                Some(LANDMARK),
                SemanticRole::Button,
                MessageId::AccessibilityLabDisabledAction,
                15,
                scale,
            ),
            action_node(
                DESTRUCTIVE,
                Some(LANDMARK),
                SemanticRole::Button,
                MessageId::AccessibilityLabDestructiveAction,
                [SemanticAction::Activate, SemanticAction::Focus],
                16,
                scale,
            ),
        ];

        nodes[5].state.selected = !self.selected_second;
        nodes[6].state.selected = self.selected_second;
        nodes[8].value = Some(SemanticValue::public_user_text(self.field_value.clone())?);
        nodes[8].state.invalid = self.field_invalid;
        nodes[8].relations = vec![
            relation(SemanticRelationKind::LabelledBy, FIELD_LABEL),
            relation(SemanticRelationKind::DescribedBy, FIELD_HELP),
            relation(SemanticRelationKind::ErrorMessage, FIELD_ERROR),
        ];
        nodes[10].state.hidden = !self.field_invalid;
        nodes[13].value = Some(SemanticValue::Number {
            current: self.progress,
            minimum: 0,
            maximum: 100,
        });
        nodes[14].state.live = Some(crate::LiveRegionPoliteness::Polite);
        nodes[15].state.disabled = true;
        nodes[15].description = Some(SemanticText::Message(
            MessageId::AccessibilityLabDisabledReason,
        ));
        nodes[1]
            .relations
            .push(relation(SemanticRelationKind::Controls, LANDMARK));

        let mut dialog = action_node(
            DIALOG,
            Some(ROOT),
            SemanticRole::Dialog,
            MessageId::AccessibilityLabDestructiveConfirm,
            [SemanticAction::Cancel, SemanticAction::Dismiss],
            20,
            scale,
        );
        dialog.description = Some(SemanticText::Message(
            MessageId::AccessibilityLabDialogDescription,
        ));
        dialog.state.hidden = !self.dialog_open;
        let mut safe_default = action_node(
            SAFE_DEFAULT,
            Some(DIALOG),
            SemanticRole::Button,
            MessageId::AccessibilityLabSafeDefault,
            [SemanticAction::Activate, SemanticAction::Focus],
            21,
            scale,
        );
        safe_default.state.hidden = !self.dialog_open;
        let mut confirm = action_node(
            CONFIRM,
            Some(DIALOG),
            SemanticRole::Button,
            MessageId::AccessibilityLabConfirmAction,
            [SemanticAction::Activate, SemanticAction::Focus],
            22,
            scale,
        );
        confirm.state.hidden = !self.dialog_open;
        nodes.extend([dialog, safe_default, confirm]);

        SemanticTree::try_new(self.generation, id(ROOT), nodes)
    }

    pub fn execute(
        &mut self,
        request: SemanticActionRequest,
    ) -> Result<AccessibilityLabCommand, SemanticError> {
        let tree = self.tree()?;
        let router = SemanticActionRouter::try_new(&tree, routes(self.dialog_open))?;
        let command = *router.resolve(request.clone())?;
        match command {
            AccessibilityLabCommand::FocusTarget(target) => {
                self.focus
                    .focus(FocusTargetId::new(target))
                    .map_err(focus_error)?;
            }
            AccessibilityLabCommand::SkipToControls => {
                self.focus_target(FIELD)?;
            }
            AccessibilityLabCommand::SelectFirst => {
                self.selected_second = false;
                self.focus_target(LIST_FIRST)?;
            }
            AccessibilityLabCommand::SelectSecond => {
                self.selected_second = true;
                self.focus_target(LIST_SECOND)?;
            }
            AccessibilityLabCommand::SetField => {
                let Some(SemanticActionValue::Text(value)) = request.value else {
                    return Err(SemanticError::new(
                        crate::SemanticErrorCode::InvalidValue,
                        Some(request.node),
                    ));
                };
                SemanticValue::public_user_text(value.clone())?;
                self.field_invalid = value.trim().is_empty();
                self.field_value = value;
            }
            AccessibilityLabCommand::CloseMenu | AccessibilityLabCommand::AnnounceStatus => {}
            AccessibilityLabCommand::IncrementProgress => {
                self.progress = (self.progress + 10).min(100);
            }
            AccessibilityLabCommand::DecrementProgress => {
                self.progress = (self.progress - 10).max(0);
            }
            AccessibilityLabCommand::CancelProgress => self.progress = 0,
            AccessibilityLabCommand::OpenConfirmation => {
                self.dialog_open = true;
                self.focus
                    .replace_targets(focus_targets(true))
                    .map_err(focus_error)?;
                self.focus
                    .open_modal(focus(DIALOG), focus(SAFE_DEFAULT))
                    .map_err(focus_error)?;
            }
            AccessibilityLabCommand::CloseConfirmation
            | AccessibilityLabCommand::ConfirmReferenceAction => {
                self.focus.close_modal(focus(DIALOG)).map_err(focus_error)?;
                self.dialog_open = false;
                self.focus
                    .replace_targets(focus_targets(false))
                    .map_err(focus_error)?;
            }
        }
        Ok(command)
    }

    pub fn move_focus(&mut self, direction: FocusMove) -> Result<FocusTargetId, SemanticError> {
        self.focus.move_focus(direction).map_err(focus_error)
    }

    pub fn semantic_snapshot(&self) -> Result<String, SemanticError> {
        let tree = self.tree()?;
        let mut output = String::new();
        for node in tree.nodes().values() {
            let relations = node
                .relations
                .iter()
                .map(|relation| {
                    format!("{}:{}", relation_name(relation.kind), relation.target.get())
                })
                .collect::<Vec<_>>()
                .join(",");
            let actions = node
                .actions
                .iter()
                .map(|action| action_name(*action))
                .collect::<Vec<_>>()
                .join(",");
            output.push_str(&format!(
                "{} parent={} role={} disabled={} selected={} invalid={} hidden={} relations=[{}] actions=[{}]\n",
                node.id.get(),
                node.parent.map_or(0, SemanticNodeId::get),
                role_name(node.role),
                node.state.disabled,
                node.state.selected,
                node.state.invalid,
                node.state.hidden,
                relations,
                actions,
            ));
        }
        Ok(output)
    }

    pub fn secret_canary(&self) -> &str {
        &self.sensitive_fixture
    }

    fn focus_target(&mut self, value: u64) -> Result<(), SemanticError> {
        self.focus.focus(focus(value)).map_err(focus_error)
    }
}

fn node(
    value: u64,
    parent: Option<u64>,
    role: SemanticRole,
    name: Option<MessageId>,
    row: u32,
    scale: u32,
) -> SemanticNode {
    let mut node = SemanticNode::new(id(value), role);
    node.parent = parent.map(id);
    node.name = name.map(SemanticText::Message);
    node.bounds = SemanticBounds {
        x: scaled(24, scale) as i32,
        y: scaled(24 + row * 44, scale) as i32,
        width: scaled(560, scale),
        height: scaled(36, scale),
    };
    node
}

fn named_node(
    value: u64,
    parent: Option<u64>,
    role: SemanticRole,
    name: MessageId,
    row: u32,
    scale: u32,
) -> SemanticNode {
    node(value, parent, role, Some(name), row, scale)
}

fn action_node<const N: usize>(
    value: u64,
    parent: Option<u64>,
    role: SemanticRole,
    name: MessageId,
    actions: [SemanticAction; N],
    row: u32,
    scale: u32,
) -> SemanticNode {
    let mut node = named_node(value, parent, role, name, row, scale);
    node.actions = BTreeSet::from(actions);
    node
}

const fn scaled(value: u32, scale: u32) -> u32 {
    value.saturating_mul(scale) / 100
}

const fn relation(kind: SemanticRelationKind, target: u64) -> SemanticRelation {
    SemanticRelation {
        kind,
        target: SemanticNodeId::new(NonZeroU64::new(target).expect("relation ID is nonzero")),
    }
}

fn focus_targets(dialog_open: bool) -> Vec<FocusTarget> {
    let mut targets = [
        SKIP,
        LIST_FIRST,
        LIST_SECOND,
        FIELD,
        MENU_ITEM,
        PROGRESS,
        DESTRUCTIVE,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, value)| FocusTarget {
        id: focus(value),
        parent: None,
        order: index as u32,
        enabled: true,
    })
    .collect::<Vec<_>>();
    if dialog_open {
        targets.extend([
            FocusTarget {
                id: focus(DIALOG),
                parent: None,
                order: 20,
                enabled: true,
            },
            FocusTarget {
                id: focus(SAFE_DEFAULT),
                parent: Some(focus(DIALOG)),
                order: 21,
                enabled: true,
            },
            FocusTarget {
                id: focus(CONFIRM),
                parent: Some(focus(DIALOG)),
                order: 22,
                enabled: true,
            },
        ]);
    }
    targets
}

fn routes(dialog_open: bool) -> Vec<((SemanticNodeId, SemanticAction), AccessibilityLabCommand)> {
    let mut routes = vec![
        route(
            SKIP,
            SemanticAction::Activate,
            AccessibilityLabCommand::SkipToControls,
        ),
        focus_route(SKIP),
        route(
            LIST_FIRST,
            SemanticAction::Activate,
            AccessibilityLabCommand::SelectFirst,
        ),
        focus_route(LIST_FIRST),
        route(
            LIST_SECOND,
            SemanticAction::Activate,
            AccessibilityLabCommand::SelectSecond,
        ),
        focus_route(LIST_SECOND),
        focus_route(FIELD),
        route(
            FIELD,
            SemanticAction::SetValue,
            AccessibilityLabCommand::SetField,
        ),
        route(
            MENU,
            SemanticAction::Cancel,
            AccessibilityLabCommand::CloseMenu,
        ),
        route(
            MENU_ITEM,
            SemanticAction::Activate,
            AccessibilityLabCommand::AnnounceStatus,
        ),
        focus_route(MENU_ITEM),
        focus_route(PROGRESS),
        route(
            PROGRESS,
            SemanticAction::Increment,
            AccessibilityLabCommand::IncrementProgress,
        ),
        route(
            PROGRESS,
            SemanticAction::Decrement,
            AccessibilityLabCommand::DecrementProgress,
        ),
        route(
            PROGRESS,
            SemanticAction::Cancel,
            AccessibilityLabCommand::CancelProgress,
        ),
        route(
            DESTRUCTIVE,
            SemanticAction::Activate,
            AccessibilityLabCommand::OpenConfirmation,
        ),
        focus_route(DESTRUCTIVE),
    ];
    if dialog_open {
        routes.extend([
            route(
                DIALOG,
                SemanticAction::Cancel,
                AccessibilityLabCommand::CloseConfirmation,
            ),
            route(
                DIALOG,
                SemanticAction::Dismiss,
                AccessibilityLabCommand::CloseConfirmation,
            ),
            route(
                SAFE_DEFAULT,
                SemanticAction::Activate,
                AccessibilityLabCommand::CloseConfirmation,
            ),
            focus_route(SAFE_DEFAULT),
            route(
                CONFIRM,
                SemanticAction::Activate,
                AccessibilityLabCommand::ConfirmReferenceAction,
            ),
            focus_route(CONFIRM),
        ]);
    }
    routes
}

const fn route(
    node: u64,
    action: SemanticAction,
    command: AccessibilityLabCommand,
) -> ((SemanticNodeId, SemanticAction), AccessibilityLabCommand) {
    ((id_const(node), action), command)
}

const fn focus_route(node: u64) -> ((SemanticNodeId, SemanticAction), AccessibilityLabCommand) {
    (
        (id_const(node), SemanticAction::Focus),
        AccessibilityLabCommand::FocusTarget(id_const(node)),
    )
}

const fn id_const(value: u64) -> SemanticNodeId {
    SemanticNodeId::new(NonZeroU64::new(value).expect("laboratory ID is nonzero"))
}

fn focus_error(_: crate::FocusError) -> SemanticError {
    SemanticError::new(crate::SemanticErrorCode::InvalidValue, None)
}

const fn role_name(role: SemanticRole) -> &'static str {
    match role {
        SemanticRole::Application => "application",
        SemanticRole::Landmark => "landmark",
        SemanticRole::Heading => "heading",
        SemanticRole::List => "list",
        SemanticRole::ListItem => "list_item",
        SemanticRole::Button => "button",
        SemanticRole::TextField => "text_field",
        SemanticRole::StaticText => "static_text",
        SemanticRole::Menu => "menu",
        SemanticRole::MenuItem => "menu_item",
        SemanticRole::Dialog => "dialog",
        SemanticRole::ProgressIndicator => "progress",
        SemanticRole::Status => "status",
        SemanticRole::Alert => "alert",
        SemanticRole::Checkbox => "checkbox",
        SemanticRole::RadioButton => "radio_button",
        SemanticRole::Tab => "tab",
        SemanticRole::TabList => "tab_list",
        SemanticRole::Group => "group",
    }
}

const fn action_name(action: SemanticAction) -> &'static str {
    match action {
        SemanticAction::Focus => "focus",
        SemanticAction::Activate => "activate",
        SemanticAction::SetValue => "set_value",
        SemanticAction::Increment => "increment",
        SemanticAction::Decrement => "decrement",
        SemanticAction::Dismiss => "dismiss",
        SemanticAction::Cancel => "cancel",
    }
}

const fn relation_name(kind: SemanticRelationKind) -> &'static str {
    match kind {
        SemanticRelationKind::LabelledBy => "labelled_by",
        SemanticRelationKind::DescribedBy => "described_by",
        SemanticRelationKind::Controls => "controls",
        SemanticRelationKind::Owns => "owns",
        SemanticRelationKind::ErrorMessage => "error_message",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FocusState, LiveRegionPoliteness, Localizer};

    #[test]
    fn laboratory_tree_covers_required_semantics_without_secret_disclosure() {
        let model = AccessibilityLabModel::try_new(Default::default()).unwrap();
        let tree = model.tree().unwrap();
        let roles = tree
            .nodes()
            .values()
            .map(|node| node.role)
            .collect::<BTreeSet<_>>();
        for role in [
            SemanticRole::Application,
            SemanticRole::Landmark,
            SemanticRole::Heading,
            SemanticRole::List,
            SemanticRole::ListItem,
            SemanticRole::TextField,
            SemanticRole::Menu,
            SemanticRole::MenuItem,
            SemanticRole::ProgressIndicator,
            SemanticRole::Status,
            SemanticRole::Alert,
            SemanticRole::Button,
        ] {
            assert!(roles.contains(&role), "missing {role:?}");
        }
        assert_eq!(
            tree.node(id(STATUS)).unwrap().state.live,
            Some(LiveRegionPoliteness::Polite)
        );
        assert!(tree.node(id(DISABLED)).unwrap().state.disabled);
        let snapshot = model.semantic_snapshot().unwrap();
        assert!(!snapshot.contains(model.secret_canary()));
        assert!(!snapshot.contains("Accessibility laboratory"));
    }

    #[test]
    fn laboratory_actions_validate_values_and_restore_modal_focus() {
        let mut model = AccessibilityLabModel::try_new(Default::default()).unwrap();
        model
            .execute(SemanticActionRequest {
                generation: 1,
                node: id(LIST_SECOND),
                action: SemanticAction::Activate,
                value: None,
            })
            .unwrap();
        model
            .execute(SemanticActionRequest {
                generation: 1,
                node: id(DESTRUCTIVE),
                action: SemanticAction::Activate,
                value: None,
            })
            .unwrap();
        assert!(matches!(model.focus_state(), FocusState::Modal { .. }));
        assert!(model.dialog_open());
        model.move_focus(FocusMove::Forward).unwrap();
        model
            .execute(SemanticActionRequest {
                generation: 1,
                node: id(DIALOG),
                action: SemanticAction::Cancel,
                value: None,
            })
            .unwrap();
        assert_eq!(model.focus_state(), FocusState::Focused(focus(LIST_SECOND)));

        let error = model
            .execute(SemanticActionRequest {
                generation: 2,
                node: id(FIELD),
                action: SemanticAction::SetValue,
                value: Some(SemanticActionValue::Text("label".to_string())),
            })
            .unwrap_err();
        assert_eq!(error.code, crate::SemanticErrorCode::StaleGeneration);

        model
            .execute(SemanticActionRequest {
                generation: 1,
                node: id(DESTRUCTIVE),
                action: SemanticAction::Activate,
                value: None,
            })
            .unwrap();
        model
            .execute(SemanticActionRequest {
                generation: 1,
                node: id(SAFE_DEFAULT),
                action: SemanticAction::Activate,
                value: None,
            })
            .unwrap();
        model
            .execute(SemanticActionRequest {
                generation: 1,
                node: id(DESTRUCTIVE),
                action: SemanticAction::Activate,
                value: None,
            })
            .unwrap();
        assert!(model.dialog_open());
    }

    #[test]
    fn laboratory_locales_themes_scale_and_motion_are_bounded() {
        for locale in Locale::ALL {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            for message in [
                MessageId::AccessibilityLabTitle,
                MessageId::AccessibilityLabField,
                MessageId::AccessibilityLabDialogDescription,
                MessageId::AccessibilityLabSafeDefault,
            ] {
                assert!(!localizer.format_static(message).unwrap().is_empty());
            }
            for theme in [
                ThemeKind::Light,
                ThemeKind::Dark,
                ThemeKind::HighContrast,
                ThemeKind::RecordingFriendly,
            ] {
                for text_scale_percent in [100, 200] {
                    for reduced_motion in [false, true] {
                        let model = AccessibilityLabModel::try_new(AccessibilityLabConfiguration {
                            locale,
                            theme,
                            text_scale_percent,
                            reduced_motion,
                        })
                        .unwrap();
                        let field = model.tree().unwrap().node(id(FIELD)).unwrap().clone();
                        assert_eq!(
                            field.bounds.width,
                            560 * u32::from(text_scale_percent) / 100
                        );
                    }
                }
            }
        }
        assert!(
            Localizer::english()
                .format_static(MessageId::StatusConnecting)
                .is_err()
        );
    }

    #[test]
    fn semantic_number_values_reject_invalid_ranges() {
        let mut root = SemanticNode::new(id(ROOT), SemanticRole::Application);
        root.value = Some(SemanticValue::Number {
            current: 101,
            minimum: 0,
            maximum: 100,
        });
        assert_eq!(
            SemanticTree::try_new(1, id(ROOT), [root]).unwrap_err().code,
            crate::SemanticErrorCode::InvalidValue
        );
    }
}
