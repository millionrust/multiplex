//! Decoding the motion region back into pixels, with VideoToolbox.
//!
//! The mirror of [`crate::HevcEncoder`]. It takes the Annex B parameter sets the host sent in its
//! video configuration, then the Annex B frames, and hands back BGRA the viewer can draw straight
//! into its framebuffer.
//!
//! Two things it does not do. It does not reorder: the encoder is configured without frame
//! reordering, so decode order is presentation order and a frame that arrives late is late. And
//! it does not hide a gap: a frame that cannot be decoded because the one before it never arrived
//! produces nothing, which is exactly what the viewer needs to report so the host can predict
//! from a reference this viewer still holds.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::ffi::*;
use crate::{Picture, VideoError};

/// Apple's decoder wants NAL units with a length prefix rather than start codes, and the
/// configuration says how many bytes that prefix takes. Four is what everything uses.
const NAL_LENGTH_SIZE: i32 = 4;

#[derive(Debug, Default)]
struct Decoded {
    picture: Option<Picture>,
}

/// A live decompression session for one motion region.
#[derive(Debug)]
pub struct HevcDecoder {
    session: VTDecompressionSessionRef,
    format: CMFormatDescriptionRef,
    decoded: Arc<Mutex<Decoded>>,
}

// Touched only through `&mut self`, with the callback's output behind its own mutex.
unsafe impl Send for HevcDecoder {}

impl Drop for HevcDecoder {
    fn drop(&mut self) {
        unsafe {
            if !self.session.is_null() {
                VTDecompressionSessionWaitForAsynchronousFrames(self.session);
                VTDecompressionSessionInvalidate(self.session);
                CFRelease(self.session as CFTypeRef);
            }
            if !self.format.is_null() {
                CFRelease(self.format);
            }
        }
    }
}

