//! Low-latency HEVC over VideoToolbox, with long-term references.
//!
//! The order of operations matters more than it looks. Low latency is an **encoder
//! specification**, chosen when the session is created; long-term references are only offered by
//! the low-latency encoder, so asking for them on a session created without it is refused and the
//! whole point of the motion path is lost. Spike 0.3 got this wrong first and still printed a
//! plausible recovery number.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::ffi::*;
use crate::{Encoded, EncoderConfig, Request, VideoError};

/// Annex B start code. Four bytes, which is what every decoder accepts.
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Frames the callback has produced but the caller has not collected. VideoToolbox calls back on
/// its own thread, so this is the only shared state.
#[derive(Debug, Default)]
struct Collected {
    frames: Vec<Encoded>,
    /// Parameter sets, in Annex B, from the first frame that carried a format description.
    parameter_sets: Vec<u8>,
}

/// A live compression session for one motion region.
///
/// Dropping it invalidates the session. One region at a time: when the region moves or resizes,
/// open a new encoder, because the old one's references describe pixels that are no longer there.
#[derive(Debug)]
pub struct HevcEncoder {
    session: VTCompressionSessionRef,
    config: EncoderConfig,
    collected: Arc<Mutex<Collected>>,
    /// Counts submissions, for the presentation stamps the encoder needs to keep frames ordered.
    submitted: i64,
}

// The session is only ever touched through `&mut self`, and the callback state is behind its own
// mutex. VideoToolbox's own handle is not tied to a thread.
unsafe impl Send for HevcEncoder {}

impl Drop for HevcEncoder {
    fn drop(&mut self) {
        if !self.session.is_null() {
            unsafe {
                VTCompressionSessionCompleteFrames(self.session, CMTimeStruct::invalid());
                VTCompressionSessionInvalidate(self.session);
                CFRelease(self.session as CFTypeRef);
            }
        }
    }
}

impl HevcEncoder {
    /// Opens a hardware session for a region of `config`'s size.
    ///
    /// Fails rather than degrading: a session without long-term references would answer every
    /// lost packet with a keyframe, which on a 4K screen is the bandwidth spike this path exists
    /// to remove. The caller keeps the tile path instead.
    pub fn open(config: EncoderConfig) -> Result<Self, VideoError> {
        if config.width == 0
            || config.height == 0
            || config.width > 16_384
            || config.height > 16_384
        {
            return Err(VideoError::InvalidSize {
                width: config.width,
                height: config.height,
            });
        }
        let collected = Arc::new(Mutex::new(Collected::default()));
        let mut session: VTCompressionSessionRef = std::ptr::null_mut();

        let specification = unsafe {
            let dictionary = CFDictionaryCreateMutable(
                kCFAllocatorDefault,
                0,
                std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
                std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
            );
            CFDictionarySetValue(
                dictionary,
                kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
                kCFBooleanTrue,
            );
            // Both of these belong here, at creation, and not in a property set afterwards.
            CFDictionarySetValue(
                dictionary,
                kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
                kCFBooleanTrue,
            );
            dictionary
        };
        let status = unsafe {
            VTCompressionSessionCreate(
                kCFAllocatorDefault,
                config.width as i32,
                config.height as i32,
                kCMVideoCodecType_HEVC,
                specification,
                std::ptr::null(),
                kCFAllocatorDefault,
                Some(on_frame),
                Arc::as_ptr(&collected) as *mut c_void,
                std::ptr::addr_of_mut!(session),
            )
        };
        unsafe { CFRelease(specification) };
        if status != 0 || session.is_null() {
            return Err(VideoError::NoEncoder(status));
        }

        let encoder = Self {
            session,
            config,
            collected,
            submitted: 0,
        };
        set_bool(session, unsafe { kVTCompressionPropertyKey_RealTime }, true)
            .map_err(VideoError::Refused)?;
        set_bool(
            session,
            unsafe { kVTCompressionPropertyKey_EnableLTR },
            true,
        )
        .map_err(|_| VideoError::NoLongTermReferences)?;
        // Keyframes only when asked for. Recovery goes through references, so a periodic
        // keyframe would be pure cost.
        let _ = set_number(
            session,
            unsafe { kVTCompressionPropertyKey_MaxKeyFrameInterval },
            i32::MAX,
        );
        let _ = set_number(
            session,
            unsafe { kVTCompressionPropertyKey_ExpectedFrameRate },
            config.frame_rate as i32,
        );
        let _ = set_number(
            session,
            unsafe { kVTCompressionPropertyKey_AverageBitRate },
            config.bitrate as i32,
        );
        // Reordering would hold frames back to encode a later one first, which is latency the
        // motion path cannot spend.
        let _ = set_bool(
            session,
            unsafe { kVTCompressionPropertyKey_AllowFrameReordering },
            false,
        );
        unsafe { VTCompressionSessionPrepareToEncodeFrames(session) };
        Ok(encoder)
    }

