use std::fmt;

use termirust_domain::PermissionState;

pub const MAX_PLATFORM_TITLE_SCALARS: usize = 160;
pub const MAX_PLATFORM_BODY_SCALARS: usize = 160;
pub const MAX_PLATFORM_IDENTIFIER_BYTES: usize = 96;

#[derive(Clone, Eq, PartialEq)]
pub struct PlatformNotificationRequest {
    identifier: String,
    title: String,
    body: String,
}

impl PlatformNotificationRequest {
    pub fn new(
        identifier: &str,
        title: &str,
        body: &str,
    ) -> Result<Self, PlatformNotificationError> {
        if identifier.is_empty()
            || identifier.len() > MAX_PLATFORM_IDENTIFIER_BYTES
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !valid_text(title, MAX_PLATFORM_TITLE_SCALARS)
            || !valid_text(body, MAX_PLATFORM_BODY_SCALARS)
        {
            return Err(PlatformNotificationError::InvalidPayload);
        }
        Ok(Self {
            identifier: identifier.to_string(),
            title: title.to_string(),
            body: body.to_string(),
        })
    }

    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn body(&self) -> &str {
        &self.body
    }
}

impl fmt::Debug for PlatformNotificationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlatformNotificationRequest")
            .field("identifier", &"<opaque>")
            .field("title", &"<redacted>")
            .field("body", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformNotificationError {
    InvalidPayload,
    PermissionDenied,
    Unavailable,
    DeliveryFailed,
}

pub trait PlatformNotifications: Send {
    fn query_permission(&self) -> PermissionState;
    fn request_permission(&mut self) -> PermissionState;
    fn deliver(
        &mut self,
        request: &PlatformNotificationRequest,
    ) -> Result<(), PlatformNotificationError>;
    fn remove(&mut self, identifier: &str) -> Result<(), PlatformNotificationError>;
    fn take_activation(&mut self) -> Option<String>;
}

pub fn system_platform_notifications() -> Box<dyn PlatformNotifications> {
    #[cfg(test)]
    {
        Box::new(TestPlatformNotifications)
    }
    #[cfg(not(test))]
    native::system()
}

#[cfg(test)]
struct TestPlatformNotifications;

#[cfg(test)]
impl PlatformNotifications for TestPlatformNotifications {
    fn query_permission(&self) -> PermissionState {
        PermissionState::Unavailable
    }

    fn request_permission(&mut self) -> PermissionState {
        PermissionState::Unavailable
    }

    fn deliver(
        &mut self,
        _request: &PlatformNotificationRequest,
    ) -> Result<(), PlatformNotificationError> {
        Err(PlatformNotificationError::Unavailable)
    }

    fn remove(&mut self, _identifier: &str) -> Result<(), PlatformNotificationError> {
        Err(PlatformNotificationError::Unavailable)
    }

    fn take_activation(&mut self) -> Option<String> {
        None
    }
}

fn valid_text(value: &str, max_scalars: usize) -> bool {
    let count = value.chars().count();
    count > 0 && count <= max_scalars && !value.chars().any(char::is_control)
}

#[cfg(all(target_os = "macos", not(test)))]
#[allow(unexpected_cfgs)]
mod native {
    use std::collections::VecDeque;
    use std::ffi::CStr;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Mutex, OnceLock};

    use block::{Block, ConcreteBlock};
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Protocol, Sel};
    use objc::{msg_send, sel, sel_impl};

    use super::*;

    #[link(name = "UserNotifications", kind = "framework")]
    unsafe extern "C" {}

    pub(super) struct NativePlatformNotifications;

    pub(super) fn system() -> Box<dyn PlatformNotifications> {
        install_delegate();
        Box::new(NativePlatformNotifications)
    }

    impl PlatformNotifications for NativePlatformNotifications {
        fn query_permission(&self) -> PermissionState {
            let Some(center) = notification_center() else {
                return PermissionState::Unavailable;
            };
            let completion = ConcreteBlock::new(move |settings: *mut Object| {
                let state = if settings.is_null() {
                    PermissionState::Unavailable
                } else {
                    let status: isize = unsafe { msg_send![settings, authorizationStatus] };
                    permission_from_status(status)
                };
                cache_permission(state);
            })
            .copy();
            unsafe {
                let _: () =
                    msg_send![center, getNotificationSettingsWithCompletionHandler: &*completion];
            }
            cached_permission()
        }

        fn request_permission(&mut self) -> PermissionState {
            let Some(center) = notification_center() else {
                return PermissionState::Unavailable;
            };
            let completion = ConcreteBlock::new(move |granted: bool, error: *mut Object| {
                let state = if !error.is_null() {
                    PermissionState::Unavailable
                } else if granted {
                    PermissionState::Granted
                } else {
                    PermissionState::Denied
                };
                cache_permission(state);
            })
            .copy();
            unsafe {
                let options: usize = (1 << 1) | (1 << 2);
                let _: () = msg_send![center,
                    requestAuthorizationWithOptions: options
                    completionHandler: &*completion
                ];
            }
            cached_permission()
        }

        fn deliver(
            &mut self,
            request: &PlatformNotificationRequest,
        ) -> Result<(), PlatformNotificationError> {
            if cached_permission() == PermissionState::Denied {
                return Err(PlatformNotificationError::PermissionDenied);
            }
            let Some(center) = notification_center() else {
                return Err(PlatformNotificationError::Unavailable);
            };
            let content_class = Class::get("UNMutableNotificationContent")
                .ok_or(PlatformNotificationError::Unavailable)?;
            let request_class = Class::get("UNNotificationRequest")
                .ok_or(PlatformNotificationError::Unavailable)?;
            let title = ns_string(request.title())?;
            let body = ns_string(request.body())?;
            let identifier = ns_string(request.identifier())?;
            let content: *mut Object = unsafe { msg_send![content_class, new] };
            if content.is_null() {
                return Err(PlatformNotificationError::Unavailable);
            }
            unsafe {
                let _: () = msg_send![content, setTitle: title];
                let _: () = msg_send![content, setBody: body];
            }
            let native_request: *mut Object = unsafe {
                msg_send![request_class,
                    requestWithIdentifier: identifier
                    content: content
                    trigger: std::ptr::null_mut::<Object>()
                ]
            };
            if native_request.is_null() {
                unsafe {
                    let _: () = msg_send![content, release];
                }
                return Err(PlatformNotificationError::DeliveryFailed);
            }
            let completion = ConcreteBlock::new(move |_error: *mut Object| {}).copy();
            unsafe {
                let _: () = msg_send![center,
                    addNotificationRequest: native_request
                    withCompletionHandler: &*completion
                ];
                let _: () = msg_send![content, release];
            }
            Ok(())
        }

        fn remove(&mut self, identifier: &str) -> Result<(), PlatformNotificationError> {
            if identifier.is_empty() || identifier.len() > MAX_PLATFORM_IDENTIFIER_BYTES {
                return Err(PlatformNotificationError::InvalidPayload);
            }
            let Some(center) = notification_center() else {
                return Err(PlatformNotificationError::Unavailable);
            };
            let array_class =
                Class::get("NSArray").ok_or(PlatformNotificationError::Unavailable)?;
            let identifier = ns_string(identifier)?;
            let identifiers: *mut Object =
                unsafe { msg_send![array_class, arrayWithObject: identifier] };
            unsafe {
                let _: () =
                    msg_send![center, removeDeliveredNotificationsWithIdentifiers: identifiers];
                let _: () = msg_send![center, removePendingNotificationRequestsWithIdentifiers: identifiers];
            }
            Ok(())
        }

        fn take_activation(&mut self) -> Option<String> {
            activations()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
        }
    }

    fn activations() -> &'static Mutex<VecDeque<String>> {
        static ACTIVATIONS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
        ACTIVATIONS.get_or_init(|| Mutex::new(VecDeque::new()))
    }

    fn install_delegate() {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            let Some(center) = notification_center() else {
                return;
            };
            let Some(superclass) = Class::get("NSObject") else {
                return;
            };
            let class = match Class::get("TermiRustNotificationDelegate") {
                Some(class) => class,
                None => {
                    let Some(mut declaration) =
                        ClassDecl::new("TermiRustNotificationDelegate", superclass)
                    else {
                        return;
                    };
                    if let Some(protocol) = Protocol::get("UNUserNotificationCenterDelegate") {
                        declaration.add_protocol(protocol);
                    }
                    unsafe {
                        declaration.add_method(
                            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
                            did_receive_response
                                as extern "C" fn(
                                    &Object,
                                    Sel,
                                    *mut Object,
                                    *mut Object,
                                    *mut std::ffi::c_void,
                                ),
                        );
                    }
                    declaration.register()
                }
            };
            let delegate: *mut Object = unsafe { msg_send![class, new] };
            if !delegate.is_null() {
                unsafe {
                    let _: () = msg_send![center, setDelegate: delegate];
                }
                // `new` leaves a +1 retain count. UNUserNotificationCenter does
                // not retain its delegate, so this singleton lives for the process.
            }
        });
    }

    extern "C" fn did_receive_response(
        _delegate: &Object,
        _selector: Sel,
        _center: *mut Object,
        response: *mut Object,
        completion: *mut std::ffi::c_void,
    ) {
        if !response.is_null() {
            unsafe {
                let notification: *mut Object = msg_send![response, notification];
                let request: *mut Object = msg_send![notification, request];
                let identifier: *mut Object = msg_send![request, identifier];
                let bytes: *const std::ffi::c_char = msg_send![identifier, UTF8String];
                if !bytes.is_null()
                    && let Ok(identifier) = CStr::from_ptr(bytes).to_str()
                {
                    let mut pending = activations()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if pending.len() == 32 {
                        pending.pop_front();
                    }
                    pending.push_back(identifier.to_string());
                }
            }
        }
        if !completion.is_null() {
            unsafe {
                (*completion.cast::<Block<(), ()>>()).call(());
            }
        }
    }

    fn notification_center() -> Option<*mut Object> {
        let bundle_class = Class::get("NSBundle")?;
        let bundle: *mut Object = unsafe { msg_send![bundle_class, mainBundle] };
        let bundle_identifier: *mut Object = unsafe { msg_send![bundle, bundleIdentifier] };
        if bundle_identifier.is_null() {
            return None;
        }
        let class = Class::get("UNUserNotificationCenter")?;
        let center: *mut Object = unsafe { msg_send![class, currentNotificationCenter] };
        (!center.is_null()).then_some(center)
    }

    fn permission_from_status(status: isize) -> PermissionState {
        match status {
            0 => PermissionState::Unknown,
            1 => PermissionState::Denied,
            2..=4 => PermissionState::Granted,
            _ => PermissionState::Unavailable,
        }
    }

    fn permission_cache() -> &'static AtomicU8 {
        static PERMISSION: AtomicU8 = AtomicU8::new(0);
        &PERMISSION
    }

    fn cache_permission(permission: PermissionState) {
        permission_cache().store(permission_code(permission), Ordering::Release);
    }

    fn cached_permission() -> PermissionState {
        match permission_cache().load(Ordering::Acquire) {
            1 => PermissionState::Granted,
            2 => PermissionState::Denied,
            3 => PermissionState::Unavailable,
            _ => PermissionState::Unknown,
        }
    }

    const fn permission_code(permission: PermissionState) -> u8 {
        match permission {
            PermissionState::Unknown => 0,
            PermissionState::Granted => 1,
            PermissionState::Denied => 2,
            PermissionState::Unavailable => 3,
        }
    }

    fn ns_string(value: &str) -> Result<*mut Object, PlatformNotificationError> {
        let class = Class::get("NSString").ok_or(PlatformNotificationError::Unavailable)?;
        let allocated: *mut Object = unsafe { msg_send![class, alloc] };
        let string: *mut Object = unsafe {
            msg_send![allocated,
                initWithBytes: value.as_ptr()
                length: value.len()
                encoding: 4usize
            ]
        };
        if string.is_null() {
            Err(PlatformNotificationError::InvalidPayload)
        } else {
            let autoreleased: *mut Object = unsafe { msg_send![string, autorelease] };
            Ok(autoreleased)
        }
    }
}

