use std::fmt;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termirust_cli::{
    Cancellation, CliCommand, CliData, CliPaths, CommandService, DeviceListFilter, DeviceView,
    ErrorCode, LocalCommandService,
};
use termirust_domain::{ControllerDeviceId, DeviceStoreRevision};

pub const MAX_TUI_DEVICES: usize = 1_000;
const MAX_DEVICE_TEXT_SCALARS: usize = 256;
const MAX_CAPABILITIES: usize = 8;
const MAX_CAPABILITY_SCALARS: usize = 32;

#[derive(Clone, Eq, PartialEq)]
pub struct TuiDevice {
    pub id: String,
    pub name: String,
    pub status: String,
    pub capabilities: Vec<String>,
    pub protocol_minimum: u16,
    pub protocol_maximum: u16,
    pub created_at_unix_seconds: u64,
    pub last_seen_at_unix_seconds: Option<u64>,
    pub fingerprint_suffix: String,
    pub identity_generation: u64,
}

impl TuiDevice {
    pub fn revoked(&self) -> bool {
        self.status == "revoked"
    }
}

impl fmt::Debug for TuiDevice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TuiDevice")
            .field("id", &self.id)
            .field("status", &self.status)
            .field("capability_count", &self.capabilities.len())
            .field("identity_generation", &self.identity_generation)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceSnapshot {
    pub repository_revision: u64,
    pub devices: Vec<TuiDevice>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct DeviceRevocationReview {
    pub repository_revision: u64,
    pub device: TuiDevice,
    pub active_access_will_be_revoked: bool,
    pub other_devices_reconnect: bool,
}

impl fmt::Debug for DeviceRevocationReview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceRevocationReview")
            .field("repository_revision", &self.repository_revision)
            .field("device_id", &self.device.id)
            .field(
                "active_access_will_be_revoked",
                &self.active_access_will_be_revoked,
            )
            .field("other_devices_reconnect", &self.other_devices_reconnect)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceRevocationResult {
    pub repository_revision: u64,
    pub device: TuiDevice,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceFailure {
    pub code: &'static str,
    pub summary: String,
    pub recovery: String,
    pub conflict_revision: Option<u64>,
}

impl DeviceFailure {
    fn validation(summary: impl Into<String>, recovery: impl Into<String>) -> Self {
        Self {
            code: "validation",
            summary: bounded_text(summary.into(), MAX_DEVICE_TEXT_SCALARS),
            recovery: bounded_text(recovery.into(), MAX_DEVICE_TEXT_SCALARS),
            conflict_revision: None,
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            code: "unavailable",
            summary: "Paired device management is unavailable.".into(),
            recovery: "Return to the fleet or refresh after checking Controller state.".into(),
            conflict_revision: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceProgress {
    Idle,
    Loading,
    Ready,
    LoadingReview,
    Reviewing,
    Revoking,
    Succeeded { summary: String },
    Failed(DeviceFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceEffect {
    None,
    Close,
    Quit,
    Load,
    Review {
        device_id: String,
    },
    Revoke {
        device_id: String,
        expected_revision: u64,
    },
}

pub struct DevicesModel {
    active: bool,
    generation: u64,
    snapshot: Option<DeviceSnapshot>,
    selected: usize,
    progress: DeviceProgress,
    review: Option<DeviceRevocationReview>,
    pending_device_id: Option<String>,
    cancellation: Cancellation,
    help_visible: bool,
}

impl Default for DevicesModel {
    fn default() -> Self {
        Self {
            active: false,
            generation: 0,
            snapshot: None,
            selected: 0,
            progress: DeviceProgress::Idle,
            review: None,
            pending_device_id: None,
            cancellation: Cancellation::default(),
            help_visible: false,
        }
    }
}

impl DevicesModel {
    pub fn active(&self) -> bool {
        self.active
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn snapshot(&self) -> Option<&DeviceSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_device(&self) -> Option<&TuiDevice> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.devices.get(self.selected))
    }

    pub fn progress(&self) -> &DeviceProgress {
        &self.progress
    }

    pub fn review(&self) -> Option<&DeviceRevocationReview> {
        self.review.as_ref()
    }

    pub fn help_visible(&self) -> bool {
        self.help_visible
    }

    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    pub fn open(&mut self) -> DeviceEffect {
        self.active = true;
        self.snapshot = None;
        self.selected = 0;
        self.review = None;
        self.pending_device_id = None;
        self.help_visible = false;
        self.begin_operation(DeviceProgress::Loading);
        DeviceEffect::Load
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> DeviceEffect {
        if !self.active {
            return DeviceEffect::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return if matches!(self.progress, DeviceProgress::Revoking) {
                DeviceEffect::None
            } else {
                self.close();
                DeviceEffect::Quit
            };
        }
        if !key.modifiers.is_empty() {
            return DeviceEffect::None;
        }
        if self.help_visible {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('?') => {
                    self.help_visible = false;
                    DeviceEffect::None
                }
                KeyCode::Char('q') => {
                    self.close();
                    DeviceEffect::Quit
                }
                _ => DeviceEffect::None,
            };
        }
        match key.code {
            KeyCode::Esc => self.escape(),
            KeyCode::Char('q') if !matches!(self.progress, DeviceProgress::Revoking) => {
                self.close();
                DeviceEffect::Quit
            }
            KeyCode::Char('f')
                if !matches!(
                    self.progress,
                    DeviceProgress::LoadingReview
                        | DeviceProgress::Reviewing
                        | DeviceProgress::Revoking
                ) =>
            {
                self.close();
                DeviceEffect::Close
            }
            KeyCode::Char('?') if !matches!(self.progress, DeviceProgress::Revoking) => {
                self.help_visible = !self.help_visible;
                DeviceEffect::None
            }
            KeyCode::Char('r')
                if !matches!(
                    self.progress,
                    DeviceProgress::Loading
                        | DeviceProgress::LoadingReview
                        | DeviceProgress::Reviewing
                        | DeviceProgress::Revoking
                ) =>
            {
                self.review = None;
                self.pending_device_id = None;
                self.begin_operation(DeviceProgress::Loading);
                DeviceEffect::Load
            }
            KeyCode::Up | KeyCode::Char('k') if self.browsable() => {
                self.move_selection(-1);
                DeviceEffect::None
            }
            KeyCode::Down | KeyCode::Char('j') if self.browsable() => {
                self.move_selection(1);
                DeviceEffect::None
            }
            KeyCode::Char('x') if self.browsable() => self.begin_review(),
            KeyCode::Enter if matches!(self.progress, DeviceProgress::Reviewing) => {
                let Some(review) = self.review.as_ref() else {
                    self.progress = DeviceProgress::Failed(DeviceFailure::unavailable());
                    return DeviceEffect::None;
                };
                let device_id = review.device.id.clone();
                let expected_revision = review.repository_revision;
                self.begin_operation(DeviceProgress::Revoking);
                DeviceEffect::Revoke {
                    device_id,
                    expected_revision,
                }
            }
            _ => DeviceEffect::None,
        }
    }

    pub fn loaded(&mut self, generation: u64, result: Result<DeviceSnapshot, DeviceFailure>) {
        if !self.accepts(generation, |progress| {
            matches!(progress, DeviceProgress::Loading)
        }) {
            return;
        }
        match result {
            Ok(snapshot) if snapshot.devices.len() <= MAX_TUI_DEVICES => {
                self.replace_snapshot(snapshot);
                self.progress = DeviceProgress::Ready;
            }
            Ok(_) => {
                self.progress = DeviceProgress::Failed(DeviceFailure::validation(
                    "The paired device list exceeded the TUI limit.",
                    "Use the CLI or desktop to reduce the device list, then refresh.",
                ));
            }
            Err(error) => self.progress = DeviceProgress::Failed(error),
        }
    }

    pub fn reviewed(
        &mut self,
        generation: u64,
        result: Result<DeviceRevocationReview, DeviceFailure>,
    ) {
        if !self.accepts(generation, |progress| {
            matches!(progress, DeviceProgress::LoadingReview)
        }) {
            return;
        }
        match result {
            Ok(review)
                if self
                    .pending_device_id
                    .as_ref()
                    .is_some_and(|device_id| device_id == &review.device.id)
                    && !review.device.revoked()
                    && review.active_access_will_be_revoked =>
            {
                self.review = Some(review);
                self.progress = DeviceProgress::Reviewing;
            }
            Ok(_) => {
                self.review = None;
                self.pending_device_id = None;
                self.progress = DeviceProgress::Failed(DeviceFailure::validation(
                    "The revocation review did not match the selected active device.",
                    "Refresh paired devices before taking another action.",
                ));
            }
            Err(error) => {
                self.review = None;
                self.pending_device_id = None;
                self.progress = DeviceProgress::Failed(error);
            }
        }
    }

    pub fn revoked(
        &mut self,
        generation: u64,
        result: Result<DeviceRevocationResult, DeviceFailure>,
    ) {
        if !self.accepts(generation, |progress| {
            matches!(progress, DeviceProgress::Revoking)
        }) {
            return;
        }
        match result {
            Ok(result)
                if self
                    .pending_device_id
                    .as_ref()
                    .is_some_and(|device_id| device_id == &result.device.id)
                    && result.device.revoked() =>
            {
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.repository_revision = result.repository_revision;
                    if let Some(device) = snapshot
                        .devices
                        .iter_mut()
                        .find(|device| device.id == result.device.id)
                    {
                        *device = result.device;
                    }
                }
                self.review = None;
                self.pending_device_id = None;
                self.progress = DeviceProgress::Succeeded {
                    summary: "Selected device access was revoked.".into(),
                };
            }
            Ok(_) => {
                self.review = None;
                self.pending_device_id = None;
                self.progress = DeviceProgress::Failed(DeviceFailure::validation(
                    "The revocation result did not match the selected device.",
                    "Refresh paired devices and inspect the authoritative result.",
                ));
            }
            Err(error) => {
                self.review = None;
                self.pending_device_id = None;
                self.progress = DeviceProgress::Failed(error);
            }
        }
    }

    pub fn close(&mut self) {
        self.cancellation.cancel();
        self.generation = self.generation.saturating_add(1);
        self.active = false;
        self.review = None;
        self.pending_device_id = None;
        self.help_visible = false;
        self.progress = DeviceProgress::Idle;
    }

    fn escape(&mut self) -> DeviceEffect {
        match self.progress {
            DeviceProgress::LoadingReview => {
                self.cancellation.cancel();
                self.generation = self.generation.saturating_add(1);
                self.review = None;
                self.pending_device_id = None;
                self.progress = DeviceProgress::Ready;
                DeviceEffect::None
            }
            DeviceProgress::Reviewing => {
                self.review = None;
                self.pending_device_id = None;
                self.progress = DeviceProgress::Ready;
                DeviceEffect::None
            }
            DeviceProgress::Revoking => DeviceEffect::None,
            _ => {
                self.close();
                DeviceEffect::Close
            }
        }
    }

    fn begin_review(&mut self) -> DeviceEffect {
        let Some(device) = self.selected_device() else {
            return DeviceEffect::None;
        };
        if device.revoked() {
            self.progress = DeviceProgress::Failed(DeviceFailure::validation(
                "This paired device is already revoked.",
                "Select an active paired device or return to the fleet.",
            ));
            return DeviceEffect::None;
        }
        let device_id = device.id.clone();
        self.pending_device_id = Some(device_id.clone());
        self.review = None;
        self.begin_operation(DeviceProgress::LoadingReview);
        DeviceEffect::Review { device_id }
    }

    fn begin_operation(&mut self, progress: DeviceProgress) {
        self.cancellation.cancel();
        self.cancellation = Cancellation::default();
        self.generation = self.generation.saturating_add(1);
        self.progress = progress;
    }

    fn accepts(&self, generation: u64, predicate: impl FnOnce(&DeviceProgress) -> bool) -> bool {
        self.active && generation == self.generation && predicate(&self.progress)
    }

    fn browsable(&self) -> bool {
        matches!(
            self.progress,
            DeviceProgress::Ready | DeviceProgress::Succeeded { .. }
        )
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.devices.len());
        if count == 0 {
            self.selected = 0;
            return;
        }
        self.selected = if delta.is_negative() {
            self.selected.saturating_sub(delta.unsigned_abs())
        } else {
            self.selected
                .saturating_add(delta as usize)
                .min(count.saturating_sub(1))
        };
        if matches!(self.progress, DeviceProgress::Succeeded { .. }) {
            self.progress = DeviceProgress::Ready;
        }
    }

    fn replace_snapshot(&mut self, snapshot: DeviceSnapshot) {
        let previous = self.selected_device().map(|device| device.id.clone());
        self.snapshot = Some(snapshot);
        self.selected = previous
            .and_then(|id| {
                self.snapshot
                    .as_ref()?
                    .devices
                    .iter()
                    .position(|device| device.id == id)
            })
            .unwrap_or(0);
    }
}

impl fmt::Debug for DevicesModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DevicesModel")
            .field("active", &self.active)
            .field("generation", &self.generation)
            .field("progress", &self.progress)
            .field(
                "device_count",
                &self
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.devices.len()),
            )
            .field("selected", &self.selected)
            .field("review", &self.review.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

pub trait DeviceExecutor: Send + Sync {
    fn load(&self, cancellation: &Cancellation) -> Result<DeviceSnapshot, DeviceFailure>;

    fn review_revoke(
        &self,
        device_id: &str,
        cancellation: &Cancellation,
    ) -> Result<DeviceRevocationReview, DeviceFailure>;

    fn revoke(
        &self,
        device_id: &str,
        expected_revision: u64,
        cancellation: &Cancellation,
    ) -> Result<DeviceRevocationResult, DeviceFailure>;
}

#[derive(Clone, Debug)]
pub struct LocalDeviceExecutor {
    paths: CliPaths,
}

impl LocalDeviceExecutor {
    pub fn new(config_root: PathBuf) -> Self {
        Self {
            paths: CliPaths::new(config_root, PathBuf::from("termirust-session-host")),
        }
    }

    fn service(&self) -> LocalCommandService {
        LocalCommandService::open(self.paths.clone())
    }
}

impl DeviceExecutor for LocalDeviceExecutor {
    fn load(&self, cancellation: &Cancellation) -> Result<DeviceSnapshot, DeviceFailure> {
        let data = self
            .service()
            .execute(
                CliCommand::DeviceList(DeviceListFilter::default()),
                cancellation,
            )
            .map_err(map_cli_error)?;
        let CliData::Devices(data) = data else {
            return Err(DeviceFailure::unavailable());
        };
        if data.devices.len() > MAX_TUI_DEVICES {
            return Err(DeviceFailure::validation(
                "The paired device list exceeded the TUI limit.",
                "Use the CLI or desktop to reduce the device list, then refresh.",
            ));
        }
        Ok(DeviceSnapshot {
            repository_revision: data.repository_revision,
            devices: data
                .devices
                .into_iter()
                .map(map_device)
                .collect::<Result<_, _>>()?,
        })
    }

    fn review_revoke(
        &self,
        device_id: &str,
        cancellation: &Cancellation,
    ) -> Result<DeviceRevocationReview, DeviceFailure> {
        let device_id = parse_device_id(device_id)?;
        let data = self
            .service()
            .execute(
                CliCommand::DeviceRevoke {
                    device_id,
                    expected_revision: None,
                    confirmed: false,
                },
                cancellation,
            )
            .map_err(map_cli_error)?;
        let CliData::DeviceRevocationPreview(data) = data else {
            return Err(DeviceFailure::unavailable());
        };
        if !data.confirmation_required {
            return Err(DeviceFailure::validation(
                "The device authority returned an unsafe revocation review.",
                "Refresh paired devices before taking another action.",
            ));
        }
        Ok(DeviceRevocationReview {
            repository_revision: data.repository_revision,
            device: map_device(data.device)?,
            active_access_will_be_revoked: data.active_access_will_be_revoked,
            other_devices_reconnect: data.other_devices_reconnect,
        })
    }

    fn revoke(
        &self,
        device_id: &str,
        expected_revision: u64,
        cancellation: &Cancellation,
    ) -> Result<DeviceRevocationResult, DeviceFailure> {
        let data = self
            .service()
            .execute(
                CliCommand::DeviceRevoke {
                    device_id: parse_device_id(device_id)?,
                    expected_revision: Some(DeviceStoreRevision::new(expected_revision)),
                    confirmed: true,
                },
                cancellation,
            )
            .map_err(map_cli_error)?;
        let CliData::DeviceRevocation(data) = data else {
            return Err(DeviceFailure::unavailable());
        };
        if !data.applied || !data.active_access_revoked {
            return Err(DeviceFailure::validation(
                "The device authority did not confirm revocation.",
                "Refresh paired devices and inspect the authoritative state.",
            ));
        }
        Ok(DeviceRevocationResult {
            repository_revision: data.repository_revision,
            device: map_device(data.device)?,
        })
    }
}

fn parse_device_id(value: &str) -> Result<ControllerDeviceId, DeviceFailure> {
    value
        .parse::<uuid::Uuid>()
        .map(ControllerDeviceId::from_uuid)
        .map_err(|_| {
            DeviceFailure::validation(
                "The selected device identity is invalid.",
                "Refresh paired devices before taking another action.",
            )
        })
}

fn map_device(value: DeviceView) -> Result<TuiDevice, DeviceFailure> {
    if !matches!(value.status.as_str(), "offline" | "online" | "revoked")
        || value.capabilities.len() > MAX_CAPABILITIES
    {
        return Err(DeviceFailure::validation(
            "The paired device response was inconsistent.",
            "Refresh paired devices before taking another action.",
        ));
    }
    Ok(TuiDevice {
        id: bounded_text(value.id, 64),
        name: bounded_text(value.name, MAX_DEVICE_TEXT_SCALARS),
        status: value.status,
        capabilities: value
            .capabilities
            .into_iter()
            .map(|capability| bounded_text(capability, MAX_CAPABILITY_SCALARS))
            .collect(),
        protocol_minimum: value.protocol_minimum,
        protocol_maximum: value.protocol_maximum,
        created_at_unix_seconds: value.created_at_unix_seconds,
        last_seen_at_unix_seconds: value.last_seen_at_unix_seconds,
        fingerprint_suffix: bounded_text(value.fingerprint_suffix, 32),
        identity_generation: value.identity_generation,
    })
}

fn map_cli_error(error: termirust_cli::CliError) -> DeviceFailure {
    DeviceFailure {
        code: match error.code {
            ErrorCode::Conflict => "conflict",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::PermissionDenied => "permission-denied",
            ErrorCode::ResourceLimit => "resource-limit",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::Incompatible => "incompatible",
            ErrorCode::Validation => "validation",
            _ => "operation-failed",
        },
        summary: bounded_text(error.message, MAX_DEVICE_TEXT_SCALARS),
        recovery: bounded_text(error.hint, MAX_DEVICE_TEXT_SCALARS),
        conflict_revision: error.current_revision,
    }
}

fn bounded_text(value: String, limit: usize) -> String {
    value
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '\u{202a}'
                        | '\u{202b}'
                        | '\u{202c}'
                        | '\u{202d}'
                        | '\u{202e}'
                        | '\u{2066}'
                        | '\u{2067}'
                        | '\u{2068}'
                        | '\u{2069}'
                )
        })
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn device(id: u128, status: &str) -> TuiDevice {
        TuiDevice {
            id: format!("00000000-0000-0000-0000-{id:012x}"),
            name: format!("Device {id}"),
            status: status.into(),
            capabilities: vec!["observe_sessions".into()],
            protocol_minimum: 1,
            protocol_maximum: 1,
            created_at_unix_seconds: 10,
            last_seen_at_unix_seconds: Some(20),
            fingerprint_suffix: "123456789abc".into(),
            identity_generation: 1,
        }
    }

    #[test]
    fn devices_reducer_requires_fresh_review_and_exact_revision() {
        let mut model = DevicesModel::default();
        assert_eq!(model.open(), DeviceEffect::Load);
        let load_generation = model.generation();
        model.loaded(
            load_generation,
            Ok(DeviceSnapshot {
                repository_revision: 4,
                devices: vec![device(1, "online"), device(2, "offline")],
            }),
        );
        assert_eq!(model.selected_device().unwrap().id, device(1, "online").id);
        model.handle_key(key(KeyCode::Down));
        assert_eq!(model.selected_device().unwrap().id, device(2, "offline").id);

        let selected_id = model.selected_device().unwrap().id.clone();
        assert_eq!(
            model.handle_key(key(KeyCode::Char('x'))),
            DeviceEffect::Review {
                device_id: selected_id.clone(),
            }
        );
        let review_generation = model.generation();
        model.reviewed(
            review_generation,
            Ok(DeviceRevocationReview {
                repository_revision: 7,
                device: device(2, "offline"),
                active_access_will_be_revoked: true,
                other_devices_reconnect: true,
            }),
        );
        assert_eq!(
            model.handle_key(key(KeyCode::Enter)),
            DeviceEffect::Revoke {
                device_id: selected_id,
                expected_revision: 7,
            }
        );
        let revoke_generation = model.generation();
        model.revoked(
            revoke_generation,
            Ok(DeviceRevocationResult {
                repository_revision: 8,
                device: device(2, "revoked"),
            }),
        );
        assert_eq!(model.snapshot().unwrap().repository_revision, 8);
        assert!(model.selected_device().unwrap().revoked());
        assert!(matches!(model.progress(), DeviceProgress::Succeeded { .. }));
    }

    #[test]
    fn devices_reducer_ignores_stale_results_and_cancel_is_safe_default() {
        let mut model = DevicesModel::default();
        model.open();
        let stale = model.generation();
        assert_eq!(model.handle_key(key(KeyCode::Esc)), DeviceEffect::Close);
        model.loaded(
            stale,
            Ok(DeviceSnapshot {
                repository_revision: 99,
                devices: vec![device(1, "online")],
            }),
        );
        assert!(!model.active());
        assert!(model.snapshot().is_none());

        model.open();
        let generation = model.generation();
        model.loaded(
            generation,
            Ok(DeviceSnapshot {
                repository_revision: 1,
                devices: vec![device(1, "online")],
            }),
        );
        model.handle_key(key(KeyCode::Char('x')));
        let generation = model.generation();
        model.reviewed(
            generation,
            Ok(DeviceRevocationReview {
                repository_revision: 2,
                device: device(1, "online"),
                active_access_will_be_revoked: true,
                other_devices_reconnect: false,
            }),
        );
        assert_eq!(model.handle_key(key(KeyCode::Esc)), DeviceEffect::None);
        assert!(matches!(model.progress(), DeviceProgress::Ready));
        assert!(model.active());
    }

    #[test]
    fn revoked_devices_cannot_open_a_confirmation() {
        let mut model = DevicesModel::default();
        model.open();
        let generation = model.generation();
        model.loaded(
            generation,
            Ok(DeviceSnapshot {
                repository_revision: 3,
                devices: vec![device(1, "revoked")],
            }),
        );
        assert_eq!(
            model.handle_key(key(KeyCode::Char('x'))),
            DeviceEffect::None
        );
        assert!(matches!(model.progress(), DeviceProgress::Failed(_)));
    }

    #[test]
    fn devices_help_traps_actions_until_explicitly_closed() {
        let mut model = DevicesModel::default();
        model.open();
        let generation = model.generation();
        model.loaded(
            generation,
            Ok(DeviceSnapshot {
                repository_revision: 1,
                devices: vec![device(1, "online")],
            }),
        );
        model.handle_key(key(KeyCode::Char('?')));
        assert!(model.help_visible());
        assert_eq!(
            model.handle_key(key(KeyCode::Char('x'))),
            DeviceEffect::None
        );
        assert!(matches!(model.progress(), DeviceProgress::Ready));
        assert_eq!(model.handle_key(key(KeyCode::Esc)), DeviceEffect::None);
        assert!(!model.help_visible());
        assert!(model.active());
    }
}