    pub const fn config(&self) -> EncoderConfig {
        self.config
    }

    /// The decoder configuration — VPS, SPS and PPS in Annex B — available once the first frame
    /// has been encoded. Send it before the first frame, and again to any viewer that joins.
    pub fn parameter_sets(&self) -> Vec<u8> {
        self.collected
            .lock()
            .expect("encoder output")
            .parameter_sets
            .clone()
    }

    /// Encodes one frame of the region, given tightly packed or padded BGRA.
    ///
    /// Returns every frame the encoder has finished, which may be none — it buffers — and may be
    /// more than one. It also drops frames under load, so the result is not one-to-one with
    /// submissions, and a token must never be matched to a frame by counting.
    pub fn encode(
        &mut self,
        bgra: &[u8],
        stride: usize,
        request: Request<'_>,
    ) -> Result<Vec<Encoded>, VideoError> {
        let height = self.config.height as usize;
        if stride < self.config.width as usize * 4 || bgra.len() < stride * height {
            return Err(VideoError::ShortFrame);
        }
        let buffer = self.pixel_buffer(bgra, stride)?;
        let properties = frame_properties(&request);
        let status = unsafe {
            VTCompressionSessionEncodeFrame(
                self.session,
                buffer,
                CMTimeStruct::new(self.submitted, self.config.frame_rate.max(1) as i32),
                CMTimeStruct::new(1, self.config.frame_rate.max(1) as i32),
                properties,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if !properties.is_null() {
            unsafe { CFRelease(properties) };
        }
        unsafe { CFRelease(buffer as CFTypeRef) };
        if status != 0 {
            return Err(VideoError::Refused(status));
        }
        self.submitted += 1;
        Ok(std::mem::take(
            &mut self.collected.lock().expect("encoder output").frames,
        ))
    }

    /// Waits for everything submitted so far, so a caller can drain the tail of a region before
    /// demoting it.
    pub fn finish(&mut self) -> Vec<Encoded> {
        unsafe { VTCompressionSessionCompleteFrames(self.session, CMTimeStruct::invalid()) };
        std::mem::take(&mut self.collected.lock().expect("encoder output").frames)
    }

    /// Copies BGRA into a pixel buffer the encoder will take. The encoder converts to 4:2:0
    /// itself; the motion region never contains text, which is what makes that acceptable.
    fn pixel_buffer(&self, bgra: &[u8], stride: usize) -> Result<CVPixelBufferRef, VideoError> {
        let (width, height) = (self.config.width as usize, self.config.height as usize);
        let mut buffer: CVPixelBufferRef = std::ptr::null_mut();
        let status = unsafe {
            CVPixelBufferCreate(
                kCFAllocatorDefault,
                width,
                height,
                kCVPixelFormatType_32BGRA,
                std::ptr::null(),
                std::ptr::addr_of_mut!(buffer),
            )
        };
        if status != 0 || buffer.is_null() {
            return Err(VideoError::Refused(status));
        }
        unsafe {
            CVPixelBufferLockBaseAddress(buffer, 0);
            let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
            let destination_stride = CVPixelBufferGetBytesPerRow(buffer);
            let row_bytes = width * 4;
            for y in 0..height {
                let source = &bgra[y * stride..y * stride + row_bytes];
                std::ptr::copy_nonoverlapping(
                    source.as_ptr(),
                    base.add(y * destination_stride),
                    row_bytes,
                );
            }
            CVPixelBufferUnlockBaseAddress(buffer, 0);
        }
        Ok(buffer)
    }
}

/// What to tell the encoder about this particular frame.
fn frame_properties(request: &Request<'_>) -> CFDictionaryRef {
    if request.acknowledged.is_empty() && !request.refresh && !request.keyframe {
        return std::ptr::null();
    }
    unsafe {
        let dictionary = CFDictionaryCreateMutable(
            kCFAllocatorDefault,
            0,
            std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
            std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
        );
        if !request.acknowledged.is_empty() {
            let numbers: Vec<CFNumberRef> = request
                .acknowledged
                .iter()
                .map(|token| cfnumber(*token as i32))
                .collect();
            let array = CFArrayCreate(
                kCFAllocatorDefault,
                numbers.as_ptr().cast(),
                numbers.len() as CFIndex,
                std::ptr::null(),
            );
            CFDictionarySetValue(
                dictionary,
                kVTEncodeFrameOptionKey_AcknowledgedLTRTokens,
                array,
            );
            CFRelease(array);
            for number in numbers {
                CFRelease(number);
            }
        }
        // A keyframe request wins, because it is only ever made when the viewer has nothing at
        // all to predict from, and a reference refresh would then produce an undecodable frame.
        if request.keyframe {
            CFDictionarySetValue(
                dictionary,
                kVTEncodeFrameOptionKey_ForceKeyFrame,
                kCFBooleanTrue,
            );
        } else if request.refresh {
            CFDictionarySetValue(
                dictionary,
                kVTEncodeFrameOptionKey_ForceLTRRefresh,
                kCFBooleanTrue,
            );
        }
        dictionary
    }
}

/// VideoToolbox's output callback, on its own thread.
unsafe extern "C" fn on_frame(
    output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: OSStatus,
    _flags: VTEncodeInfoFlags,
    sample: CMSampleBufferRef,
) {
    if status != 0 || sample.is_null() || output_ref_con.is_null() {
        return;
    }
    let collected = unsafe { &*(output_ref_con as *const Mutex<Collected>) };
    let (keyframe, token) = unsafe { attachments(sample) };
    let Some(payload) = (unsafe { annex_b_payload(sample) }) else {
        return;
    };
    let mut collected = collected.lock().expect("encoder output");
    if collected.parameter_sets.is_empty()
        && let Some(sets) = unsafe { annex_b_parameter_sets(sample) }
    {
        collected.parameter_sets = sets;
    }
    collected.frames.push(Encoded {
        payload,
        keyframe,
        token,
    });
}

/// Whether the frame is a keyframe, and the acknowledgement token it carries.
unsafe fn attachments(sample: CMSampleBufferRef) -> (bool, Option<u32>) {
    let array = unsafe { CMSampleBufferGetSampleAttachmentsArray(sample, 0) };
    if array.is_null() || unsafe { CFArrayGetCount(array) } == 0 {
        // No attachments means the encoder did not mark it "not a sync frame", which by Apple's
        // convention means it is one.
        return (true, None);
    }
    let dictionary = unsafe { CFArrayGetValueAtIndex(array, 0) };
    let not_sync = unsafe { CFDictionaryGetValue(dictionary, kCMSampleAttachmentKey_NotSync) };
    let keyframe = not_sync.is_null();

    let token = unsafe {
        CFDictionaryGetValue(
            dictionary,
            kVTSampleAttachmentKey_RequireLTRAcknowledgementToken,
        )
    };
    if token.is_null() {
        return (keyframe, None);
    }
    let mut value: i32 = 0;
    let read = unsafe {
        CFNumberGetValue(
            token,
            kCFNumberSInt32Type,
            std::ptr::addr_of_mut!(value).cast(),
        )
    };
    (keyframe, (read != 0 && value >= 0).then_some(value as u32))
}

/// The sample's NAL units, converted from VideoToolbox's length prefixes to Annex B start codes.
unsafe fn annex_b_payload(sample: CMSampleBufferRef) -> Option<Vec<u8>> {
    let block = unsafe { CMSampleBufferGetDataBuffer(sample) };
    if block.is_null() {
        return None;
    }
    let length = unsafe { CMBlockBufferGetDataLength(block) };
    if length == 0 {
        return None;
    }
    let mut raw = vec![0u8; length];
    let status = unsafe { CMBlockBufferCopyDataBytes(block, 0, length, raw.as_mut_ptr().cast()) };
    if status != 0 {
        return None;
    }
    let prefix = unsafe { nal_length_size(sample) }.unwrap_or(4);
    Some(to_annex_b(&raw, prefix))
}

/// Rewrites `length`-prefixed NAL units as start-code separated ones. An unparseable buffer
/// yields nothing rather than a payload with a wrong boundary in it, which a decoder would
/// take as corrupt video.
fn to_annex_b(raw: &[u8], prefix: usize) -> Vec<u8> {
    if !(1..=4).contains(&prefix) {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 64 + 8);
    let mut at = 0;
    while at + prefix <= raw.len() {
        let mut size = 0usize;
        for byte in &raw[at..at + prefix] {
            size = (size << 8) | *byte as usize;
        }
        at += prefix;
        if size == 0 || at + size > raw.len() {
            return Vec::new();
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&raw[at..at + size]);
        at += size;
    }
    if at == raw.len() { out } else { Vec::new() }
}

/// How many bytes each NAL unit's length prefix takes in this stream.
unsafe fn nal_length_size(sample: CMSampleBufferRef) -> Option<usize> {
    let description = unsafe { CMSampleBufferGetFormatDescription(sample) };
    if description.is_null() {
        return None;
    }
    let mut nal_length_size: i32 = 0;
    let status = unsafe {
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
            description,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(nal_length_size),
        )
    };
    (status == 0 && (1..=4).contains(&nal_length_size)).then_some(nal_length_size as usize)
}

/// VPS, SPS and PPS from the sample's format description, in Annex B.
unsafe fn annex_b_parameter_sets(sample: CMSampleBufferRef) -> Option<Vec<u8>> {
    let description = unsafe { CMSampleBufferGetFormatDescription(sample) };
    if description.is_null() {
        return None;
    }
    let mut count: usize = 0;
    let status = unsafe {
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
            description,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(count),
            std::ptr::null_mut(),
        )
    };
    if status != 0 || count == 0 {
        return None;
    }
    let mut out = Vec::new();
    for index in 0..count {
        let mut pointer: *const u8 = std::ptr::null();
        let mut size: usize = 0;
        let status = unsafe {
            CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                description,
                index,
                std::ptr::addr_of_mut!(pointer),
                std::ptr::addr_of_mut!(size),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 || pointer.is_null() || size == 0 {
            return None;
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(pointer, size) });
    }
    Some(out)
}

