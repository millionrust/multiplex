mod assets;
mod credentials;
mod local;
mod models;
mod sftp;
mod ssh;
mod storage;
mod terminal;
mod ui;

use gpui::*;
use gpui_component::Root;

use crate::storage::{load_local_ssh_hosts, load_saved_state};
use crate::ui::TermiRustApp;

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
        gpui_component::Theme::change(gpui_component::ThemeMode::Light, None, cx);

        let initial_state = saved_state.clone();
        let bounds = Bounds::centered(None, size(px(1480.), px(960.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("TermiRust".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| TermiRustApp::new(initial_state.clone(), window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .unwrap();

        cx.activate(true);
    });
}
