//! The background service's icon in the Windows notification area.
//!
//! The service has no window, so this is the one place a person can see that their PC is
//! reachable, who is looking at its screen, and switch the service off. It shows:
//!
//! - a tooltip and a first menu line saying whether a paired device is viewing or controlling
//!   the screen, and a notification the moment one starts, which is the thing nobody should
//!   learn about after the fact;
//! - **Open Multiplex**, which starts the app (the app then takes the route, as it always does);
//! - **Stop until next sign-in**, which ends the service; the `Run` value brings it back at the
//!   next logon, and the app's own setting removes it for good.
//!
//! Everything runs on one thread that owns a hidden window, because the notification area
//! reports clicks as window messages. The rest of the service talks to it through [`TrayHandle`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::thread;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, HICON, ICONINFO,
    KillTimer, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer, TPM_BOTTOMALIGN,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE,
    WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

use super::screen_sharing::{ScreenSharing, ScreenWatcher};

const CALLBACK_MESSAGE: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;
const REFRESH_TIMER: usize = 1;
const REFRESH_MS: u32 = 1_000;
const MENU_OPEN: usize = 1;
const MENU_STOP: usize = 2;
const ICON_PIXELS: u32 = 32;

/// The running tray. Dropping it takes the icon away; [`Self::stop_requested`] says whether the
/// person asked the service to stop.
pub struct TrayHandle {
    window: Arc<AtomicIsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TrayHandle {
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        let window = self.window.load(Ordering::Acquire);
        if window != 0 {
            // SAFETY: posting to a window that has since closed fails harmlessly.
            unsafe {
                PostMessageW(window as HWND, WM_CLOSE, 0, 0);
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Shows the icon on its own thread. A PC where the notification area cannot be reached, such as
/// a session with no desktop, simply has no icon; the service runs the same either way.
pub fn spawn(screens: ScreenSharing) -> TrayHandle {
    let window = Arc::new(AtomicIsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let window = window.clone();
        let stop = stop.clone();
        thread::Builder::new()
            .name("multiplex-tray".to_owned())
            .spawn(move || run(screens, &window, &stop))
            .ok()
    };
    TrayHandle {
        window,
        stop,
        thread,
    }
}

/// What the window procedure needs, owned by the tray thread.
struct TrayState {
    screens: ScreenSharing,
    stop: Arc<AtomicBool>,
    icon: HICON,
    taskbar_created: u32,
    last_watchers: Vec<String>,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<TrayState>> = const { std::cell::RefCell::new(None) };
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

/// Copies `value` into a fixed notification-area field, cut short to fit with its terminator.
fn fill(field: &mut [u16], value: &str) {
    let units: Vec<u16> = value.encode_utf16().take(field.len() - 1).collect();
    field[..units.len()].copy_from_slice(&units);
    field[units.len()] = 0;
}

fn run(screens: ScreenSharing, window_slot: &AtomicIsize, stop: &Arc<AtomicBool>) {
    let class_name = wide("MultiplexControllerServiceTray");
    // SAFETY: every pointer passed below is to a live local that outlives the call using it, and
    // the window, timer, and icon made here are all released before this function returns.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_procedure),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&class);
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if window.is_null() {
            return;
        }
        let icon = app_icon();
        let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        STATE.with(|state| {
            *state.borrow_mut() = Some(TrayState {
                screens,
                stop: stop.clone(),
                icon,
                taskbar_created,
                last_watchers: Vec::new(),
            });
        });
        add_icon(window);
        SetTimer(window, REFRESH_TIMER, REFRESH_MS, None);
        window_slot.store(window as isize, Ordering::Release);

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        window_slot.store(0, Ordering::Release);
        if !icon.is_null() {
            DestroyIcon(icon);
        }
        STATE.with(|state| state.borrow_mut().take());
    }
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_created = STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map_or(0, |state| state.taskbar_created)
    });
    match message {
        CALLBACK_MESSAGE => {
            match (lparam & 0xFFFF) as u32 {
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(window),
                WM_LBUTTONDBLCLK => open_app(),
                _ => {}
            }
            0
        }
        WM_TIMER if wparam == REFRESH_TIMER => {
            refresh(window);
            0
        }
        WM_CLOSE => {
            // SAFETY: the icon and timer belong to this window, which is being closed here.
            unsafe {
                KillTimer(window, REFRESH_TIMER);
                remove_icon(window);
                DestroyWindow(window);
            }
            0
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop, which is what closing the tray means.
            unsafe { PostQuitMessage(0) };
            0
        }
        // Explorer restarted and forgot every icon; put this one back.
        _ if taskbar_created != 0 && message == taskbar_created => {
            add_icon(window);
            0
        }
        // SAFETY: everything not handled above gets the system's default handling.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn describe(watchers: &[ScreenWatcher]) -> String {
    match watchers {
        [] => "Paired devices can reach this PC".to_owned(),
        [one] if one.controlling => format!("{} is controlling your screen", one.display_name),
        [one] => format!("{} is viewing your screen", one.display_name),
        many if many.iter().any(|watcher| watcher.controlling) => format!(
            "{} devices are viewing your screen, one controlling it",
            many.len()
        ),
        many => format!("{} devices are viewing your screen", many.len()),
    }
}

fn icon_data(window: HWND) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data for which all zeroes is a valid empty value.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = u32::try_from(size_of::<NOTIFYICONDATAW>()).expect("the struct is small");
    data.hWnd = window;
    data.uID = ICON_ID;
    data
}

fn add_icon(window: HWND) {
    let (icon, tip) = STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map_or((std::ptr::null_mut(), String::new()), |state| {
                (
                    state.icon,
                    format!("Multiplex: {}", describe(&state.screens.watchers())),
                )
            })
    });
    let mut data = icon_data(window);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = CALLBACK_MESSAGE;
    data.hIcon = icon;
    fill(&mut data.szTip, &tip);
    // SAFETY: `data` is fully initialised and lives for the call.
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &data);
    }
}

