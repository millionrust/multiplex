mod agents;
mod artifact_preview;
mod assets;
mod controller;
mod credentials;
mod local;
mod models;
mod platform_mac;
mod platform_notifications;
mod platform_open_url;
mod sftp;
mod ssh;
mod storage;
mod terminal;
#[cfg(test)]
mod test_support;
mod ui;
mod worktree_launch;

use gpui::*;
use gpui_component::Root;

use crate::models::SavedWindowBounds;
use crate::storage::{load_local_ssh_hosts, load_saved_state};
use crate::ui::TermiRustApp;

const SESSION_HOST_MODE: &str = "--session-host";
const ARTIFACT_PREVIEW_MODE: &str = "--artifact-preview-worker";

fn run_session_host_mode() -> Result<(), termirust_session_host::HostError> {
    use std::io::Write as _;

    if !termirust_session_host::stdin_is_pipe()? {
        return Err(termirust_session_host::HostError::new(
            termirust_session_host::HostErrorCode::PermissionDenied,
        ));
    }
    let descriptor = termirust_session_host::LaunchDescriptor::read(std::io::stdin().lock())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(termirust_session_host::HostError::io)?;
    runtime.block_on(async move {
        let host = termirust_session_host::start(descriptor).await?;
        writeln!(
            std::io::stdout(),
            "{{\"schema_version\":1,\"lifecycle\":\"ready\",\"code\":\"host_ready\"}}"
        )
        .map_err(termirust_session_host::HostError::io)?;
        std::io::stdout()
            .flush()
            .map_err(termirust_session_host::HostError::io)?;
        host.wait().await
    })
}

/// Resolve the bounds and display to open the window at. Restores the last
/// saved frame on the display it was saved on; if that display is no longer
/// connected, the saved coordinates are meaningless, so it centers on the
/// primary display instead.
fn restored_window_bounds(
    saved: Option<SavedWindowBounds>,
    cx: &App,
) -> (Bounds<Pixels>, Option<DisplayId>) {
    let centered = || (Bounds::centered(None, size(px(1480.), px(960.)), cx), None);

    for display in cx.displays() {
        let b = display.bounds();
        eprintln!(
            "[main] display id={} origin=({:.0},{:.0}) size=({:.0}x{:.0})",
            u32::from(display.id()),
            f32::from(b.origin.x),
            f32::from(b.origin.y),
            f32::from(b.size.width),
            f32::from(b.size.height),
        );
    }
    eprintln!(
        "[main] primary display id={:?}",
        cx.primary_display().map(|d| u32::from(d.id()))
    );
    eprintln!("[main] saved window_bounds = {saved:?}");

    let Some(saved) = saved else {
        return centered();
    };
    if saved.width < 200.0 || saved.height < 200.0 {
        return centered();
    }

    // Re-bind the saved display by id. macOS needs the display passed
    // explicitly, otherwise a position on a secondary monitor is mis-mapped
    // onto the primary one. If the display is gone, center instead.
    let display_id = match saved.display_id {
        Some(saved_id) => {
            match cx
                .displays()
                .into_iter()
                .find(|display| u32::from(display.id()) == saved_id)
            {
                Some(display) => Some(display.id()),
                None => return centered(),
            }
        }
        None => None,
    };

    let bounds = Bounds::new(
        point(px(saved.x), px(saved.y)),
        size(px(saved.width), px(saved.height)),
    );
    eprintln!("[main] restoring at {bounds:?} display_id={display_id:?}");
    (bounds, display_id)
}

/// Redirect stderr — every `eprintln!` line and panic backtrace — to a log
/// file in the app data directory, so logs are available without a terminal.
/// The file is truncated on each launch.
fn init_file_logging() {
    let Some(data_dir) = dirs::data_dir() else {
        return;
    };
    let log_dir = data_dir.join("termirust");
    if std::fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    let log_path = log_dir.join("termirust.log");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
    else {
        return;
    };
    eprintln!("[main] logging to {}", log_path.display());
    // Point fd 2 at the log file; all eprintln! output follows it from here.
    unsafe {
        libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&file), libc::STDERR_FILENO);
    }
    std::mem::forget(file);
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some(ARTIFACT_PREVIEW_MODE) {
        std::process::exit(crate::artifact_preview::run_worker_mode());
    }
    if std::env::args().nth(1).as_deref() == Some(SESSION_HOST_MODE) {
        if let Err(error) = run_session_host_mode() {
            eprintln!(
                "{{\"schema_version\":1,\"lifecycle\":\"failed\",\"code\":\"{}\",\"io_kind\":\"{:?}\"}}",
                error.stable_code(),
                error.io_kind
            );
            std::process::exit(1);
        }
        return;
    }
    init_file_logging();

    std::panic::set_hook(Box::new(|info| {
        eprintln!("=== PANIC ===");
        eprintln!("{info}");
        if let Some(location) = info.location() {
            eprintln!(
                "  at {}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            );
        }
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("{bt}");
        eprintln!("=== END PANIC ===");
    }));

    eprintln!("[main] starting termirust...");
    let mut saved_state = load_saved_state().unwrap_or_default();
    if let Ok(imported_hosts) = load_local_ssh_hosts() {
        saved_state.merge_imported_profiles(imported_hosts);
    }
    let app = Application::new().with_assets(crate::assets::Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);

        let initial_state = saved_state.clone();
        let (bounds, restore_display_id) = restored_window_bounds(saved_state.window_bounds, cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                display_id: restore_display_id,
                titlebar: Some(TitlebarOptions {
                    title: Some("TermiRust".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(-200.), px(8.))),
                }),
                window_min_size: Some(size(px(1120.), px(720.))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| TermiRustApp::new(initial_state.clone(), window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .unwrap();

        // Take ownership of window dragging from the OS so the chrome tabs
        // (which sit in the title-bar zone) stay draggable.
        crate::platform_mac::disable_titlebar_window_drag();

        cx.activate(true);
    });
}
