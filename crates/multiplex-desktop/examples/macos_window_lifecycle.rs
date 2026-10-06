//! Native AppKit regression probe; headless GPUI tests do not create title-bar decorations.
//! Run with `cargo run -p multiplex --example macos_window_lifecycle` on macOS.
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/platform_mac.rs"]
mod platform_mac;

#[cfg(target_os = "macos")]
fn main() {
    use gpui::{
        App, AppContext as _, Application, Context, Render, TitlebarOptions, Window, WindowOptions,
        div,
    };
    use objc::declare::ClassDecl;
    use objc::rc::StrongPtr;
    use objc::runtime::{Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};
    use std::time::Duration;

    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            div()
        }
    }
    extern "C" fn observe(
        _: &Object,
        _: Sel,
        _: *mut Object,
        _: *mut Object,
        _: *mut Object,
        _: *mut std::ffi::c_void,
    ) {
    }
    unsafe fn classes(view: *mut Object, result: &mut Vec<(usize, usize)>) {
        unsafe {
            if view.is_null() {
                return;
            }
            result.push((view as usize, objc::runtime::object_getClass(view) as usize));
            let children: *mut Object = msg_send![view, subviews];
            let count: usize = msg_send![children, count];
            for i in 0..count {
                classes(msg_send![children, objectAtIndex: i], result);
            }
        }
    }
    Application::new().run(|cx| {
        cx.spawn(async |cx| {
            for _ in 0..20 {
                let handle = cx.update(|cx| cx.open_window(WindowOptions {
                    show: false,
                    focus: false,
                    titlebar: Some(TitlebarOptions { appears_transparent: true, ..Default::default() }),
                    ..Default::default()
                }, |_, cx| cx.new(|_| Empty))).unwrap().unwrap();
                let (content, observer, key) = cx.update(|_: &mut App| unsafe {
                    let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
                    let windows: *mut Object = msg_send![app, windows];
                    let window: *mut Object = msg_send![windows, objectAtIndex: 0usize];
                    let content: *mut Object = msg_send![window, contentView];
                    let content = StrongPtr::retain(content);
                    let cls = objc::runtime::Class::get("MultiplexWindowProbeObserver").unwrap_or_else(|| {
                        let mut decl = ClassDecl::new("MultiplexWindowProbeObserver", class!(NSObject)).unwrap();
                        decl.add_method(sel!(observeValueForKeyPath:ofObject:change:context:), observe as extern "C" fn(&Object, Sel, *mut Object, *mut Object, *mut Object, *mut std::ffi::c_void));
                        decl.register()
                    });
                    let observer: *mut Object = msg_send![cls, new];
                    let observer = StrongPtr::new(observer);
                    let key: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c"frame".as_ptr()];
                    let key = StrongPtr::retain(key);
                    let _: () = msg_send![*content, addObserver: *observer forKeyPath: *key options: 0usize context: std::ptr::null_mut::<std::ffi::c_void>()];
                    let root: *mut Object = msg_send![*content, superview];
                    let mut before = Vec::new();
                    classes(root, &mut before);
                    platform_mac::disable_titlebar_window_drag();
                    let mut after = Vec::new();
                    classes(root, &mut after);
                    assert_eq!(before, after, "drag customization must preserve AppKit and KVO classes");
                    let movable: bool = msg_send![window, isMovable];
                    assert!(!movable, "title-bar tabs must not initiate OS dragging");
                    (content, observer, key)
                }).unwrap();
                for _ in 0..4 {
                    handle.update(cx, |_, window, _| window.zoom_window()).unwrap();
                    cx.background_executor().timer(Duration::from_millis(20)).await;
                }
                cx.update(|_| unsafe {
                    let _: () = msg_send![*content, removeObserver: *observer forKeyPath: *key];
                }).unwrap();
                drop((content, observer, key));
                handle.update(cx, |_, window, _| window.remove_window()).unwrap();
                cx.background_executor().timer(Duration::from_millis(20)).await;
            }
            println!("PASS: 20 native windows, 80 zoom cycles, preserved view classes, clean KVO teardown");
            cx.update(|cx| cx.quit()).unwrap();
        }).detach();
    });
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("macOS-only native window lifecycle probe");
}
