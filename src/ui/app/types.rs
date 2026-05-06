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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TerminalPanelTab {
    Quick,
    Snippets,
    History,
    Themes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnectDialogMode {
    Username,
    ChooseProtocol,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnectProtocol {
    Ssh,
    Mosh,
    Telnet,
}

#[allow(dead_code)]
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