#[cfg(all(target_os = "linux", not(test)))]
mod native {
    use std::collections::HashMap;

    use notify_rust::{Notification, NotificationHandle};

    use super::*;

    pub(super) struct NativePlatformNotifications {
        delivered: HashMap<String, NotificationHandle>,
    }

    pub(super) fn system() -> Box<dyn PlatformNotifications> {
        Box::new(NativePlatformNotifications {
            delivered: HashMap::new(),
        })
    }

    impl PlatformNotifications for NativePlatformNotifications {
        fn query_permission(&self) -> PermissionState {
            if notify_rust::get_server_information().is_ok() {
                PermissionState::Granted
            } else {
                PermissionState::Unavailable
            }
        }

        fn request_permission(&mut self) -> PermissionState {
            self.query_permission()
        }

        fn deliver(
            &mut self,
            request: &PlatformNotificationRequest,
        ) -> Result<(), PlatformNotificationError> {
            if let Some(previous) = self.delivered.remove(request.identifier()) {
                previous.close();
            }
            let handle = Notification::new()
                .appname("TermiRust")
                .summary(request.title())
                .body(request.body())
                .show()
                .map_err(|_| PlatformNotificationError::DeliveryFailed)?;
            self.delivered
                .insert(request.identifier().to_string(), handle);
            Ok(())
        }

