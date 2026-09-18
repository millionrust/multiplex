//! Spike 0.3: can a Rust host encode the Stage B motion path with VideoToolbox, and recover from
//! loss with long-term references instead of a keyframe?
//!
//! ```text
//! cargo run --release -- [frames] [width] [height]
//! ```
//!
//! What it does, in the order the real motion path would:
//!
//! 1. Opens an HEVC compression session in low-latency rate control.
//! 2. Turns on long-term references and asks the encoder to tag frames with acknowledgement
//!    tokens, which is the loop WWDC21 describes.
//! 3. Encodes synthetic screen-like frames — a window, text-ish rows, a moving caret — and
//!    acknowledges every token the encoder hands back, as a healthy receiver would.
//! 4. Stops acknowledging partway through, which is what a viewer that lost packets looks like.
//! 5. Calls `ForceLTRRefresh` and reports whether the frame that came back was a P-frame
//!    predicted from an older acknowledged reference, or a full keyframe.
//!
//! The question it answers is the one the plan asks: whether recovery costs a keyframe. A
//! keyframe on a 4K screen is the spike in the bandwidth graph that the motion path exists to
//! avoid, so if `ForceLTRRefresh` only ever produces IDRs then Stage B needs a different
//! recovery design.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This spike measures Apple's VideoToolbox; it only runs on macOS.");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
mod ffi;

#[cfg(target_os = "macos")]
fn main() {
    let mut arguments = std::env::args().skip(1);
    let frames: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60);
    let width: i32 = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1920);
    let height: i32 = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1080);
    // A wrong key string is ignored rather than refused, so print what the linked constants
    // really are before trusting any of the numbers below.
    unsafe {
        println!("linked key strings");
        ffi::describe("RealTime", ffi::kVTCompressionPropertyKey_RealTime);
        ffi::describe(
            "EnableLowLatencyRateControl",
            ffi::kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
        );
        ffi::describe("ForceKeyFrame", ffi::kVTEncodeFrameOptionKey_ForceKeyFrame);
        ffi::describe("NotSync", ffi::kCMSampleAttachmentKey_NotSync);
        ffi::describe("EnableLTR", ffi::kVTCompressionPropertyKey_EnableLTR);
        ffi::describe(
            "ForceLTRRefresh",
            ffi::kVTEncodeFrameOptionKey_ForceLTRRefresh,
        );
        ffi::describe(
            "AcknowledgedLTRTokens",
            ffi::kVTEncodeFrameOptionKey_AcknowledgedLTRTokens,
        );
        ffi::describe(
            "RequireLTRAcknowledgementToken",
            ffi::kVTSampleAttachmentKey_RequireLTRAcknowledgementToken,
        );
        println!();
    }
    // The same sequence twice: once recovering with a long-term reference, once with the
    // keyframe that recovery would otherwise cost.
    let with_ltr = match encode::run(frames, width, height, false) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("spike failed: {error}");
            std::process::exit(1);
        }
    };
    let with_keyframe = encode::run(frames, width, height, true).ok();
    with_ltr.print(with_keyframe.as_ref());
}

#[cfg(target_os = "macos")]
mod encode {
    use std::ffi::c_void;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use crate::ffi::*;

    /// One encoded frame, as the sender would see it.
    #[derive(Clone, Debug)]
    pub struct Encoded {
        pub index: usize,
        pub bytes: usize,
        /// A keyframe: everything before it can be thrown away, and it is expensive.
        pub keyframe: bool,
        /// Tokens the encoder wants acknowledged before it will predict from them.
        pub required_tokens: Vec<i32>,
        pub micros: u128,
    }

    #[derive(Default)]
    struct Collected {
        frames: Vec<Encoded>,
        started: Option<Instant>,
    }

    pub struct Report {
        pub frames: Vec<Encoded>,
        pub refresh_at: usize,
        pub stopped_acknowledging_at: usize,
        pub ltr_supported: bool,
    }