fn remove_icon(window: HWND) {
    let data = icon_data(window);
    // SAFETY: `data` identifies this window's icon and lives for the call.
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// Keeps the tooltip current and announces each device that starts watching.
fn refresh(window: HWND) {
    let Some((watchers, newcomers)) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state.as_mut()?;
        let watchers = state.screens.watchers();
        let names: Vec<String> = watchers.iter().map(|w| w.display_name.clone()).collect();
        let newcomers: Vec<String> = names
            .iter()
            .filter(|name| !state.last_watchers.contains(name))
            .cloned()
            .collect();
        state.last_watchers = names;
        Some((watchers, newcomers))
    }) else {
        return;
    };
    let mut data = icon_data(window);
    data.uFlags = NIF_TIP;
    fill(
        &mut data.szTip,
        &format!("Multiplex: {}", describe(&watchers)),
    );
    if let Some(name) = newcomers.first() {
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        fill(&mut data.szInfoTitle, "Multiplex");
        fill(
            &mut data.szInfo,
            &format!("{name} started viewing your screen."),
        );
    }
    // SAFETY: `data` identifies this window's icon and lives for the call.
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn show_menu(window: HWND) {
    let status = STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|state| describe(&state.screens.watchers()))
            .unwrap_or_default()
    });
    // SAFETY: the menu is created, shown, and destroyed here; the strings outlive the calls that
    // read them. Bringing the window forward first is what lets the menu close when the person
    // clicks elsewhere, as Windows documents for notification-area menus.
    let chosen = unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let status = wide(&status);
        let open = wide("Open Multiplex");
        let stop = wide("Stop until next sign-in");
        AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, status.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_OPEN, open.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_STOP, stop.as_ptr());
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        SetForegroundWindow(window);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            cursor.x,
            cursor.y,
            0,
            window,
            std::ptr::null(),
        );
        DestroyMenu(menu);
        chosen
    };
    match usize::try_from(chosen).unwrap_or(0) {
        MENU_OPEN => open_app(),
        MENU_STOP => {
            STATE.with(|state| {
                if let Some(state) = state.borrow().as_ref() {
                    state.stop.store(true, Ordering::Release);
                }
            });
            // SAFETY: closing this thread's own window.
            unsafe {
                PostMessageW(window, WM_CLOSE, 0, 0);
            }
        }
        _ => {}
    }
}

/// Starts the app, which is this same executable run without arguments.
fn open_app() {
    use std::os::windows::process::CommandExt as _;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

    if let Ok(executable) = std::env::current_exe() {
        let _ = std::process::Command::new(executable)
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn();
    }
}

/// The app icon at notification-area size, built from the PNG the app already embeds. A null
/// icon, if decoding ever failed, leaves the system's blank one.
fn app_icon() -> HICON {
    let Ok(image) = image::load_from_memory(include_bytes!("../../assets/icons/app.png")) else {
        return std::ptr::null_mut();
    };
    let image = image
        .resize_exact(
            ICON_PIXELS,
            ICON_PIXELS,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    // Windows wants BGRA, and an icon's colour bitmap carries its own alpha.
    let mut bgra = image.into_raw();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let size = i32::try_from(ICON_PIXELS).expect("icon size fits");
    let mask = vec![0_u8; (ICON_PIXELS * ICON_PIXELS / 8) as usize];
    // SAFETY: both buffers hold exactly the bits a bitmap of this size and depth reads, and the
    // bitmaps are deleted once the icon has copied them.
    unsafe {
        let color = CreateBitmap(size, size, 1, 32, bgra.as_ptr().cast());
        let mask = CreateBitmap(size, size, 1, 1, mask.as_ptr().cast());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watcher(name: &str, controlling: bool) -> ScreenWatcher {
        ScreenWatcher {
            device_id: multiplex_domain::ControllerDeviceId::new(),
            display_name: name.to_owned(),
            controlling,
        }
    }

    #[test]
    fn the_status_line_says_who_is_watching() {
        assert_eq!(describe(&[]), "Paired devices can reach this PC");
        assert_eq!(
            describe(&[watcher("Phone", false)]),
            "Phone is viewing your screen"
        );
        assert_eq!(
            describe(&[watcher("Phone", true)]),
            "Phone is controlling your screen"
        );
        assert_eq!(
            describe(&[watcher("Phone", true), watcher("Laptop", false)]),
            "2 devices are viewing your screen, one controlling it"
        );
    }

    #[test]
    fn long_text_is_cut_to_fit_its_field() {
        let mut field = [1_u16; 8];
        fill(&mut field, "a much longer tooltip");
        assert_eq!(field[7], 0);
        assert_eq!(String::from_utf16_lossy(&field[..7]), "a much ");
    }

    #[test]
    fn the_app_icon_decodes() {
        assert!(!app_icon().is_null());
    }
}
