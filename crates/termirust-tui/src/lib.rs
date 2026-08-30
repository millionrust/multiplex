//! Bounded, read-only terminal fleet navigation.

pub mod localization;
pub mod model;
pub mod render;
pub mod source;
pub mod terminal;

pub use model::{
    FleetGroup, FleetHealth, FleetProject, FleetRevision, FleetSession, FleetSnapshot, LoadState,
    MAX_FILTER_SCALARS, MAX_PROJECTS, MAX_VISIBLE_SESSIONS, ModelAction, ModelEffect, PaneFocus,
    ProjectAvailability, ScopeId, TuiDiagnostic, TuiModel,
};
pub use source::{FleetCancellation, FleetSource, LocalFleetSource};
