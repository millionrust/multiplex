#![allow(dead_code)]

pub mod devices;
pub mod host_identity;
pub mod lan;
pub mod pairing;
pub mod relay_desktop;
pub mod relay_host;
pub mod relay_host_service;
pub mod route_coordinator;
pub mod ssh_pairing;

use std::path::Path;

use termirust_controller_listener::{
    DesktopPaneBridgeEndpoint, RepositoryBridgeSources, TmuxSessionSource,
};

/// Where the desktop app's live pane bridge listens, under the Controller runtime parent.
pub fn desktop_pane_bridge_root(runtime_parent: &Path) -> std::path::PathBuf {
    runtime_parent.join("desktop-pane-bridge")
}

/// The live sessions a Controller bridge process offers: the running desktop app's panes,
/// when it published them, and tmux sessions, when the user turned sharing on. Read at each
/// connection so a settings change applies to the next device that connects.
pub fn remote_bridge_sources(runtime_parent: &Path) -> RepositoryBridgeSources {
    bridge_sources(
        runtime_parent,
        crate::storage::load_saved_state()
            .map(|state| state.settings.remote_tmux_sessions)
            .unwrap_or(false),
    )
}

fn bridge_sources(runtime_parent: &Path, tmux_sessions: bool) -> RepositoryBridgeSources {
    RepositoryBridgeSources {
        desktop_pane_bridge: DesktopPaneBridgeEndpoint::discover(&desktop_pane_bridge_root(
            runtime_parent,
        )),
        tmux_sessions: tmux_sessions
            .then(|| TmuxSessionSource::system(runtime_parent).ok())
            .flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_controller_listener::{DesktopPaneBridgeServer, DesktopPaneRegistry};

    #[test]
    fn bridge_sources_follow_the_published_bridge_and_the_sharing_setting() {
        let fixture = tempfile::Builder::new()
            .prefix("tr-src-")
            .tempdir_in("/tmp")
            .unwrap();
        let runtime_parent = fixture.path();
        let none = bridge_sources(runtime_parent, false);
        assert!(none.desktop_pane_bridge.is_none());
        assert!(none.tmux_sessions.is_none());

        let mut server = DesktopPaneBridgeServer::start(
            desktop_pane_bridge_root(runtime_parent),
            DesktopPaneRegistry::default(),
        )
        .unwrap();
        server.publish().unwrap();
        let both = bridge_sources(runtime_parent, true);
        assert!(both.desktop_pane_bridge == Some(server.endpoint()));
        assert!(both.tmux_sessions.is_some());

        drop(server);
        assert!(
            bridge_sources(runtime_parent, true)
                .desktop_pane_bridge
                .is_none()
        );
    }
}
