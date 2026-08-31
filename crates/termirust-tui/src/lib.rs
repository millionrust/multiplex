//! Bounded fleet/device control and single-session durable terminal attachment.

pub mod attach;
pub mod devices;
pub mod input;
pub mod localization;
pub mod management;
pub mod model;
pub mod render;
pub mod source;
pub mod terminal;
pub mod terminal_view;

pub use attach::{
    AttachBatch, AttachCommand, AttachEvent, AttachFailure, AttachSnapshot, AttachWorker,
    AttachedTerminal, HostAttachState, HostLifecycle, TuiAttachState, Viewport,
    endpoint_for_source, spawn_attach_worker,
};
pub use devices::{
    DeviceEffect, DeviceExecutor, DeviceFailure, DeviceProgress, DeviceRevocationResult,
    DeviceRevocationReview, DeviceSnapshot, DevicesModel, LocalDeviceExecutor, MAX_TUI_DEVICES,
    TuiDevice,
};
pub use input::{InputDecision, InteractiveLease, TerminalInputModel, TuiFocus};
pub use management::{
    CommandProgress, ConfirmationKind, LaunchChoice, LocalManagementExecutor, ManagementCommand,
    ManagementDraft, ManagementEffect, ManagementExecutor, ManagementFailure, ManagementIntent,
    ManagementModel, ManagementResult, RemovalPreview,
};
pub use model::{
    FleetGroup, FleetHealth, FleetProject, FleetRevision, FleetSession, FleetSnapshot, LoadState,
    MAX_FILTER_SCALARS, MAX_PROJECTS, MAX_VISIBLE_SESSIONS, ModelAction, ModelEffect, PaneFocus,
    ProjectAvailability, ScopeId, TuiDiagnostic, TuiModel,
};
pub use source::{FleetCancellation, FleetSource, LocalFleetSource};
