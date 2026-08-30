//! Small shared enums for the app shell. No methods, no dependencies on
//! `TermiRustApp` — purely state markers passed around between the UI
//! sub-modules.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HostsViewMode {
    Grid,
    List,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HostsSort {
    AZ,
    ZA,
    NewestFirst,
    OldestFirst,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorMenu {
    Vault,
    Overflow,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolbarMenu {
    ViewMode,
    TagFilter,
    Sort,
    Avatar,
}

/// Which half of a terminal pane a dragged tab is hovering over, deciding the
/// split direction when it is dropped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropZone {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnectDialogMode {
    Username,
    ChooseProtocol,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnectProtocol {
    Ssh,
    Telnet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceRuntimeTone {
    Live,
    Connecting,
    Error,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkspaceViewMode {
    #[default]
    Terminal,
    Files,
}