    impl Report {
        pub fn print(&self, keyframe_run: Option<&Report>) {
            let total: usize = self.frames.iter().map(|frame| frame.bytes).sum();
            let keyframes = self.frames.iter().filter(|frame| frame.keyframe).count();
            println!("frames encoded        {}", self.frames.len());
            println!("keyframes             {keyframes}");
            println!(
                "bytes total           {total} ({:.1} KB)",
                total as f64 / 1024.0
            );
            if let Some(first) = self.frames.first() {
                println!("first frame           {} bytes (the keyframe)", first.bytes);
            }
            let steady: Vec<usize> = self
                .frames
                .iter()
                .skip(1)
                .map(|frame| frame.bytes)
                .collect();
            if !steady.is_empty() {
                let mean = steady.iter().sum::<usize>() as f64 / steady.len() as f64;
                let worst = steady.iter().copied().max().unwrap_or(0);
                println!("after that            mean {mean:.0} bytes, worst {worst} bytes");
            }
            let slowest = self.frames.iter().map(|frame| frame.micros).max().unwrap_or(0);
            println!("slowest encode        {:.2} ms", slowest as f64 / 1000.0);
            println!();
            println!("long-term references  {}", if self.ltr_supported { "accepted" } else { "REFUSED" });
            println!("stopped acknowledging at frame {}", self.stopped_acknowledging_at);
            println!("forced a refresh at   frame {}", self.refresh_at);
            match self.frames.get(self.refresh_at) {
                Some(frame) if frame.keyframe => println!(
                    "recovery              KEYFRAME, {} bytes — LTR did not save the spike",
                    frame.bytes
                ),
                Some(frame) => println!(
                    "recovery              P-frame from an older reference, {} bytes — no keyframe needed",
                    frame.bytes
                ),
                None => println!("recovery              no frame came back"),
            }
            if let Some(run) = keyframe_run {
                let at: Vec<usize> = run
                    .frames
                    .iter()
                    .filter(|frame| frame.keyframe)
                    .map(|frame| frame.index)
                    .collect();
                println!("comparison run        keyframes at {at:?}");
            }
            // Compare against the forced keyframe wherever the encoder put it, rather than
            // assuming output order matches submission order.
            if let Some(other) = keyframe_run.and_then(|run| {
                run.frames
                    .iter()
                    .skip(1)
                    .find(|frame| frame.keyframe)
            }) {
                println!(
                    "the same recovery as a keyframe {} bytes (frame {})",
                    other.bytes, other.index
                );
                if let Some(ours) = self.frames.get(self.refresh_at) {
                    if ours.bytes > 0 {
                        println!(
                            "so a long-term reference costs {:.0}x less to recover",
                            other.bytes as f64 / ours.bytes as f64
                        );
                    }
                }
            }
        }
    }

    /// The encoder hands every finished frame here, on its own thread.
    unsafe extern "C" fn on_frame(
        output_ref_con: *mut c_void,
        _source_ref_con: *mut c_void,
        status: OSStatus,
        _flags: VTEncodeInfoFlags,
        sample: CMSampleBufferRef,
    ) {
        if status != 0 || sample.is_null() {
            return;
        }
        let collected = unsafe { &*(output_ref_con as *const Mutex<Collected>) };
        let bytes = unsafe { CMSampleBufferGetTotalSampleSize(sample) };
        let (keyframe, required_tokens) = unsafe { attachments(sample) };
        let mut collected = collected.lock().expect("collected frames");
        let micros = collected
            .started
            .map(|started| started.elapsed().as_micros())
            .unwrap_or(0);
        let index = collected.frames.len();
        collected.frames.push(Encoded {
            index,
            bytes,
            keyframe,
            required_tokens,
            micros,
        });
    }