impl HevcDecoder {
    /// Opens a decoder from the parameter sets the host sent, in Annex B.
    ///
    /// Fails rather than guessing: parameter sets that do not parse, or a machine with no HEVC
    /// decoder, mean this viewer cannot show the motion region and should say so, so the host
    /// falls back to tiles.
    pub fn open(parameter_sets: &[u8]) -> Result<Self, VideoError> {
        let sets = split_annex_b(parameter_sets);
        if sets.is_empty() {
            return Err(VideoError::InvalidParameterSets);
        }
        let pointers: Vec<*const u8> = sets.iter().map(|set| set.as_ptr()).collect();
        let sizes: Vec<usize> = sets.iter().map(|set| set.len()).collect();

        let mut format: CMFormatDescriptionRef = std::ptr::null();
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                kCFAllocatorDefault,
                sets.len(),
                pointers.as_ptr(),
                sizes.as_ptr(),
                NAL_LENGTH_SIZE,
                std::ptr::null(),
                std::ptr::addr_of_mut!(format),
            )
        };
        if status != 0 || format.is_null() {
            return Err(VideoError::InvalidParameterSets);
        }

        // Ask for BGRA out, which is what the tile framebuffer is, so nothing has to convert
        // colour on the way to the screen.
        let attributes = unsafe {
            let dictionary = CFDictionaryCreateMutable(
                kCFAllocatorDefault,
                0,
                std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
                std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
            );
            let format = cfnumber(kCVPixelFormatType_32BGRA as i32);
            CFDictionarySetValue(dictionary, kCVPixelBufferPixelFormatTypeKey, format);
            CFRelease(format);
            dictionary
        };

        let decoded = Arc::new(Mutex::new(Decoded::default()));
        let callback = VTDecompressionOutputCallbackRecord {
            callback: Some(on_picture),
            ref_con: Arc::as_ptr(&decoded) as *mut c_void,
        };
        let mut session: VTDecompressionSessionRef = std::ptr::null_mut();
        let status = unsafe {
            VTDecompressionSessionCreate(
                kCFAllocatorDefault,
                format,
                std::ptr::null(),
                attributes,
                std::ptr::addr_of!(callback),
                std::ptr::addr_of_mut!(session),
            )
        };
        unsafe { CFRelease(attributes) };
        if status != 0 || session.is_null() {
            unsafe { CFRelease(format) };
            return Err(VideoError::NoDecoder(status));
        }
        Ok(Self {
            session,
            format,
            decoded,
        })
    }

    /// Decodes one Annex B frame.
    ///
    /// `None` means the decoder produced nothing for it, which happens when the frame depends on
    /// one that never arrived. That is a fact for the viewer to report, not an error to raise.
    pub fn decode(&mut self, payload: &[u8]) -> Result<Option<Picture>, VideoError> {
        let units = split_annex_b(payload);
        if units.is_empty() {
            return Err(VideoError::Malformed);
        }
        let mut framed = Vec::with_capacity(payload.len());
        for unit in &units {
            framed.extend_from_slice(&(unit.len() as u32).to_be_bytes());
            framed.extend_from_slice(unit);
        }

        let sample = unsafe { self.sample_buffer(&framed)? };
        let status = unsafe {
            VTDecompressionSessionDecodeFrame(
                self.session,
                sample,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        unsafe {
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
            CFRelease(sample as CFTypeRef);
        }
        if status != 0 {
            // A frame the decoder could not use. The picture stands still; the viewer reports it.
            return Ok(None);
        }
        Ok(self.decoded.lock().expect("decoder output").picture.take())
    }

    /// Wraps the framed units in the sample buffer the decoder takes.
    unsafe fn sample_buffer(&self, framed: &[u8]) -> Result<CMSampleBufferRef, VideoError> {
        let mut block: CMBlockBufferRef = std::ptr::null_mut();
        let status = unsafe {
            CMBlockBufferCreateWithMemoryBlock(
                kCFAllocatorDefault,
                std::ptr::null_mut(),
                framed.len(),
                kCFAllocatorDefault,
                std::ptr::null(),
                0,
                framed.len(),
                0,
                std::ptr::addr_of_mut!(block),
            )
        };
        if status != 0 || block.is_null() {
            return Err(VideoError::Refused(status));
        }
        // The block owns its memory, so the frame is copied in rather than borrowed: the decoder
        // outlives this call and must not be left pointing at a caller's slice.
        let status = unsafe {
            CMBlockBufferReplaceDataBytes(framed.as_ptr().cast(), block, 0, framed.len())
        };
        if status != 0 {
            unsafe { CFRelease(block as CFTypeRef) };
            return Err(VideoError::Refused(status));
        }

        let timing = CMSampleTimingInfo {
            duration: CMTimeStruct::invalid(),
            presentation_time_stamp: CMTimeStruct::invalid(),
            decode_time_stamp: CMTimeStruct::invalid(),
        };
        let sizes = [framed.len()];
        let mut sample: CMSampleBufferRef = std::ptr::null_mut();
        let status = unsafe {
            CMSampleBufferCreateReady(
                kCFAllocatorDefault,
                block,
                self.format,
                1,
                1,
                std::ptr::addr_of!(timing),
                1,
                sizes.as_ptr(),
                std::ptr::addr_of_mut!(sample),
            )
        };
        unsafe { CFRelease(block as CFTypeRef) };
        if status != 0 || sample.is_null() {
            return Err(VideoError::Refused(status));
        }
        Ok(sample)
    }
}

/// VideoToolbox's output callback, on its own thread.
unsafe extern "C" fn on_picture(
    output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: OSStatus,
    _info_flags: VTDecodeInfoFlags,
    image_buffer: CVImageBufferRef,
    _presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    if status != 0 || image_buffer.is_null() || output_ref_con.is_null() {
        return;
    }
    let decoded = unsafe { &*(output_ref_con as *const Mutex<Decoded>) };
    let picture = unsafe { copy_bgra(image_buffer) };
    if let Some(picture) = picture {
        decoded.lock().expect("decoder output").picture = Some(picture);
    }
}

/// Copies a decoded pixel buffer into tightly packed BGRA.
unsafe fn copy_bgra(buffer: CVImageBufferRef) -> Option<Picture> {
    unsafe {
        let width = CVPixelBufferGetWidth(buffer);
        let height = CVPixelBufferGetHeight(buffer);
        if width == 0 || height == 0 {
            return None;
        }
        // Read-only lock: the flag is kCVPixelBufferLock_ReadOnly, and getting it wrong costs a
        // needless copy-back rather than correctness.
        if CVPixelBufferLockBaseAddress(buffer, 1) != 0 {
            return None;
        }
        let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
        let stride = CVPixelBufferGetBytesPerRow(buffer);
        let mut bgra = Vec::with_capacity(width * height * 4);
        if base.is_null() || stride < width * 4 {
            CVPixelBufferUnlockBaseAddress(buffer, 1);
            return None;
        }
        for y in 0..height {
            let row = std::slice::from_raw_parts(base.add(y * stride), width * 4);
            bgra.extend_from_slice(row);
        }
        CVPixelBufferUnlockBaseAddress(buffer, 1);
        Some(Picture {
            width: width as u32,
            height: height as u32,
            bgra,
        })
    }
}

/// Splits an Annex B buffer into its NAL units, accepting three- and four-byte start codes.
///
/// Returns nothing for a buffer that does not begin with a start code, rather than guessing where
/// the first unit starts.
pub(crate) fn split_annex_b(bytes: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut at = 0;
    while at + 3 <= bytes.len() {
        if bytes[at] == 0 && bytes[at + 1] == 0 {
            if bytes[at + 2] == 1 {
                starts.push((at, at + 3));
                at += 3;
                continue;
            }
            if at + 4 <= bytes.len() && bytes[at + 2] == 0 && bytes[at + 3] == 1 {
                starts.push((at, at + 4));
                at += 4;
                continue;
            }
        }
        at += 1;
    }
    if starts.first().map(|(at, _)| *at) != Some(0) {
        return Vec::new();
    }
    let mut units = Vec::with_capacity(starts.len());
    for (index, (_, from)) in starts.iter().enumerate() {
        let to = starts.get(index + 1).map_or(bytes.len(), |(at, _)| *at);
        if to > *from {
            units.push(&bytes[*from..to]);
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EncoderConfig, HevcEncoder, Request};

    #[test]
    fn annex_b_splits_on_both_start_code_lengths() {
        let bytes = [0, 0, 0, 1, 0x40, 0x01, 0, 0, 1, 0x42, 0, 0, 0, 1, 0x44];
        assert_eq!(
            split_annex_b(&bytes),
            vec![&[0x40u8, 0x01][..], &[0x42][..], &[0x44][..]]
        );
        assert!(
            split_annex_b(&[0x40, 0x01]).is_empty(),
            "a buffer with no start code at all is refused rather than guessed at"
        );
        assert!(
            split_annex_b(&[1, 2, 0, 0, 0, 1, 9]).is_empty(),
            "a start code that is not at the beginning does not make the leading bytes a unit"
        );
        assert!(split_annex_b(&[]).is_empty());
        assert!(
            split_annex_b(&[0, 0, 0, 1]).is_empty(),
            "a start code with nothing after it is not a unit"
        );
    }

    #[test]
    fn parameter_sets_that_are_not_parameter_sets_are_refused() {
        assert_eq!(
            HevcDecoder::open(&[]).unwrap_err(),
            VideoError::InvalidParameterSets
        );
        assert_eq!(
            HevcDecoder::open(&[0, 0, 0, 1, 0xFF, 0xFF]).unwrap_err(),
            VideoError::InvalidParameterSets
        );
    }

    /// A screen-like frame, the same shape the encoder's own tests use.
    fn frame(config: EncoderConfig, step: u32) -> Vec<u8> {
        let (width, height) = (config.width as usize, config.height as usize);
        let mut pixels = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let at = (y * width + x) * 4;
                let row = y / 12;
                let caret = row == (step as usize % 8) + 1 && x > 20 && x < 32;
                let (b, g, r) = if caret {
                    (80u8, 200, 240)
                } else if row % 2 == 0 && x % 7 < 5 {
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
    fn what_the_encoder_produced_is_what_the_decoder_gives_back() {
        let config = EncoderConfig::new(320, 192);
        let Ok(mut encoder) = HevcEncoder::open(config) else {
            eprintln!("no low-latency HEVC encoder on this machine; skipping");
            return;
        };
        let mut encoded = Vec::new();
        for step in 0..8 {
            encoded.extend(
                encoder
                    .encode(
                        &frame(config, step),
                        config.width as usize * 4,
                        Request::default(),
                    )
                    .expect("a frame"),
            );
        }
        encoded.extend(encoder.finish());
        assert!(!encoded.is_empty());

        let sets = encoder.parameter_sets();
        let mut decoder = HevcDecoder::open(&sets).expect("the encoder's own sets open a decoder");
        let mut pictures = 0;
        for frame in &encoded {
            if let Some(picture) = decoder.decode(&frame.payload).expect("a decodable frame") {
                assert_eq!(
                    (picture.width, picture.height),
                    (config.width, config.height)
                );
                assert_eq!(picture.bgra.len(), 320 * 192 * 4);
                // Opaque: the source was opaque and the decoder was asked for BGRA.
                assert!(picture.bgra.chunks_exact(4).all(|pixel| pixel[3] == 255));
                pictures += 1;
            }
        }
        assert_eq!(
            pictures,
            encoded.len(),
            "every frame the encoder produced decoded again"
        );
    }

    #[test]
    fn a_keyframe_decodes_to_something_that_looks_like_the_source() {
        let config = EncoderConfig::new(256, 128);
        let Ok(mut encoder) = HevcEncoder::open(config) else {
            eprintln!("no low-latency HEVC encoder on this machine; skipping");
            return;
        };
        let source = frame(config, 0);
        let mut encoded = encoder
            .encode(&source, config.width as usize * 4, Request::default())
            .expect("a frame");
        encoded.extend(encoder.finish());
        let keyframe = encoded.first().expect("a keyframe");
        assert!(keyframe.keyframe);

        let mut decoder = HevcDecoder::open(&encoder.parameter_sets()).expect("a decoder");
        let picture = decoder
            .decode(&keyframe.payload)
            .expect("decodable")
            .expect("a picture");

        // Lossy, and 4:2:0, so this is not an exact comparison: what matters is that the picture
        // is the source and not noise or a blank frame.
        let difference: u64 = picture
            .bgra
            .chunks_exact(4)
            .zip(source.chunks_exact(4))
            .map(|(decoded, original)| {
                (0..3)
                    .map(|at| decoded[at].abs_diff(original[at]) as u64)
                    .sum::<u64>()
            })
            .sum();
        let average = difference / (picture.bgra.len() as u64 / 4 * 3);
        assert!(
            average < 24,
            "the decoded picture is {average} off the source, which is not the same picture"
        );
    }

    #[test]
    fn a_frame_whose_reference_never_arrived_produces_no_picture() {
        let config = EncoderConfig::new(256, 128);
        let Ok(mut encoder) = HevcEncoder::open(config) else {
            eprintln!("no low-latency HEVC encoder on this machine; skipping");
            return;
        };
        let mut encoded = Vec::new();
        for step in 0..6 {
            encoded.extend(
                encoder
                    .encode(
                        &frame(config, step),
                        config.width as usize * 4,
                        Request::default(),
                    )
                    .expect("a frame"),
            );
        }
        encoded.extend(encoder.finish());
        let Some(later) = encoded.iter().find(|frame| !frame.keyframe) else {
            eprintln!("the encoder produced only keyframes; skipping");
            return;
        };

        // A fresh decoder that never saw the keyframe. It must not invent a picture.
        let mut decoder = HevcDecoder::open(&encoder.parameter_sets()).expect("a decoder");
        assert_eq!(
            decoder.decode(&later.payload).expect("not an error"),
            None,
            "a frame with nothing to predict from must produce nothing to draw"
        );
    }

    #[test]
    fn a_payload_that_is_not_annex_b_is_refused() {
        let config = EncoderConfig::new(128, 64);
        let Ok(encoder) = HevcEncoder::open(config) else {
            return;
        };
        let mut encoder = encoder;
        let _ = encoder.encode(
            &frame(config, 0),
            config.width as usize * 4,
            Request::default(),
        );
        let _ = encoder.finish();
        let Ok(mut decoder) = HevcDecoder::open(&encoder.parameter_sets()) else {
            return;
        };
        assert_eq!(
            decoder.decode(&[0x26, 0x01, 0x02]),
            Err(VideoError::Malformed)
        );
        assert_eq!(decoder.decode(&[]), Err(VideoError::Malformed));
    }
}