        fn remove(&mut self, identifier: &str) -> Result<(), PlatformNotificationError> {
            if let Some(handle) = self.delivered.remove(identifier) {
                handle.close();
            }
            Ok(())
        }

        fn take_activation(&mut self) -> Option<String> {
            None
        }
    }
}

#[cfg(all(target_os = "windows", not(test)))]
mod native {
    use notify_rust::Notification;

    use super::*;

    pub(super) struct NativePlatformNotifications {
        delivery_available: Option<bool>,
    }

    pub(super) fn system() -> Box<dyn PlatformNotifications> {
        Box::new(NativePlatformNotifications {
            delivery_available: None,
        })
    }

    impl PlatformNotifications for NativePlatformNotifications {
        fn query_permission(&self) -> PermissionState {
            match self.delivery_available {
                Some(true) => PermissionState::Granted,
                Some(false) => PermissionState::Unavailable,
                None => PermissionState::Unknown,
            }
        }

        fn request_permission(&mut self) -> PermissionState {
            // Windows does not expose a notification permission prompt through this
            // API. The first delivery truthfully establishes availability.
            self.query_permission()
        }

        fn deliver(
            &mut self,
            request: &PlatformNotificationRequest,
        ) -> Result<(), PlatformNotificationError> {
            let result = Notification::new()
                .summary(request.title())
                .body(request.body())
                .show();
            self.delivery_available = Some(result.is_ok());
            result.map_err(|_| PlatformNotificationError::DeliveryFailed)
        }

