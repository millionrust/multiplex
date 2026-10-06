//! macOS window-control interop.
//!
//! GPUI 0.2.2 has no per-element window-drag control on macOS — the OS drags
//! the window from the whole title-bar zone, which would hijack any drag that
//! starts on the chrome tabs. So we take ownership: stop the OS from
//! auto-dragging the window, and start a native drag explicitly only from the
//! chrome areas that should move the window.
//!
//! On non-macOS targets these are no-ops (GPUI handles dragging there).

#[cfg(target_os = "macos")]
mod imp {
    use std::time::Duration;

    use objc::rc::StrongPtr;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    type Id = *mut Object;

    /// The foreground task retains its window until the Window Server finishes the drag.
    /// Restore automatic-drag prevention even if the task is cancelled during shutdown.
    struct NativeDrag(StrongPtr);

    impl Drop for NativeDrag {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![*self.0, setMovable: false];
            }
        }
    }

    /// Stop automatic title-bar dragging without changing AppKit view classes.
    ///
    /// AppKit observes its title-bar views with KVO. Replacing their runtime classes can
    /// discard Foundation's observer subclass and crash when a decoration is released.
    /// The public window property leaves those view lifetimes and observers intact.
    pub fn disable_titlebar_window_drag() {
        unsafe {
            let app: Id = msg_send![class!(NSApplication), sharedApplication];
            if app.is_null() {
                return;
            }
            let windows: Id = msg_send![app, windows];
            if windows.is_null() {
                return;
            }
            let count: usize = msg_send![windows, count];
            for index in 0..count {
                let window: Id = msg_send![windows, objectAtIndex: index];
                if !window.is_null() {
                    let _: () = msg_send![window, setMovableByWindowBackground: false];
                    let _: () = msg_send![window, setMovable: false];
                }
            }
        }
    }

    /// Begin a native macOS window drag for the in-flight mouse-down event.
    pub fn start_window_drag(cx: &mut gpui::App) {
        unsafe {
            let app: Id = msg_send![class!(NSApplication), sharedApplication];
            if app.is_null() {
                return;
            }
            let event: Id = msg_send![app, currentEvent];
            if event.is_null() {
                return;
            }
            // Prefer the window the event was delivered to; fall back to key.
            let mut window: Id = msg_send![event, window];
            if window.is_null() {
                window = msg_send![app, keyWindow];
            }
            if !window.is_null() {
                let drag = NativeDrag(StrongPtr::retain(window));
                let _: () = msg_send![window, setMovable: true];
                let _: () = msg_send![window, performWindowDragWithEvent: event];
                // performWindowDragWithEvent returns immediately and may consume mouse-up.
                // Poll on GPUI's main-thread executor rather than relying on a GPUI mouse-up.
                cx.spawn(async move |cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(16))
                            .await;
                        let pressed: usize = msg_send![class!(NSEvent), pressedMouseButtons];
                        if pressed & 1 == 0 {
                            break;
                        }
                    }
                    drop(drag);
                })
                .detach();
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::{disable_titlebar_window_drag, start_window_drag};

/// Stop the OS from auto-dragging the window from the title-bar zone.
#[cfg(not(target_os = "macos"))]
pub fn disable_titlebar_window_drag() {}

/// Begin a native window drag (handled by GPUI's `start_window_move` elsewhere
/// on non-macOS platforms).
#[cfg(not(target_os = "macos"))]
pub fn start_window_drag(_: &mut gpui::App) {}
