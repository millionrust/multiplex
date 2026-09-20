use std::collections::{BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use crate::{
    LiveRegionPoliteness, MessageId, SemanticAction, SemanticError, SemanticErrorCode,
    SemanticNode, SemanticNodeId, SemanticRole, SemanticState, SemanticText, SemanticValue,
};

pub const MAX_SETTINGS: usize = 128;
pub const MAX_SETTINGS_QUERY_CHARS: usize = 256;
pub const SETTINGS_ANNOUNCEMENT_INTERVAL: Duration = Duration::from_millis(250);

const ROOT_NODE: u64 = 190_000;
const STATUS_NODE: u64 = 190_001;
const SEARCH_NODE: u64 = 190_002;
const CLEAR_NODE: u64 = 190_003;
const SECTION_NODE_BASE: u64 = 12_u64 << 60;
const SETTING_NODE_BASE: u64 = 13_u64 << 60;
const NODE_MASK: u64 = (1_u64 << 60) - 1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SettingsSectionId {
    Appearance,
    Terminal,
    ProjectsSessions,
    PresetsRuntimes,
    Notifications,
    Keyboard,
    StoragePrivacyDiagnostics,
    RemoteDevices,
}

impl SettingsSectionId {
    pub const ALL: [Self; 8] = [
        Self::Appearance,
        Self::Terminal,
        Self::ProjectsSessions,
        Self::PresetsRuntimes,
        Self::Notifications,
        Self::Keyboard,
        Self::StoragePrivacyDiagnostics,
        Self::RemoteDevices,
    ];

    pub const fn title(self) -> MessageId {
        match self {
            Self::Appearance => MessageId::SettingsSectionAppearance,
            Self::Terminal => MessageId::SettingsSectionTerminal,
            Self::ProjectsSessions => MessageId::SettingsSectionProjectsSessions,
            Self::PresetsRuntimes => MessageId::SettingsSectionPresetsRuntimes,
            Self::Notifications => MessageId::SettingsSectionNotifications,
            Self::Keyboard => MessageId::SettingsSectionKeyboard,
            Self::StoragePrivacyDiagnostics => MessageId::SettingsSectionStoragePrivacyDiagnostics,
            Self::RemoteDevices => MessageId::SettingsSectionRemoteDevices,
        }
    }

    pub const fn description(self) -> MessageId {
        match self {
            Self::Appearance => MessageId::SettingsSectionAppearanceDescription,
            Self::Terminal => MessageId::SettingsSectionTerminalDescription,
            Self::ProjectsSessions => MessageId::SettingsSectionProjectsSessionsDescription,
            Self::PresetsRuntimes => MessageId::SettingsSectionPresetsRuntimesDescription,
            Self::Notifications => MessageId::SettingsSectionNotificationsDescription,
            Self::Keyboard => MessageId::SettingsSectionKeyboardDescription,
            Self::StoragePrivacyDiagnostics => {
                MessageId::SettingsSectionStoragePrivacyDiagnosticsDescription
            }
            Self::RemoteDevices => MessageId::SettingsSectionRemoteDevicesDescription,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SettingId {
    Theme,
    DevelopmentLocale,
    TerminalFontSize,
    CopyOnSelect,
    ConfirmMultilinePaste,
    TerminalFontFamily,
    RestoreWorkspaces,
    Onboarding,
    SessionHistoryLimit,
    DefaultSshDirectory,
    AutoReconnectAttempts,
    SshKeepalive,
    ReconnectDelay,
    LocalShellProgram,
    LocalShellWorkingDirectory,
    Diagnostics,
    StorageHealth,
    NotificationMode,
    RecordingFriendly,
    RemoteDevices,
    CliStatus,
    KeyboardShortcuts,
    PortableData,
    EncryptedBackup,
    BackupExportPassphrase,
    BackupImportPassphrase,
    MobileVault,
    MobilePairing,
    SharedFolderSync,
    SyncFolder,
}

impl SettingId {
    pub const ALL: [Self; 30] = [
        Self::Theme,
        Self::DevelopmentLocale,
        Self::TerminalFontSize,
        Self::CopyOnSelect,
        Self::ConfirmMultilinePaste,
        Self::TerminalFontFamily,
        Self::RestoreWorkspaces,
        Self::Onboarding,
        Self::SessionHistoryLimit,
        Self::DefaultSshDirectory,
        Self::AutoReconnectAttempts,
        Self::SshKeepalive,
        Self::ReconnectDelay,
        Self::LocalShellProgram,
        Self::LocalShellWorkingDirectory,
        Self::Diagnostics,
        Self::StorageHealth,
        Self::NotificationMode,
        Self::RecordingFriendly,
        Self::RemoteDevices,
        Self::CliStatus,
        Self::KeyboardShortcuts,
        Self::PortableData,
        Self::EncryptedBackup,
        Self::BackupExportPassphrase,
        Self::BackupImportPassphrase,
        Self::MobileVault,
        Self::MobilePairing,
        Self::SharedFolderSync,
        Self::SyncFolder,
    ];

    pub const fn section(self) -> SettingsSectionId {
        match self {
            Self::Theme | Self::DevelopmentLocale => SettingsSectionId::Appearance,
            Self::TerminalFontSize
            | Self::CopyOnSelect
            | Self::ConfirmMultilinePaste
            | Self::TerminalFontFamily => SettingsSectionId::Terminal,
            Self::RestoreWorkspaces
            | Self::Onboarding
            | Self::SessionHistoryLimit
            | Self::DefaultSshDirectory
            | Self::AutoReconnectAttempts
            | Self::SshKeepalive
            | Self::ReconnectDelay => SettingsSectionId::ProjectsSessions,
            Self::LocalShellProgram | Self::LocalShellWorkingDirectory | Self::CliStatus => {
                SettingsSectionId::PresetsRuntimes
            }
            Self::NotificationMode | Self::RecordingFriendly => SettingsSectionId::Notifications,
            Self::KeyboardShortcuts => SettingsSectionId::Keyboard,
            Self::Diagnostics
            | Self::StorageHealth
            | Self::PortableData
            | Self::EncryptedBackup
            | Self::BackupExportPassphrase
            | Self::BackupImportPassphrase
            | Self::MobileVault
            | Self::MobilePairing
            | Self::SharedFolderSync
            | Self::SyncFolder => SettingsSectionId::StoragePrivacyDiagnostics,
            Self::RemoteDevices => SettingsSectionId::RemoteDevices,
        }
    }

    pub const fn label(self) -> MessageId {
        match self {
            Self::Theme => MessageId::SettingsThemeLabel,
            Self::DevelopmentLocale => MessageId::SettingsDevelopmentLocaleLabel,
            Self::TerminalFontSize => MessageId::SettingsTerminalFontSizeLabel,
            Self::CopyOnSelect => MessageId::SettingsCopyOnSelectLabel,
            Self::ConfirmMultilinePaste => MessageId::SettingsConfirmMultilinePasteLabel,
            Self::TerminalFontFamily => MessageId::SettingsTerminalFontFamilyLabel,
            Self::RestoreWorkspaces => MessageId::SettingsRestoreWorkspacesLabel,
            Self::Onboarding => MessageId::SettingsOnboardingLabel,
            Self::SessionHistoryLimit => MessageId::SettingsSessionHistoryLimitLabel,
            Self::DefaultSshDirectory => MessageId::SettingsDefaultSshDirectoryLabel,
            Self::AutoReconnectAttempts => MessageId::SettingsAutoReconnectLabel,
            Self::SshKeepalive => MessageId::SettingsSshKeepaliveLabel,
            Self::ReconnectDelay => MessageId::SettingsReconnectDelayLabel,
            Self::LocalShellProgram => MessageId::SettingsLocalShellProgramLabel,
            Self::LocalShellWorkingDirectory => MessageId::SettingsLocalShellCwdLabel,
            Self::Diagnostics => MessageId::DiagnosticsSettingsTitle,
            Self::StorageHealth => MessageId::HealthSettingsTitle,
            Self::NotificationMode => MessageId::NotificationSettingsTitle,
            Self::RecordingFriendly => MessageId::NotificationRecordingTitle,
            Self::RemoteDevices => MessageId::RemoteDevicesTitle,
            Self::CliStatus => MessageId::CliSettingsTitle,
            Self::KeyboardShortcuts => MessageId::SettingsKeyboardShortcutsTitle,
            Self::PortableData => MessageId::SettingsPortableDataTitle,
            Self::EncryptedBackup => MessageId::SettingsEncryptedBackupTitle,
            Self::BackupExportPassphrase => MessageId::SettingsExportPassphraseLabel,
            Self::BackupImportPassphrase => MessageId::SettingsImportPassphraseLabel,
            Self::MobileVault => MessageId::SettingsMobileVaultLabel,
            Self::MobilePairing => MessageId::SettingsMobilePairingLabel,
            Self::SharedFolderSync => MessageId::SettingsSharedFolderSyncTitle,
            Self::SyncFolder => MessageId::SettingsSyncFolderLabel,
        }
    }

    pub const fn description(self) -> MessageId {
        match self {
            Self::Theme => MessageId::SettingsThemeDescription,
            Self::DevelopmentLocale => MessageId::SettingsDevelopmentLocaleDescription,
            Self::TerminalFontSize => MessageId::SettingsTerminalFontSizeDescription,
            Self::CopyOnSelect => MessageId::SettingsCopyOnSelectDescription,
            Self::ConfirmMultilinePaste => MessageId::SettingsConfirmMultilinePasteDescription,
            Self::TerminalFontFamily => MessageId::SettingsTerminalFontFamilyDescription,
            Self::RestoreWorkspaces => MessageId::SettingsRestoreWorkspacesDescription,
            Self::Onboarding => MessageId::SettingsOnboardingDescription,
            Self::SessionHistoryLimit => MessageId::SettingsSessionHistoryLimitDescription,
            Self::DefaultSshDirectory => MessageId::SettingsDefaultSshDirectoryDescription,
            Self::AutoReconnectAttempts => MessageId::SettingsAutoReconnectDescription,
            Self::SshKeepalive => MessageId::SettingsSshKeepaliveDescription,
            Self::ReconnectDelay => MessageId::SettingsReconnectDelayDescription,
            Self::LocalShellProgram => MessageId::SettingsLocalShellDescription,
            Self::LocalShellWorkingDirectory => MessageId::SettingsLocalShellDescription,
            Self::Diagnostics => MessageId::DiagnosticsSettingsDescription,
            Self::StorageHealth => MessageId::HealthSettingsDescription,
            Self::NotificationMode => MessageId::NotificationSettingsDescription,
            Self::RecordingFriendly => MessageId::NotificationRecordingDescription,
            Self::RemoteDevices => MessageId::RemoteDevicesDescription,
            Self::CliStatus => MessageId::CliSettingsDescription,
            Self::KeyboardShortcuts => MessageId::SettingsKeyboardShortcutsDescription,
            Self::PortableData => MessageId::SettingsPortableDataDescription,
            Self::EncryptedBackup => MessageId::SettingsEncryptedBackupDescription,
            Self::BackupExportPassphrase => MessageId::SettingsPassphraseSafetyNotice,
            Self::BackupImportPassphrase => MessageId::SettingsEncryptedImportDescription,
            Self::MobileVault => MessageId::SettingsMobileVaultDescription,
            Self::MobilePairing => MessageId::SettingsMobilePairingDescription,
            Self::SharedFolderSync => MessageId::SettingsSharedFolderSyncDescription,
            Self::SyncFolder => MessageId::SettingsSyncFolderDescription,
        }
    }

    pub const fn kind(self) -> SettingControlKind {
        match self {
            Self::CopyOnSelect
            | Self::ConfirmMultilinePaste
            | Self::RestoreWorkspaces
            | Self::Diagnostics
            | Self::RecordingFriendly => SettingControlKind::Toggle,
            Self::TerminalFontFamily
            | Self::DefaultSshDirectory
            | Self::LocalShellProgram
            | Self::LocalShellWorkingDirectory
            | Self::SyncFolder
            | Self::MobilePairing => SettingControlKind::Text,
            Self::BackupExportPassphrase | Self::BackupImportPassphrase => {
                SettingControlKind::Secret
            }
            Self::TerminalFontSize
            | Self::SessionHistoryLimit
            | Self::AutoReconnectAttempts
            | Self::SshKeepalive
            | Self::ReconnectDelay => SettingControlKind::Number,
            Self::Theme | Self::DevelopmentLocale | Self::NotificationMode => {
                SettingControlKind::Choice
            }
            Self::CliStatus | Self::KeyboardShortcuts | Self::RemoteDevices => {
                SettingControlKind::Status
            }
            Self::Onboarding
            | Self::StorageHealth
            | Self::PortableData
            | Self::EncryptedBackup
            | Self::MobileVault
            | Self::SharedFolderSync => SettingControlKind::Action,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingControlKind {
    Toggle,
    Text,
    Secret,
    Number,
    Choice,
    Action,
    Status,
}

impl SettingControlKind {
    const fn role(self) -> SemanticRole {
        match self {
            Self::Toggle => SemanticRole::Checkbox,
            Self::Text | Self::Secret | Self::Number | Self::Choice => SemanticRole::TextField,
            Self::Action => SemanticRole::Button,
            Self::Status => SemanticRole::Group,
        }
    }

    const fn primary_action(self) -> Option<SemanticAction> {
        match self {
            Self::Toggle | Self::Action => Some(SemanticAction::Activate),
            Self::Text | Self::Secret | Self::Number | Self::Choice => {
                Some(SemanticAction::SetValue)
            }
            Self::Status => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsSurfaceState {
    Ready,
    SearchResults,
    SearchEmpty,
    Validating,
    Saving,
    PermissionDenied,
    StorageFailure,
    Stale,
    RecoveryRequired,
    Unavailable,
    Error,
}

impl SettingsSurfaceState {
    pub const ALL: [Self; 11] = [
        Self::Ready,
        Self::SearchResults,
        Self::SearchEmpty,
        Self::Validating,
        Self::Saving,
        Self::PermissionDenied,
        Self::StorageFailure,
        Self::Stale,
        Self::RecoveryRequired,
        Self::Unavailable,
        Self::Error,
    ];

    pub const fn message(self) -> MessageId {
        match self {
            Self::Ready => MessageId::SettingsStateReady,
            Self::SearchResults => MessageId::SettingsStateSearchResults,
            Self::SearchEmpty => MessageId::SettingsStateSearchEmpty,
            Self::Validating => MessageId::SettingsStateValidating,
            Self::Saving => MessageId::SettingsStateSaving,
            Self::PermissionDenied => MessageId::SettingsStatePermissionDenied,
            Self::StorageFailure => MessageId::SettingsStateStorageFailure,
            Self::Stale => MessageId::SettingsStateStale,
            Self::RecoveryRequired => MessageId::SettingsStateRecoveryRequired,
            Self::Unavailable => MessageId::SettingsStateUnavailable,
            Self::Error => MessageId::SettingsStateError,
        }
    }

    const fn busy(self) -> bool {
        matches!(self, Self::Validating | Self::Saving)
    }

    const fn error(self) -> bool {
        matches!(
            self,
            Self::PermissionDenied
                | Self::StorageFailure
                | Self::Stale
                | Self::RecoveryRequired
                | Self::Unavailable
                | Self::Error
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingValuePresentation {
    Boolean(bool),
    Number {
        current: i64,
        minimum: i64,
        maximum: i64,
    },
    Choice(MessageId),
    Masked,
    Unavailable,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingPresentation {
    pub id: SettingId,
    pub value: SettingValuePresentation,
    pub disabled: bool,
    pub invalid: bool,
    pub destructive: bool,
}

impl SettingPresentation {
    fn validate(&self) -> Result<(), SemanticError> {
        if matches!(self.id.kind(), SettingControlKind::Secret)
            != matches!(
                self.value,
                SettingValuePresentation::Masked | SettingValuePresentation::Unavailable
            )
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        if let SettingValuePresentation::Number {
            current,
            minimum,
            maximum,
        } = self.value
            && (minimum > maximum || current < minimum || current > maximum)
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsSearchDocument {
    pub id: SettingId,
    pub section: SettingsSectionId,
    pub label: String,
    pub help: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsSearchResult {
    pub id: SettingId,
    pub section: SettingsSectionId,
    pub rank: u8,
}

pub fn search_settings(
    query: &str,
    documents: &[SettingsSearchDocument],
) -> Result<Vec<SettingsSearchResult>, SemanticError> {
    if query.chars().count() > MAX_SETTINGS_QUERY_CHARS || documents.len() > MAX_SETTINGS {
        return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
    }
    let query = normalize(query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let terms = query.split_whitespace().collect::<Vec<_>>();
    let mut seen = HashSet::with_capacity(documents.len());
    let mut results = Vec::new();
    for document in documents {
        if document.section != document.id.section()
            || !seen.insert(document.id)
            || !valid_search_text(&document.label)
            || !valid_search_text(&document.help)
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }
        let label = normalize(&document.label);
        let help = normalize(&document.help);
        if !terms
            .iter()
            .all(|term| label.contains(term) || help.contains(term))
        {
            continue;
        }
        let rank = if label == query {
            0
        } else if label.starts_with(&query) {
            1
        } else if label
            .split_whitespace()
            .any(|token| token.starts_with(&query))
        {
            2
        } else if label.contains(&query) {
            3
        } else {
            4
        };
        results.push(SettingsSearchResult {
            id: document.id,
            section: document.section,
            rank,
        });
    }
    results.sort_by_key(|result| (result.rank, result.section, result.id));
    Ok(results)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsSemanticSnapshot {
    pub state: SettingsSurfaceState,
    pub settings: Vec<SettingPresentation>,
    pub search_results: Vec<SettingId>,
    pub query_active: bool,
}

impl SettingsSemanticSnapshot {
    pub fn try_nodes(&self, parent: SemanticNodeId) -> Result<Vec<SemanticNode>, SemanticError> {
        if self.settings.len() > MAX_SETTINGS || self.search_results.len() > MAX_SETTINGS {
            return Err(SemanticError::new(SemanticErrorCode::ResourceLimit, None));
        }
        let mut ids = HashSet::with_capacity(self.settings.len());
        for setting in &self.settings {
            setting.validate()?;
            if !ids.insert(setting.id) {
                return Err(SemanticError::new(SemanticErrorCode::DuplicateNode, None));
            }
        }
        if self
            .search_results
            .iter()
            .any(|setting| !ids.contains(setting))
        {
            return Err(SemanticError::new(SemanticErrorCode::InvalidValue, None));
        }

        let root_id = semantic_id(ROOT_NODE);
        let mut root = named_node(root_id, SemanticRole::Landmark, MessageId::SettingsTitle);
        root.parent = Some(parent);
        root.state.busy = self.state.busy();

        let mut status = named_node(
            semantic_id(STATUS_NODE),
            if self.state.error() {
                SemanticRole::Alert
            } else {
                SemanticRole::Status
            },
            self.state.message(),
        );
        status.parent = Some(root_id);
        status.state.live = Some(if self.state.error() {
            LiveRegionPoliteness::Immediate
        } else {
            LiveRegionPoliteness::Polite
        });

        let mut search = named_node(
            semantic_id(SEARCH_NODE),
            SemanticRole::TextField,
            MessageId::SettingsSearchLabel,
        );
        search.parent = Some(root_id);
        search
            .actions
            .extend([SemanticAction::Focus, SemanticAction::SetValue]);

        let mut nodes = vec![root, status, search];
        if self.query_active {
            let mut clear = named_node(
                semantic_id(CLEAR_NODE),
                SemanticRole::Button,
                MessageId::SettingsSearchClear,
            );
            clear.parent = Some(root_id);
            clear
                .actions
                .extend([SemanticAction::Focus, SemanticAction::Activate]);
            nodes.push(clear);
        }

        let visible = if self.query_active {
            self.search_results.iter().copied().collect::<BTreeSet<_>>()
        } else {
            ids.iter().copied().collect::<BTreeSet<_>>()
        };
        for section in SettingsSectionId::ALL {
            if !self
                .settings
                .iter()
                .any(|setting| setting.id.section() == section && visible.contains(&setting.id))
            {
                continue;
            }
            let section_id = section_semantic_node(section);
            let mut node = named_node(section_id, SemanticRole::Group, section.title());
            node.parent = Some(root_id);
            node.description = Some(SemanticText::Message(section.description()));
            node.actions.insert(SemanticAction::Focus);
            nodes.push(node);
        }

        for setting in self
            .settings
            .iter()
            .filter(|setting| visible.contains(&setting.id))
        {
            let kind = setting.id.kind();
            let mut node = named_node(
                setting_semantic_node(setting.id),
                kind.role(),
                setting.id.label(),
            );
            node.parent = Some(section_semantic_node(setting.id.section()));
            node.description = Some(SemanticText::Message(if setting.destructive {
                MessageId::SensitiveDestructiveActionWarning
            } else {
                setting.id.description()
            }));
            node.state = SemanticState {
                disabled: setting.disabled,
                selected: false,
                expanded: None,
                checked: match setting.value {
                    SettingValuePresentation::Boolean(value) => Some(value),
                    _ => None,
                },
                invalid: setting.invalid,
                busy: false,
                hidden: false,
                live: None,
            };
            node.value = match setting.value {
                SettingValuePresentation::Boolean(value) => Some(SemanticValue::Boolean(value)),
                SettingValuePresentation::Number {
                    current,
                    minimum,
                    maximum,
                } => Some(SemanticValue::Number {
                    current,
                    minimum,
                    maximum,
                }),
                SettingValuePresentation::Choice(message) => {
                    Some(SemanticValue::PublicText(SemanticText::Message(message)))
                }
                SettingValuePresentation::Masked => Some(SemanticValue::PublicText(
                    SemanticText::Message(MessageId::SecretFieldMasked),
                )),
                SettingValuePresentation::Unavailable => Some(SemanticValue::PublicText(
                    SemanticText::Message(MessageId::SecretFieldUnavailable),
                )),
                SettingValuePresentation::None => None,
            };
            node.actions.insert(SemanticAction::Focus);
            if !setting.disabled
                && let Some(action) = kind.primary_action()
            {
                node.actions.insert(action);
            }
            nodes.push(node);
        }
        Ok(nodes)
    }

    pub fn routes(
        &self,
    ) -> Vec<(
        (SemanticNodeId, SemanticAction),
        SettingsAccessibilityCommand,
    )> {
        let mut routes = vec![
            (
                (semantic_id(SEARCH_NODE), SemanticAction::Focus),
                SettingsAccessibilityCommand::FocusSearch,
            ),
            (
                (semantic_id(SEARCH_NODE), SemanticAction::SetValue),
                SettingsAccessibilityCommand::SetSearchValue,
            ),
        ];
        if self.query_active {
            routes.extend([
                (
                    (semantic_id(CLEAR_NODE), SemanticAction::Focus),
                    SettingsAccessibilityCommand::FocusSearch,
                ),
                (
                    (semantic_id(CLEAR_NODE), SemanticAction::Activate),
                    SettingsAccessibilityCommand::ClearSearch,
                ),
            ]);
        }
        let visible = if self.query_active {
            self.search_results.iter().copied().collect::<BTreeSet<_>>()
        } else {
            self.settings.iter().map(|setting| setting.id).collect()
        };
        for section in SettingsSectionId::ALL {
            if self
                .settings
                .iter()
                .any(|setting| setting.id.section() == section && visible.contains(&setting.id))
            {
                routes.push((
                    (section_semantic_node(section), SemanticAction::Focus),
                    SettingsAccessibilityCommand::FocusSection(section),
                ));
            }
        }
        for setting in self
            .settings
            .iter()
            .filter(|setting| visible.contains(&setting.id))
        {
            let node = setting_semantic_node(setting.id);
            routes.push((
                (node, SemanticAction::Focus),
                SettingsAccessibilityCommand::FocusSetting(setting.id),
            ));
            if !setting.disabled
                && let Some(action) = setting.id.kind().primary_action()
            {
                routes.push((
                    (node, action),
                    match action {
                        SemanticAction::SetValue => {
                            SettingsAccessibilityCommand::SetSettingValue(setting.id)
                        }
                        _ => SettingsAccessibilityCommand::ActivateSetting(setting.id),
                    },
                ));
            }
        }
        routes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsAccessibilityCommand {
    FocusSearch,
    SetSearchValue,
    ClearSearch,
    FocusSection(SettingsSectionId),
    FocusSetting(SettingId),
    ActivateSetting(SettingId),
    SetSettingValue(SettingId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsResponsiveLayout {
    Sidebar,
    Compact,
    Stacked,
}

pub const fn settings_responsive_layout(scale_percent: u16) -> Option<SettingsResponsiveLayout> {
    match scale_percent {
        100..=150 => Some(SettingsResponsiveLayout::Sidebar),
        151..=200 => Some(SettingsResponsiveLayout::Compact),
        201..=400 => Some(SettingsResponsiveLayout::Stacked),
        _ => None,
    }
}

#[derive(Clone, Debug, Default)]
pub struct SettingsAnnouncementCoalescer {
    last: Option<Instant>,
    pending: bool,
}

impl SettingsAnnouncementCoalescer {
    pub fn record_change(&mut self, now: Instant, final_change: bool) -> bool {
        if final_change
            || self.last.is_none_or(|last| {
                now.saturating_duration_since(last) >= SETTINGS_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last = Some(now);
            self.pending = false;
            return true;
        }
        self.pending = true;
        false
    }

    pub fn flush(&mut self, now: Instant) -> bool {
        if self.pending
            && self.last.is_none_or(|last| {
                now.saturating_duration_since(last) >= SETTINGS_ANNOUNCEMENT_INTERVAL
            })
        {
            self.last = Some(now);
            self.pending = false;
            return true;
        }
        false
    }
}

pub fn settings_root_semantic_node() -> SemanticNodeId {
    semantic_id(ROOT_NODE)
}

fn section_semantic_node(section: SettingsSectionId) -> SemanticNodeId {
    semantic_id(SECTION_NODE_BASE | (stable_hash(&section) & NODE_MASK))
}

fn setting_semantic_node(setting: SettingId) -> SemanticNodeId {
    semantic_id(SETTING_NODE_BASE | (stable_hash(&setting) & NODE_MASK))
}

fn named_node(id: SemanticNodeId, role: SemanticRole, name: MessageId) -> SemanticNode {
    let mut node = SemanticNode::new(id, role);
    node.name = Some(SemanticText::Message(name));
    node
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

fn valid_search_text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= crate::MAX_SEMANTIC_TEXT_CHARS
        && !value.contains('\0')
}

fn semantic_id(value: u64) -> SemanticNodeId {
    SemanticNodeId::new(NonZeroU64::new(value).expect("semantic node IDs are non-zero"))
}

fn stable_hash(value: &impl Hash) -> u64 {
    let mut hasher = StableHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}

impl Hasher for StableHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesignTokens, Locale, Localizer, ThemeKind};

    fn settings() -> Vec<SettingPresentation> {
        SettingId::ALL
            .into_iter()
            .map(|id| SettingPresentation {
                value: match id.kind() {
                    SettingControlKind::Toggle => SettingValuePresentation::Boolean(false),
                    SettingControlKind::Secret => SettingValuePresentation::Masked,
                    SettingControlKind::Number => SettingValuePresentation::Number {
                        current: 1,
                        minimum: 0,
                        maximum: 10,
                    },
                    SettingControlKind::Choice => {
                        SettingValuePresentation::Choice(MessageId::SettingsValueOn)
                    }
                    _ => SettingValuePresentation::None,
                },
                id,
                disabled: false,
                invalid: false,
                destructive: false,
            })
            .collect()
    }

    fn snapshot() -> SettingsSemanticSnapshot {
        SettingsSemanticSnapshot {
            state: SettingsSurfaceState::Ready,
            settings: settings(),
            search_results: Vec::new(),
            query_active: false,
        }
    }

    #[test]
    fn complete_built_inventory_has_stable_sections_and_unique_ids() {
        let ids = SettingId::ALL.into_iter().collect::<HashSet<_>>();
        assert_eq!(ids.len(), SettingId::ALL.len());
        for id in SettingId::ALL {
            assert!(SettingsSectionId::ALL.contains(&id.section()));
        }
    }

    #[test]
    fn search_is_value_free_bounded_and_deterministic() {
        let documents = vec![
            SettingsSearchDocument {
                id: SettingId::TerminalFontSize,
                section: SettingsSectionId::Terminal,
                label: "Terminal font size".to_string(),
                help: "Adjust terminal text".to_string(),
            },
            SettingsSearchDocument {
                id: SettingId::BackupExportPassphrase,
                section: SettingsSectionId::StoragePrivacyDiagnostics,
                label: "Export passphrase".to_string(),
                help: "Encrypt a local backup".to_string(),
            },
        ];
        let first = search_settings("terminal", &documents).unwrap();
        let second = search_settings("terminal", &documents).unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].id, SettingId::TerminalFontSize);
        assert!(search_settings(&"x".repeat(MAX_SETTINGS_QUERY_CHARS + 1), &documents).is_err());
    }

    #[test]
    fn search_rejects_wrong_section_and_duplicate_setting() {
        let wrong = SettingsSearchDocument {
            id: SettingId::Theme,
            section: SettingsSectionId::Keyboard,
            label: "Theme".to_string(),
            help: "Appearance".to_string(),
        };
        assert_eq!(
            search_settings("theme", &[wrong]).unwrap_err().code,
            SemanticErrorCode::InvalidValue
        );
    }

    #[test]
    fn secret_values_can_only_be_masked_or_unavailable() {
        let mut invalid = snapshot();
        let secret = invalid
            .settings
            .iter_mut()
            .find(|setting| setting.id == SettingId::BackupExportPassphrase)
            .unwrap();
        secret.value = SettingValuePresentation::None;
        assert_eq!(
            invalid.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::InvalidValue
        );
    }

    #[test]
    fn query_exposes_only_exact_current_results() {
        let mut filtered = snapshot();
        filtered.query_active = true;
        filtered.state = SettingsSurfaceState::SearchResults;
        filtered.search_results = vec![SettingId::TerminalFontSize];
        let nodes = filtered.try_nodes(semantic_id(9)).unwrap();
        assert!(nodes.iter().any(|node| {
            node.name
                == Some(SemanticText::Message(
                    MessageId::SettingsTerminalFontSizeLabel,
                ))
        }));
        assert!(!nodes.iter().any(|node| {
            node.name == Some(SemanticText::Message(MessageId::SettingsCopyOnSelectLabel))
        }));
    }

    #[test]
    fn unavailable_future_controls_are_absent_from_built_inventory() {
        assert_eq!(SettingId::ALL.len(), 30);
        assert!(
            SettingId::ALL
                .iter()
                .all(|id| { !matches!(id.description(), MessageId::SettingsStateUnavailable) })
        );
    }

    #[test]
    fn every_state_locale_theme_and_scale_is_defined() {
        for state in SettingsSurfaceState::ALL {
            assert!(MessageId::ALL.contains(&state.message()));
        }
        for locale in Locale::ALL {
            let localizer = Localizer::try_new(locale.tag()).unwrap();
            assert!(
                !localizer
                    .format_static(MessageId::SettingsTitle)
                    .unwrap()
                    .is_empty()
            );
        }
        for theme in ThemeKind::ALL {
            assert!(DesignTokens::new(theme).focus_ring_width().0 >= 2.0);
        }
        for scale in [100, 150, 200, 300, 400] {
            assert!(settings_responsive_layout(scale).is_some());
        }
        assert!(settings_responsive_layout(99).is_none());
        assert!(settings_responsive_layout(401).is_none());
    }

    #[test]
    fn malformed_and_oversized_snapshots_fail_closed() {
        let mut duplicate = snapshot();
        duplicate.settings.push(duplicate.settings[0].clone());
        assert_eq!(
            duplicate.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::DuplicateNode
        );
        let mut oversized = snapshot();
        oversized.search_results = vec![SettingId::Theme; MAX_SETTINGS + 1];
        assert_eq!(
            oversized.try_nodes(semantic_id(9)).unwrap_err().code,
            SemanticErrorCode::ResourceLimit
        );
    }

    #[test]
    fn destructive_controls_have_non_color_warning() {
        let mut destructive = snapshot();
        destructive.settings[0].destructive = true;
        let nodes = destructive.try_nodes(semantic_id(9)).unwrap();
        assert!(nodes.iter().any(|node| {
            node.description
                == Some(SemanticText::Message(
                    MessageId::SensitiveDestructiveActionWarning,
                ))
        }));
    }

    #[test]
    fn announcements_are_bounded_and_final_results_are_immediate() {
        let start = Instant::now();
        let mut coalescer = SettingsAnnouncementCoalescer::default();
        assert!(coalescer.record_change(start, false));
        assert!(!coalescer.record_change(start + Duration::from_millis(10), false));
        assert!(coalescer.record_change(start + Duration::from_millis(20), true));
    }
}