        fn remove(&mut self, _identifier: &str) -> Result<(), PlatformNotificationError> {
            // The Windows backend does not return a removable notification handle.
            Err(PlatformNotificationError::Unavailable)
        }

        fn take_activation(&mut self) -> Option<String> {
            None
        }
    }
}

#[cfg(all(
    not(test),
    not(any(target_os = "macos", target_os = "linux", target_os = "windows"))
))]
mod native {
    use super::*;

    pub(super) struct NativePlatformNotifications;

    pub(super) fn system() -> Box<dyn PlatformNotifications> {
        Box::new(NativePlatformNotifications)
    }

    impl PlatformNotifications for NativePlatformNotifications {
        fn query_permission(&self) -> PermissionState {
            PermissionState::Unavailable
        }

        fn request_permission(&mut self) -> PermissionState {
            PermissionState::Unavailable
        }

        fn deliver(
            &mut self,
            _request: &PlatformNotificationRequest,
        ) -> Result<(), PlatformNotificationError> {
            Err(PlatformNotificationError::Unavailable)
        }

        fn remove(&mut self, _identifier: &str) -> Result<(), PlatformNotificationError> {
            Err(PlatformNotificationError::Unavailable)
        }

        fn take_activation(&mut self) -> Option<String> {
            None
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Default)]
    struct FakePlatformNotifications {
        permission: PermissionState,
        requested: usize,
        delivered: Vec<PlatformNotificationRequest>,
        removed: Vec<String>,
        fail_delivery: bool,
        activation: Option<String>,
    }