    /// Whether the frame is a keyframe, and which acknowledgement tokens it carries.
    unsafe fn attachments(sample: CMSampleBufferRef) -> (bool, Vec<i32>) {
        let array = unsafe { CMSampleBufferGetSampleAttachmentsArray(sample, 0) };
        if array.is_null() || unsafe { CFArrayGetCount(array) } == 0 {
            // No attachments at all means the encoder did not mark it as "not a sync frame",
            // which by Apple's convention means it is one.
            return (true, Vec::new());
        }
        let dictionary = unsafe { CFArrayGetValueAtIndex(array, 0) };
        let depends = unsafe { CFDictionaryGetValue(dictionary, kCMSampleAttachmentKey_NotSync) };
        let keyframe = depends.is_null();

        let token = unsafe {
            CFDictionaryGetValue(dictionary, kVTSampleAttachmentKey_RequireLTRAcknowledgementToken)
        };
        let mut tokens = Vec::new();
        if !token.is_null() {
            let mut value: i32 = 0;
            let read = unsafe {
                CFNumberGetValue(
                    token,
                    kCFNumberSInt32Type,
                    std::ptr::addr_of_mut!(value).cast(),
                )
            };
            if read != 0 {
                tokens.push(value);
            }
        }
        (keyframe, tokens)
    }

    pub fn run(
        frames: usize,
        width: i32,
        height: i32,
        keyframe_recovery: bool,
    ) -> Result<Report, String> {
        let collected = Arc::new(Mutex::new(Collected::default()));
        let mut session: VTCompressionSessionRef = std::ptr::null_mut();

        // Ask for the hardware encoder; a software fallback would not answer the question.
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
            // Low latency is an encoder *specification*, chosen when the session is created,
            // not a property set afterwards. Long-term references are only offered by the
            // low-latency encoder, so getting this wrong silently loses both.
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
                width,
                height,
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
            return Err(format!("no HEVC encoder on this Mac (OSStatus {status})"));
        }

        // Low latency first: it is the mode the whole design assumes, and it changes which of
        // the properties below the encoder will accept.
        set_bool(session, kVTCompressionPropertyKey_RealTime, true)?;
        let ltr = set_bool(session, kVTCompressionPropertyKey_EnableLTR, true);
        let ltr_supported = ltr.is_ok();
        if let Err(reason) = &ltr {
            eprintln!("note: {reason}");
        }
        // A long keyframe interval, so the only keyframes are the first and any forced one.
        let _ = set_number(session, kVTCompressionPropertyKey_MaxKeyFrameInterval, 600);
        let _ = set_number(session, kVTCompressionPropertyKey_AverageBitRate, 8_000_000);
        let _ = set_bool(session, kVTCompressionPropertyKey_AllowFrameReordering, false);
        unsafe { VTCompressionSessionPrepareToEncodeFrames(session) };

        // Stop acknowledging two thirds of the way through, then ask for a refresh two frames
        // later: long enough that the unacknowledged references are the recent ones.
        let stopped_acknowledging_at = frames * 2 / 3;
        let refresh_at = (stopped_acknowledging_at + 2).min(frames.saturating_sub(1));
        let mut acknowledged: Vec<i32> = Vec::new();

        for index in 0..frames {
            let buffer = pixel_buffer(width, height, index)?;
            collected
                .lock()
                .expect("collected frames")
                .started
                .replace(Instant::now());

            let properties =
                frame_properties(index, refresh_at, &acknowledged, keyframe_recovery);
            let status = unsafe {
                VTCompressionSessionEncodeFrame(
                    session,
                    buffer,
                    CMTimeStruct::new(index as i64, 60),
                    CMTimeStruct::new(1, 60),
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
                unsafe { VTCompressionSessionInvalidate(session) };
                return Err(format!("the encoder refused a frame (OSStatus {status})"));
            }
            // A healthy receiver acknowledges what it decoded; a lossy one stops.
            if index < stopped_acknowledging_at {
                let tokens: Vec<i32> = collected
                    .lock()
                    .expect("collected frames")
                    .frames
                    .iter()
                    .flat_map(|frame| frame.required_tokens.clone())
                    .collect();
                acknowledged = tokens;
            }
        }

        unsafe {
            VTCompressionSessionCompleteFrames(session, CMTimeStruct::invalid());
            VTCompressionSessionInvalidate(session);
        }

        let frames = collected.lock().expect("collected frames").frames.clone();
        Ok(Report {
            frames,
            refresh_at,
            stopped_acknowledging_at,
            ltr_supported,
        })
    }

