//! The slice of VideoToolbox, Core Media, Core Video and Core Foundation this spike needs.
//!
//! Declared by hand rather than pulled from a binding crate: a spike should not add a dependency
//! to the workspace's lockfile before the design it is testing has been accepted. Only the
//! functions and keys used below are declared, and every one is a documented public API.

#![allow(non_upper_case_globals, non_snake_case)]

use std::ffi::c_void;

pub type OSStatus = i32;
pub type Boolean = u8;
pub type CFIndex = isize;
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type CFBooleanRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CVPixelBufferRef = *mut c_void;
pub type CMSampleBufferRef = *mut c_void;
pub type CMTime = CMTimeStruct;
pub type VTCompressionSessionRef = *mut c_void;
pub type VTEncodeInfoFlags = u32;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CMTimeStruct {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

impl CMTimeStruct {
    /// A valid time, which is all the encoder needs from the presentation stamp here.
    pub fn new(value: i64, timescale: i32) -> Self {
        Self {
            value,
            timescale,
            flags: 1, // kCMTimeFlags_Valid
            epoch: 0,
        }
    }

    pub fn invalid() -> Self {
        Self {
            value: 0,
            timescale: 0,
            flags: 0,
            epoch: 0,
        }
    }
}

pub const kCFNumberSInt32Type: CFIndex = 3;
pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
/// kCMVideoCodecType_HEVC, 'hvc1' as a big-endian four-character code.
pub const kCMVideoCodecType_HEVC: i32 = 0x6876_6331;
/// kCVPixelFormatType_32BGRA.
pub const kCVPixelFormatType_32BGRA: u32 = 0x4247_5241;

unsafe extern "C" {
    pub static kCFAllocatorDefault: CFAllocatorRef;
    pub static kCFBooleanTrue: CFBooleanRef;
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;

    pub fn CFStringCreateWithCString(
        allocator: CFAllocatorRef,
        cstr: *const i8,
        encoding: u32,
    ) -> CFStringRef;
    pub fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: CFIndex,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    pub fn CFDictionaryCreateMutable(
        allocator: CFAllocatorRef,
        capacity: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *mut c_void;
    pub fn CFDictionarySetValue(dictionary: *mut c_void, key: *const c_void, value: *const c_void);
    pub fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;
    pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
    pub fn CFArrayCreate(
        allocator: CFAllocatorRef,
        values: *const *const c_void,
        count: CFIndex,
        callbacks: *const c_void,
    ) -> CFArrayRef;
    pub fn CFNumberGetValue(number: CFNumberRef, the_type: CFIndex, value: *mut c_void) -> Boolean;
    pub fn CFRelease(value: CFTypeRef);
    pub fn CFRetain(value: CFTypeRef) -> CFTypeRef;

    pub fn CVPixelBufferCreate(
        allocator: CFAllocatorRef,
        width: usize,
        height: usize,
        pixel_format: u32,
        attributes: CFDictionaryRef,
        out: *mut CVPixelBufferRef,
    ) -> i32;
    pub fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
    pub fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
    pub fn CVPixelBufferGetBaseAddress(buffer: CVPixelBufferRef) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRow(buffer: CVPixelBufferRef) -> usize;

    pub fn CMSampleBufferGetSampleAttachmentsArray(
        buffer: CMSampleBufferRef,
        create_if_necessary: Boolean,
    ) -> CFArrayRef;
    pub fn CMSampleBufferGetTotalSampleSize(buffer: CMSampleBufferRef) -> usize;

    pub fn VTCompressionSessionCreate(
        allocator: CFAllocatorRef,
        width: i32,
        height: i32,
        codec_type: i32,
        encoder_specification: CFDictionaryRef,
        source_image_buffer_attributes: CFDictionaryRef,
        compressed_data_allocator: CFAllocatorRef,
        output_callback: Option<
            unsafe extern "C" fn(*mut c_void, *mut c_void, OSStatus, VTEncodeInfoFlags, CMSampleBufferRef),
        >,
        output_callback_ref_con: *mut c_void,
        out: *mut VTCompressionSessionRef,
    ) -> OSStatus;
    pub fn VTCompressionSessionPrepareToEncodeFrames(session: VTCompressionSessionRef) -> OSStatus;
    pub fn VTCompressionSessionEncodeFrame(
        session: VTCompressionSessionRef,
        image_buffer: CVPixelBufferRef,
        presentation_timestamp: CMTime,
        duration: CMTime,
        frame_properties: CFDictionaryRef,
        source_frame_ref_con: *mut c_void,
        info_flags_out: *mut VTEncodeInfoFlags,
    ) -> OSStatus;
    pub fn VTCompressionSessionCompleteFrames(
        session: VTCompressionSessionRef,
        complete_until: CMTime,
    ) -> OSStatus;
    pub fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);
    pub fn VTSessionSetProperty(
        session: VTCompressionSessionRef,
        key: CFStringRef,
        value: CFTypeRef,
    ) -> OSStatus;
}

/// A Core Foundation string from a Rust literal, for the property keys below.
pub fn cfstr(value: &str) -> CFStringRef {
    let owned = std::ffi::CString::new(value).expect("a property key has no interior nul");
    unsafe { CFStringCreateWithCString(kCFAllocatorDefault, owned.as_ptr(), kCFStringEncodingUTF8) }
}

pub fn cfnumber(value: i32) -> CFNumberRef {
    unsafe {
        CFNumberCreate(
            kCFAllocatorDefault,
            kCFNumberSInt32Type,
            std::ptr::addr_of!(value).cast(),
        )
    }
}

// The real keys, linked from the frameworks rather than spelled out. Several VideoToolbox
// constants do not have the string value their symbol name suggests, and a wrong string is
// silently ignored rather than refused, which is exactly how a spike reaches a false conclusion.
unsafe extern "C" {
    pub static kVTCompressionPropertyKey_RealTime: CFStringRef;
    pub static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    pub static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    pub static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    pub static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: CFStringRef;
    pub static kVTVideoEncoderSpecification_EnableLowLatencyRateControl: CFStringRef;
    pub static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;
    pub static kCMSampleAttachmentKey_NotSync: CFStringRef;
    pub static kVTCompressionPropertyKey_EnableLTR: CFStringRef;
    pub static kVTEncodeFrameOptionKey_ForceLTRRefresh: CFStringRef;
    pub static kVTEncodeFrameOptionKey_AcknowledgedLTRTokens: CFStringRef;
    pub static kVTSampleAttachmentKey_RequireLTRAcknowledgementToken: CFStringRef;
}

/// Prints what a linked constant's string actually is, so a guess can be checked rather than
/// trusted.
pub fn describe(name: &str, key: CFStringRef) {
    let mut buffer = [0i8; 128];
    let ok = unsafe { CFStringGetCString(key, buffer.as_mut_ptr(), 128, kCFStringEncodingUTF8) };
    if ok != 0 {
        let text = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
        println!("  {name} = {:?}", text.to_string_lossy());
    } else {
        println!("  {name} = <unavailable>");
    }
}

unsafe extern "C" {
    pub fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut i8,
        size: CFIndex,
        encoding: u32,
    ) -> Boolean;
}