    impl PlatformNotifications for FakePlatformNotifications {
        fn query_permission(&self) -> PermissionState {
            self.permission
        }

        fn request_permission(&mut self) -> PermissionState {
            self.requested += 1;
            self.permission
        }

        fn deliver(
            &mut self,
            request: &PlatformNotificationRequest,
        ) -> Result<(), PlatformNotificationError> {
            if self.permission != PermissionState::Granted {
                return Err(PlatformNotificationError::PermissionDenied);
            }
            if self.fail_delivery {
                return Err(PlatformNotificationError::DeliveryFailed);
            }
            self.delivered.push(request.clone());
            Ok(())
        }

        fn remove(&mut self, identifier: &str) -> Result<(), PlatformNotificationError> {
            self.removed.push(identifier.to_string());
            Ok(())
        }

        fn take_activation(&mut self) -> Option<String> {
            self.activation.take()
        }
    }

    #[test]
    fn platform_notifications_fake_requires_permission_and_records_bounded_payload() {
        let request =
            PlatformNotificationRequest::new("notification-1", "A session", "Needs input").unwrap();
        let mut fake = FakePlatformNotifications {
            permission: PermissionState::Denied,
            ..FakePlatformNotifications::default()
        };
        assert_eq!(
            fake.deliver(&request),
            Err(PlatformNotificationError::PermissionDenied)
        );
        fake.permission = PermissionState::Granted;
        assert_eq!(fake.request_permission(), PermissionState::Granted);
        fake.deliver(&request).unwrap();
        fake.remove(request.identifier()).unwrap();
        fake.activation = Some("termirust-activity-summary".to_string());
        assert_eq!(fake.requested, 1);
        assert_eq!(request.title(), "A session");
        assert_eq!(request.body(), "Needs input");
        assert_eq!(fake.delivered, vec![request.clone()]);
        assert_eq!(fake.removed, vec!["notification-1"]);
        assert_eq!(
            fake.take_activation().as_deref(),
            Some("termirust-activity-summary")
        );
        assert!(!format!("{request:?}").contains("Needs input"));
    }

    #[test]
    fn platform_notifications_reject_content_and_identifier_overflow() {
        assert_eq!(
            PlatformNotificationRequest::new("bad id", "A session", "Done"),
            Err(PlatformNotificationError::InvalidPayload)
        );
        assert_eq!(
            PlatformNotificationRequest::new(
                "safe",
                &"x".repeat(MAX_PLATFORM_TITLE_SCALARS + 1),
                "Done"
            ),
            Err(PlatformNotificationError::InvalidPayload)
        );
        assert_eq!(
            PlatformNotificationRequest::new("safe", "secret\nline", "Done"),
            Err(PlatformNotificationError::InvalidPayload)
        );
    }

    #[test]
    fn platform_notifications_delivery_failure_has_no_automatic_retry() {
        let request = PlatformNotificationRequest::new("safe", "A session", "Failed").unwrap();
        let mut fake = FakePlatformNotifications {
            permission: PermissionState::Granted,
            fail_delivery: true,
            ..FakePlatformNotifications::default()
        };
        assert_eq!(
            fake.deliver(&request),
            Err(PlatformNotificationError::DeliveryFailed)
        );
        assert!(fake.delivered.is_empty());
    }
}