    /// What to tell the encoder about this particular frame.
    fn frame_properties(
        index: usize,
        refresh_at: usize,
        acknowledged: &[i32],
        keyframe_recovery: bool,
    ) -> CFDictionaryRef {
        if index != refresh_at && acknowledged.is_empty() {
            return std::ptr::null();
        }
        unsafe {
            let dictionary = CFDictionaryCreateMutable(
                kCFAllocatorDefault,
                0,
                std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
                std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
            );
            if !acknowledged.is_empty() {
                let numbers: Vec<CFNumberRef> = acknowledged.iter().copied().map(cfnumber).collect();
                let array = CFArrayCreate(
                    kCFAllocatorDefault,
                    numbers.as_ptr().cast(),
                    numbers.len() as CFIndex,
                    std::ptr::null(),
                );
                CFDictionarySetValue(dictionary, kVTEncodeFrameOptionKey_AcknowledgedLTRTokens, array);
                CFRelease(array);
                for number in numbers {
                    CFRelease(number);
                }
            }
            if index == refresh_at {
                // The whole point of the spike: recover without asking for a keyframe. The
                // comparison run asks for one instead, so the two costs sit side by side.
                let key = if keyframe_recovery {
                    kVTEncodeFrameOptionKey_ForceKeyFrame
                } else {
                    kVTEncodeFrameOptionKey_ForceLTRRefresh
                };
                CFDictionarySetValue(dictionary, key, kCFBooleanTrue);
            }
            dictionary
        }
    }

    fn set_bool(
        session: VTCompressionSessionRef,
        key: CFStringRef,
        value: bool,
    ) -> Result<(), String> {
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
        if status == 0 {
            Ok(())
        } else {
            Err(format!("a session property was refused (OSStatus {status})"))
        }
    }

    fn set_number(
        session: VTCompressionSessionRef,
        key: CFStringRef,
        value: i32,
    ) -> Result<(), String> {
        let number = cfnumber(value);
        let status = unsafe { VTSessionSetProperty(session, key, number) };
        unsafe { CFRelease(number) };
        if status == 0 {
            Ok(())
        } else {
            Err(format!("a session property was refused (OSStatus {status})"))
        }
    }

    /// A frame that looks like a screen: a window on a desk, rows of text, a caret that moves.
    /// Screen content is what the motion path has to carry, and it compresses nothing like video.
    fn pixel_buffer(width: i32, height: i32, step: usize) -> Result<CVPixelBufferRef, String> {
        let mut buffer: CVPixelBufferRef = std::ptr::null_mut();
        let status = unsafe {
            CVPixelBufferCreate(
                kCFAllocatorDefault,
                width as usize,
                height as usize,
                kCVPixelFormatType_32BGRA,
                std::ptr::null(),
                std::ptr::addr_of_mut!(buffer),
            )
        };
        if status != 0 || buffer.is_null() {
            return Err(format!("no pixel buffer (CVReturn {status})"));
        }
        unsafe {
            CVPixelBufferLockBaseAddress(buffer, 0);
            let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRow(buffer);
            for y in 0..height as usize {
                let row = base.add(y * stride);
                for x in 0..width as usize {
                    let pixel = row.add(x * 4);
                    let inside_window = x > 60 && x < width as usize - 60 && y > 80 && y < height as usize - 80;
                    let text_row = inside_window && (y / 22) % 2 == 0 && x % 9 < 6;
                    let caret = inside_window
                        && y / 22 == (step % 20) + 4
                        && x > 100
                        && x < 120;
                    let (blue, green, red) = if caret {
                        (80u8, 200, 240)
                    } else if text_row {
                        (200, 208, 214)
                    } else if inside_window {
                        (26, 28, 30)
                    } else {
                        (228, 232, 236)
                    };
                    pixel.write(blue);
                    pixel.add(1).write(green);
                    pixel.add(2).write(red);
                    pixel.add(3).write(255);
                }
            }
            CVPixelBufferUnlockBaseAddress(buffer, 0);
        }
        Ok(buffer)
    }
}