fn set_bool(session: VTCompressionSessionRef, key: CFStringRef, value: bool) -> Result<(), i32> {
    let status = unsafe {
        VTSessionSetProperty(
            session,
            key,
            if value {
                kCFBooleanTrue
            } else {
                std::ptr::null()
            },
        )
    };
    if status == 0 { Ok(()) } else { Err(status) }
}

fn set_number(session: VTCompressionSessionRef, key: CFStringRef, value: i32) -> Result<(), i32> {
    let number = cfnumber(value);
    let status = unsafe { VTSessionSetProperty(session, key, number) };
    unsafe { CFRelease(number) };
    if status == 0 { Ok(()) } else { Err(status) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame that looks like screen content in motion: a dark panel, rows, a moving caret.
    fn frame(config: EncoderConfig, step: u32) -> Vec<u8> {
        let (width, height) = (config.width as usize, config.height as usize);
        let mut pixels = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let at = (y * width + x) * 4;
                let row = y / 12;
                let caret = row == (step as usize % 8) + 1 && x > 20 && x < 32;
                let text = row % 2 == 0 && x % 7 < 5;
                let (b, g, r) = if caret {
                    (80u8, 200, 240)
                } else if text {
                    (200, 208, 214)
                } else {
                    (26, 28, 30)
                };
                pixels[at] = b;
                pixels[at + 1] = g;
                pixels[at + 2] = r;
                pixels[at + 3] = 255;
            }
        }
        pixels
    }

    #[test]
    fn length_prefixes_become_start_codes() {
        let raw = [0, 0, 0, 2, 0x40, 0x01, 0, 0, 0, 3, 0x42, 0x01, 0x02];
        assert_eq!(
            to_annex_b(&raw, 4),
            vec![0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x42, 0x01, 0x02]
        );
        assert_eq!(
            to_annex_b(&[0, 0, 0, 9, 1, 2], 4),
            Vec::<u8>::new(),
            "runs past the end"
        );
        assert_eq!(
            to_annex_b(&[0, 0, 0, 0], 4),
            Vec::<u8>::new(),
            "an empty unit"
        );
        assert_eq!(
            to_annex_b(&[0, 0, 0, 1, 7, 8], 4),
            Vec::<u8>::new(),
            "a trailing byte"
        );
        assert_eq!(
            to_annex_b(&[2, 0x40, 0x01], 1),
            vec![0, 0, 0, 1, 0x40, 0x01]
        );
        assert_eq!(
            to_annex_b(&[0, 0, 0, 1, 7], 5),
            Vec::<u8>::new(),
            "no such prefix width"
        );
    }

    #[test]
    fn a_wrong_size_is_refused_before_the_encoder_sees_it() {
        assert_eq!(
            HevcEncoder::open(EncoderConfig::new(0, 64)).unwrap_err(),
            VideoError::InvalidSize {
                width: 0,
                height: 64
            }
        );
    }

    /// The lesson from spike 0.3, pinned: these key strings do not all match their symbol names,
    /// and a wrong one is ignored rather than refused.
    #[test]
    fn the_linked_key_strings_are_what_the_encoder_expects() {
        for (key, expected) in unsafe {
            [
                (
                    kVTEncodeFrameOptionKey_ForceKeyFrame,
                    "EncoderForceKeyframe",
                ),
                (kVTEncodeFrameOptionKey_ForceLTRRefresh, "ForceLTRRefresh"),
                (
                    kVTEncodeFrameOptionKey_AcknowledgedLTRTokens,
                    "AcknowledgedLTRTokens",
                ),
                (kVTCompressionPropertyKey_EnableLTR, "EnableLTR"),
                (
                    kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
                    "EnableLowLatencyRateControl",
                ),
            ]
        } {
            assert_eq!(key_string(key).as_deref(), Some(expected));
        }
    }

    #[test]
    fn a_region_encodes_and_only_the_first_frame_is_a_keyframe() {
        let config = EncoderConfig::new(320, 192);
        let Ok(mut encoder) = HevcEncoder::open(config) else {
            eprintln!("no low-latency HEVC encoder on this machine; skipping");
            return;
        };
        let mut produced = Vec::new();
        for step in 0..12 {
            produced.extend(
                encoder
                    .encode(
                        &frame(config, step),
                        config.width as usize * 4,
                        Request::default(),
                    )
                    .expect("the encoder takes a frame"),
            );
        }
        produced.extend(encoder.finish());
        assert!(!produced.is_empty(), "the encoder produced nothing");
        assert!(produced[0].keyframe, "a stream opens with a keyframe");
        assert!(
            produced[1..].iter().all(|frame| !frame.keyframe),
            "nothing asked for a keyframe, so there should be no more"
        );
        assert!(
            produced
                .iter()
                .all(|frame| frame.payload.starts_with(&START_CODE)),
            "every payload is Annex B"
        );
        let sets = encoder.parameter_sets();
        assert!(
            sets.starts_with(&START_CODE) && sets.len() > 16,
            "the decoder configuration is missing"
        );
    }

    #[test]
    fn recovery_costs_far_less_than_a_keyframe() {
        let config = EncoderConfig::new(640, 384);
        let Ok(mut encoder) = HevcEncoder::open(config) else {
            eprintln!("no low-latency HEVC encoder on this machine; skipping");
            return;
        };
        // Encode a while, acknowledging everything, as a healthy viewer would.
        let mut acknowledged: Vec<u32> = Vec::new();
        let mut steady = Vec::new();
        for step in 0..20 {
            let request = Request {
                acknowledged: &acknowledged,
                ..Request::default()
            };
            for produced in encoder
                .encode(&frame(config, step), config.width as usize * 4, request)
                .expect("a frame")
            {
                acknowledged.extend(produced.token);
                steady.push(produced);
            }
        }
        assert!(
            steady.len() > 1,
            "the encoder produced nothing to recover from"
        );

        // Now the viewer reports loss. Recovery predicts from an older acknowledged reference.
        let recovered = encoder
            .encode(
                &frame(config, 20),
                config.width as usize * 4,
                Request {
                    acknowledged: &acknowledged,
                    refresh: true,
                    ..Request::default()
                },
            )
            .expect("a frame");
        let mut recovered: Vec<Encoded> = recovered;
        recovered.extend(encoder.finish());
        let Some(recovery) = recovered.first() else {
            eprintln!("the encoder buffered the recovery frame; skipping the comparison");
            return;
        };
        let keyframe = steady[0].payload.len();
        assert!(
            !recovery.keyframe,
            "recovery must not cost a keyframe; that is the whole point"
        );
        assert!(
            recovery.payload.len() * 4 < keyframe,
            "recovery was {} bytes against a {keyframe} byte keyframe",
            recovery.payload.len()
        );
    }
}
