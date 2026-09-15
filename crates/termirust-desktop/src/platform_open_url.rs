use std::fmt;

use termirust_domain::{LocalDevUrl, OpenUrlError};

pub trait PlatformOpenUrl: Send {
    fn open(&mut self, url: &LocalDevUrl) -> Result<(), OpenUrlError>;
}

pub fn system_platform_open_url() -> Box<dyn PlatformOpenUrl> {
    #[cfg(test)]
    {
        Box::new(UnavailablePlatformOpenUrl)
    }
    #[cfg(not(test))]
    {
        native::system()
    }
}

#[cfg(any(
    test,
    not(any(target_os = "macos", target_os = "linux", target_os = "windows"))
))]
struct UnavailablePlatformOpenUrl;

#[cfg(any(
    test,
    not(any(target_os = "macos", target_os = "linux", target_os = "windows"))
))]
impl PlatformOpenUrl for UnavailablePlatformOpenUrl {
    fn open(&mut self, _url: &LocalDevUrl) -> Result<(), OpenUrlError> {
        Err(OpenUrlError::BrowserUnavailable)
    }
}

impl fmt::Debug for dyn PlatformOpenUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PlatformOpenUrl(<redacted>)")
    }
}

#[cfg(all(target_os = "macos", not(test)))]
#[allow(unexpected_cfgs)]
mod native {
    use std::ffi::CString;

    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    use super::*;

    pub(super) struct MacPlatformOpenUrl;

    pub(super) fn system() -> Box<dyn PlatformOpenUrl> {
        Box::new(MacPlatformOpenUrl)
    }

    impl PlatformOpenUrl for MacPlatformOpenUrl {
        fn open(&mut self, url: &LocalDevUrl) -> Result<(), OpenUrlError> {
            let workspace_class =
                Class::get("NSWorkspace").ok_or(OpenUrlError::BrowserUnavailable)?;
            let url_class = Class::get("NSURL").ok_or(OpenUrlError::BrowserUnavailable)?;
            let string_class = Class::get("NSString").ok_or(OpenUrlError::BrowserUnavailable)?;
            let value = CString::new(url.as_str()).map_err(|_| OpenUrlError::Invalidated)?;
            let string: *mut Object =
                unsafe { msg_send![string_class, stringWithUTF8String: value.as_ptr()] };
            if string.is_null() {
                return Err(OpenUrlError::DispatchFailed);
            }
            let native_url: *mut Object = unsafe { msg_send![url_class, URLWithString: string] };
            if native_url.is_null() {
                return Err(OpenUrlError::Invalidated);
            }
            let workspace: *mut Object = unsafe { msg_send![workspace_class, sharedWorkspace] };
            if workspace.is_null() {
                return Err(OpenUrlError::BrowserUnavailable);
            }
            let opened: bool = unsafe { msg_send![workspace, openURL: native_url] };
            if opened {
                Ok(())
            } else {
                Err(OpenUrlError::DispatchFailed)
            }
        }
    }
}

#[cfg(all(target_os = "linux", not(test)))]
mod native {
    use std::io;
    use std::process::{Command, Stdio};

    use super::*;

    pub(super) struct LinuxPlatformOpenUrl;

    pub(super) fn system() -> Box<dyn PlatformOpenUrl> {
        Box::new(LinuxPlatformOpenUrl)
    }

    impl PlatformOpenUrl for LinuxPlatformOpenUrl {
        fn open(&mut self, url: &LocalDevUrl) -> Result<(), OpenUrlError> {
            Command::new("xdg-open")
                .arg(url.as_str())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(|_| ())
                .map_err(map_spawn_error)
        }
    }

    fn map_spawn_error(error: io::Error) -> OpenUrlError {
        match error.kind() {
            io::ErrorKind::NotFound => OpenUrlError::BrowserUnavailable,
            io::ErrorKind::PermissionDenied => OpenUrlError::PermissionDenied,
            _ => OpenUrlError::DispatchFailed,
        }
    }
}

#[cfg(all(target_os = "windows", not(test)))]
mod native {
    use std::io;
    use std::process::{Command, Stdio};

    use super::*;

    pub(super) struct WindowsPlatformOpenUrl;

    pub(super) fn system() -> Box<dyn PlatformOpenUrl> {
        Box::new(WindowsPlatformOpenUrl)
    }

    impl PlatformOpenUrl for WindowsPlatformOpenUrl {
        fn open(&mut self, url: &LocalDevUrl) -> Result<(), OpenUrlError> {
            Command::new("explorer.exe")
                .arg(url.as_str())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(|_| ())
                .map_err(map_spawn_error)
        }
    }

    fn map_spawn_error(error: io::Error) -> OpenUrlError {
        match error.kind() {
            io::ErrorKind::NotFound => OpenUrlError::BrowserUnavailable,
            io::ErrorKind::PermissionDenied => OpenUrlError::PermissionDenied,
            _ => OpenUrlError::DispatchFailed,
        }
    }
}

#[cfg(all(
    not(test),
    not(any(target_os = "macos", target_os = "linux", target_os = "windows"))
))]
mod native {
    use super::*;

    pub(super) fn system() -> Box<dyn PlatformOpenUrl> {
        Box::new(UnavailablePlatformOpenUrl)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct RecordingPlatformOpenUrl {
        calls: Arc<Mutex<Vec<String>>>,
        result: Result<(), OpenUrlError>,
    }

    impl PlatformOpenUrl for RecordingPlatformOpenUrl {
        fn open(&mut self, url: &LocalDevUrl) -> Result<(), OpenUrlError> {
            self.calls.lock().unwrap().push(url.as_str().to_string());
            self.result
        }
    }

    #[test]
    fn platform_open_url_dispatches_one_exact_revalidated_value() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut platform = RecordingPlatformOpenUrl {
            calls: Arc::clone(&calls),
            result: Ok(()),
        };
        let url = LocalDevUrl::parse("http://localhost:3000/path?secret=x").unwrap();
        platform.open(&url).unwrap();
        assert_eq!(calls.lock().unwrap().as_slice(), [url.as_str()]);
    }

    #[test]
    fn platform_open_url_failure_has_no_retry_or_sensitive_debug_content() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut platform = RecordingPlatformOpenUrl {
            calls: Arc::clone(&calls),
            result: Err(OpenUrlError::PermissionDenied),
        };
        let url = LocalDevUrl::parse("http://localhost:3000/private?canary-secret").unwrap();
        assert_eq!(platform.open(&url), Err(OpenUrlError::PermissionDenied));
        assert_eq!(calls.lock().unwrap().len(), 1);
        let debug = format!("{:?}", &platform as &dyn PlatformOpenUrl);
        assert!(!debug.contains("canary-secret"));
    }
}
