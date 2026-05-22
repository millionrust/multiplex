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
    use std::ffi::CStr;

    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};

    type Id = *mut Object;

    unsafe extern "C" {
        fn object_setClass(obj: Id, cls: *const Class) -> *const Class;
    }

    /// Replacement `mouseDownCanMoveWindow` — always NO, so macOS never
    /// auto-drags the window from a mouse-down on this view.
    extern "C" fn no_window_drag(_: &Object, _: Sel) -> bool {
        false
    }

    /// Suffix appended to dynamically-created non-dragging subclasses.
    const NO_DRAG_SUFFIX: &str = "_TermiNoDrag";

    /// Reclass one view instance to a (cached) subclass of its current class
    /// that returns NO from `mouseDownCanMoveWindow`. The subclass adds no
    /// ivars, so the instance layout is unchanged and the swap is safe.
    /// Returns `true` if the view was (or already is) non-dragging.
    fn make_view_non_dragging(view: Id) -> bool {
        if view.is_null() {
            return false;
        }
        unsafe {
            let class: *const Class = msg_send![view, class];
            if class.is_null() {
                return false;
            }
            let class_name = CStr::from_ptr(objc::runtime::class_getName(&*class))
                .to_string_lossy()
                .into_owned();
            if class_name.ends_with(NO_DRAG_SUFFIX) {
                return true; // already patched
            }
            let subclass_name = format!("{class_name}{NO_DRAG_SUFFIX}");
            let subclass: *const Class = match Class::get(&subclass_name) {
                Some(existing) => existing,
                None => {
                    let Some(mut decl) = ClassDecl::new(&subclass_name, &*class) else {
                        eprintln!("[platform_mac] could not subclass {class_name}");
                        return false;
                    };
                    decl.add_method(
                        sel!(mouseDownCanMoveWindow),
                        no_window_drag as extern "C" fn(&Object, Sel) -> bool,
                    );
                    decl.register()
                }
            };
            object_setClass(view, subclass);
            true
        }
    }

    /// Recursively make `view` and every descendant non-dragging; returns the
    /// number of views patched.
    fn patch_view_tree(view: Id) -> usize {
        if view.is_null() {
            return 0;
        }
        let mut patched = 0;
        unsafe {
            let subviews: Id = msg_send![view, subviews];
            if !subviews.is_null() {
                let count: usize = msg_send![subviews, count];
                for index in 0..count {
                    let child: Id = msg_send![subviews, objectAtIndex: index];
                    patched += patch_view_tree(child);
                }
            }
        }
        if make_view_non_dragging(view) {
            patched += 1;
        }
        patched
    }

    /// Stop macOS from auto-dragging the window from the title-bar zone.
    ///
    /// We walk the whole window view tree from the theme frame down — title-bar
    /// views, content view, and GPUI's render view — and make every view report
    /// `mouseDownCanMoveWindow = NO`. The window stays `isMovable = YES` so our
    /// own [`start_window_drag`] can still move it.
    ///
    /// Call once after the first window is created.
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
            let mut patched = 0;
            for index in 0..count {
                let window: Id = msg_send![windows, objectAtIndex: index];
                if window.is_null() {
                    continue;
                }
                let content_view: Id = msg_send![window, contentView];
                if content_view.is_null() {
                    continue;
                }
                // The theme frame is the content view's superview; patching
                // from there also covers the native title-bar container.
                let theme_frame: Id = msg_send![content_view, superview];
                let root = if theme_frame.is_null() {
                    content_view
                } else {
                    theme_frame
                };
                patched += patch_view_tree(root);
            }
            eprintln!("[platform_mac] disabled OS window drag on {patched} views");
        }
    }

    /// Begin a native macOS window drag for the in-flight mouse-down event.
    pub fn start_window_drag() {
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
                let _: () = msg_send![window, performWindowDragWithEvent: event];
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
pub fn start_window_drag() {}
