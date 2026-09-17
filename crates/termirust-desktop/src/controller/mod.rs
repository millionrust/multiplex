#![allow(dead_code)]

pub mod background_service;
pub mod devices;
pub mod host_identity;
pub mod lan;
pub mod pairing;
pub mod relay_desktop;
pub mod relay_host;
pub mod relay_host_service;
pub mod route_coordinator;
pub mod screen_sharing;
pub mod ssh_pairing;
pub mod tailscale;
pub mod watch_session;
pub mod watched;

use std::path::Path;

use termirust_controller_listener::{
    DesktopPaneBridgeEndpoint, RepositoryBridgeSources, TmuxSessionSource,
};

/// Where the desktop app's live pane bridge listens, under the Controller runtime parent.
pub fn desktop_pane_bridge_root(runtime_parent: &Path) -> std::path::PathBuf {
    runtime_parent.join("desktop-pane-bridge")
}

/// The live sessions a Controller bridge process offers: the running desktop app's panes,
/// when it published them, tmux sessions, and this computer's screens, each when the user
/// turned that sharing on. Read at each connection so a settings change applies to the next
/// device that connects.
pub fn remote_bridge_sources(runtime_parent: &Path) -> RepositoryBridgeSources {
    let settings = crate::storage::load_saved_state().map(|state| state.settings);
    bridge_sources(
        runtime_parent,
        settings
            .as_ref()
            .map(|settings| settings.remote_tmux_sessions)
            .unwrap_or(false),
        settings
            .as_ref()
            .map(|settings| settings.remote_screen_sharing)
            .unwrap_or(false),
    )
}

fn bridge_sources(
    runtime_parent: &Path,
    tmux_sessions: bool,
    screen_sharing: bool,
) -> RepositoryBridgeSources {
    RepositoryBridgeSources {
        desktop_pane_bridge: DesktopPaneBridgeEndpoint::discover(&desktop_pane_bridge_root(
            runtime_parent,
        )),
        tmux_sessions: tmux_sessions
            .then(|| TmuxSessionSource::system(runtime_parent).ok())
            .flatten(),
        screens: screen_sharing.then(|| {
            std::sync::Arc::new(screen_sharing::ScreenSharing::enabled())
                as std::sync::Arc<dyn termirust_controller_listener::ScreenSessionFactory>
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_controller_listener::{DesktopPaneBridgeServer, DesktopPaneRegistry};

    // The desktop pane bridge listens on a Unix socket, which Windows has none of.
    #[cfg(unix)]
    #[test]
    fn bridge_sources_follow_the_published_bridge_and_the_sharing_setting() {
        // A short parent keeps the Unix socket paths under it within the length the platform
        // allows, which the usual temporary directory on macOS is too deep for. Windows has no
        // /tmp and no such limit.
        let short_parent = if cfg!(windows) {
            std::env::temp_dir()
        } else {
            std::path::PathBuf::from("/tmp")
        };
        let fixture = tempfile::Builder::new()
            .prefix("tr-src-")
            .tempdir_in(short_parent)
            .unwrap();
        let runtime_parent = fixture.path();
        let none = bridge_sources(runtime_parent, false, false);
        assert!(none.desktop_pane_bridge.is_none());
        assert!(none.tmux_sessions.is_none());
        assert!(none.screens.is_none(), "screens are shared only on request");

        let mut server = DesktopPaneBridgeServer::start(
            desktop_pane_bridge_root(runtime_parent),
            DesktopPaneRegistry::default(),
        )
        .unwrap();
        server.publish().unwrap();
        let both = bridge_sources(runtime_parent, true, true);
        assert!(both.desktop_pane_bridge == Some(server.endpoint()));
        assert!(both.tmux_sessions.is_some());
        assert!(both.screens.is_some());

        drop(server);
        assert!(
            bridge_sources(runtime_parent, true, false)
                .desktop_pane_bridge
                .is_none()
        );
    }
}
