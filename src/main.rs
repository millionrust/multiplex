mod assets;
mod credentials;
mod local;
mod models;
mod platform_mac;
mod sftp;
mod ssh;
mod storage;
mod terminal;
mod ui;

use gpui::*;
use gpui_component::Root;

use crate::models::SavedWindowBounds;
use crate::storage::{load_local_ssh_hosts, load_saved_state};
use crate::ui::TermiRustApp;

/// Resolve the bounds to open the window at. Restores the last saved frame,
/// but only if it still lands on a connected display — and clamps it fully
/// on-screen so a saved position can never strand the window off the visible
/// area (e.g. after a monitor is disconnected or rearranged). Falls back to
/// centered on the primary display.
fn restored_window_bounds(saved: Option<SavedWindowBounds>, cx: &App) -> Bounds<Pixels> {
    let fallback = || Bounds::centered(None, size(px(1480.), px(960.)), cx);

    let Some(saved) = saved else {
        return fallback();
    };
    if saved.width < 200.0 || saved.height < 200.0 {
        return fallback();
    }

    let (wx, wy) = (saved.x, saved.y);
    let (mut ww, mut wh) = (saved.width, saved.height);

    // Area of the saved window that overlaps a display rect.
    let overlap = |dx: f32, dy: f32, dw: f32, dh: f32| -> f32 {
        let ix = (wx + ww).min(dx + dw) - wx.max(dx);
        let iy = (wy + wh).min(dy + dh) - wy.max(dy);
        if ix > 0.0 && iy > 0.0 { ix * iy } else { 0.0 }
    };

    // Pick the display the saved window overlaps the most.
    let display = cx
        .displays()
        .into_iter()
        .map(|display| {
            let bounds = display.bounds();
            (
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            )
        })
        .filter(|&(dx, dy, dw, dh)| overlap(dx, dy, dw, dh) > 0.0)
        .max_by(|a, b| overlap(a.0, a.1, a.2, a.3).total_cmp(&overlap(b.0, b.1, b.2, b.3)));

    let Some((dx, dy, dw, dh)) = display else {
        return fallback();
    };

    // Shrink to fit the display, then clamp the window fully on-screen.
    ww = ww.min(dw);
    wh = wh.min(dh);
    let x = wx.clamp(dx, dx + dw - ww);
    let y = wy.clamp(dy, dy + dh - wh);

    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(ww), px(wh)),
    }
}

fn main() {
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
        let bounds = restored_window_bounds(saved_state.window_bounds, cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
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
