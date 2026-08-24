use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use termirust_domain::OutputSequence;

use crate::{AtomicWriter, HostLease, SystemAtomicWriter};

pub const JOURNAL_MAGIC: [u8; 4] = *b"TRJ1";
pub const SNAPSHOT_MAGIC: [u8; 4] = *b"TRS1";
pub const JOURNAL_HEADER_BYTES: usize = 32;
pub const JOURNAL_CHECKSUM_BYTES: usize = 4;
pub const MAX_JOURNAL_FRAME_BYTES: usize = 1024 * 1024;
pub const DEFAULT_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_JOURNAL_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_RETAINED_SEGMENTS: usize = 4;
pub const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const FORMAT_VERSION: u16 = 1;
const NONCRITICAL_FLAG: u32 = 1;
const SNAPSHOT_HEADER_BYTES: usize = 28;
const RECOVERY_FILE: &str = "recovery-tail.trj";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum JournalKind {
    Output = 1,
    Lifecycle = 2,
    Resize = 3,
    Exit = 4,
    Warning = 5,
}

impl TryFrom<u16> for JournalKind {
    type Error = JournalError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Output),
            2 => Ok(Self::Lifecycle),
            3 => Ok(Self::Resize),
            4 => Ok(Self::Exit),
            5 => Ok(Self::Warning),
            _ => Err(JournalError::new(JournalErrorCode::UnknownCriticalKind)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalFrame {
    pub kind: JournalKind,
    pub sequence: OutputSequence,
    pub monotonic_nanos: u64,
    pub flags: u32,
    pub payload: Vec<u8>,
}

impl JournalFrame {
    pub fn encoded_len(&self) -> Result<usize, JournalError> {
        JOURNAL_HEADER_BYTES
            .checked_add(self.payload.len())
            .and_then(|value| value.checked_add(JOURNAL_CHECKSUM_BYTES))
            .filter(|value| *value <= MAX_JOURNAL_FRAME_BYTES)
            .ok_or_else(|| JournalError::new(JournalErrorCode::FrameTooLarge))
    }

    pub fn encode(&self) -> Result<Vec<u8>, JournalError> {
        if self.sequence == OutputSequence::ZERO {
            return Err(JournalError::new(JournalErrorCode::Sequence));
        }
        let encoded_len = self.encoded_len()?;
        let payload_len = u32::try_from(self.payload.len())
            .map_err(|_| JournalError::new(JournalErrorCode::FrameTooLarge))?;
        let mut bytes = Vec::with_capacity(encoded_len);
        bytes.extend_from_slice(&JOURNAL_MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
        bytes.extend_from_slice(&(self.kind as u16).to_be_bytes());
        bytes.extend_from_slice(&self.sequence.get().to_be_bytes());
        bytes.extend_from_slice(&self.monotonic_nanos.to_be_bytes());
        bytes.extend_from_slice(&payload_len.to_be_bytes());
        bytes.extend_from_slice(&self.flags.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes.extend_from_slice(&crc32c::crc32c(&bytes).to_be_bytes());
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalErrorCode {
    Io,
    UnsafeEntry,
    FrameTooLarge,
    Malformed,
    Checksum,
    Sequence,
    UnknownCriticalKind,
    ResourceLimit,
    HistoryUnavailable,
    LeaseMismatch,
}

#[derive(Debug)]
pub struct JournalError {
    pub code: JournalErrorCode,
    pub io_kind: Option<io::ErrorKind>,
    pub expected_sequence: Option<OutputSequence>,
}

impl JournalError {
    fn new(code: JournalErrorCode) -> Self {
        Self {
            code,
            io_kind: None,
            expected_sequence: None,
        }
    }

    fn io(error: io::Error) -> Self {
        Self {
            code: JournalErrorCode::Io,
            io_kind: Some(error.kind()),
            expected_sequence: None,
        }
    }

    fn sequence(expected: OutputSequence) -> Self {
        Self {
            code: JournalErrorCode::Sequence,
            io_kind: None,
            expected_sequence: Some(expected),
        }
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            JournalErrorCode::Io => "session journal I/O failed",
            JournalErrorCode::UnsafeEntry => "session journal contains an unsafe entry",
            JournalErrorCode::FrameTooLarge => "session journal frame exceeds its limit",
            JournalErrorCode::Malformed => "session journal frame is malformed",
            JournalErrorCode::Checksum => "session journal checksum does not match",
            JournalErrorCode::Sequence => "session journal sequence is not contiguous",
            JournalErrorCode::UnknownCriticalKind => {
                "session journal contains an unknown critical record"
            }
            JournalErrorCode::ResourceLimit => "session journal reached its resource limit",
            JournalErrorCode::HistoryUnavailable => "requested session history is unavailable",
            JournalErrorCode::LeaseMismatch => "session journal lease does not match its Host",
        })
    }
}

impl std::error::Error for JournalError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JournalLimits {
    pub segment_bytes: u64,
    pub hard_bytes: u64,
    pub retained_segments: usize,
}

impl Default for JournalLimits {
    fn default() -> Self {
        Self {
            segment_bytes: DEFAULT_SEGMENT_BYTES,
            hard_bytes: DEFAULT_JOURNAL_BYTES,
            retained_segments: DEFAULT_RETAINED_SEGMENTS,
        }
    }
}

impl JournalLimits {
    pub fn validate(self) -> Result<Self, JournalError> {
        if self.segment_bytes < MAX_JOURNAL_FRAME_BYTES as u64
            || self.hard_bytes < self.segment_bytes
            || self.hard_bytes > DEFAULT_JOURNAL_BYTES
            || !(1..=DEFAULT_RETAINED_SEGMENTS).contains(&self.retained_segments)
        {
            return Err(JournalError::new(JournalErrorCode::ResourceLimit));
        }
        Ok(self)
    }

    #[cfg(test)]
    fn fixture(segment_bytes: u64, hard_bytes: u64, retained_segments: usize) -> Self {
        Self {
            segment_bytes,
            hard_bytes,
            retained_segments,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    Recorded,
    RecordingPausedDiskLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanIssue {
    TornTail,
    Corrupt,
    SequenceGap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalScan {
    pub frames: Vec<JournalFrame>,
    pub valid_bytes: usize,
    pub issue: Option<ScanIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSnapshot {
    pub boundary: OutputSequence,
    pub columns: u32,
    pub rows: u32,
    pub terminal_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalRead {
    pub earliest: Option<OutputSequence>,
    pub latest: Option<OutputSequence>,
    pub frames: Vec<JournalFrame>,
    pub has_gap: bool,
}

pub struct JournalStore {
    session_dir: PathBuf,
    limits: JournalLimits,
    active_index: u32,
    active: File,
    active_bytes: u64,
    total_bytes: u64,
    latest_sequence: OutputSequence,
    base_sequence: OutputSequence,
    recording_paused: bool,
    has_gap: bool,
}

impl fmt::Debug for JournalStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JournalStore")
            .field("session_dir", &"[REDACTED]")
            .field("active_index", &self.active_index)
            .field("active_bytes", &self.active_bytes)
            .field("total_bytes", &self.total_bytes)
            .field("latest_sequence", &self.latest_sequence)
            .field("recording_paused", &self.recording_paused)
            .field("has_gap", &self.has_gap)
            .finish()
    }
}

impl JournalStore {
    pub fn open(lease: &HostLease, limits: JournalLimits) -> Result<Self, JournalError> {
        let limits = limits.validate()?;
        let session_dir = lease.session_dir().to_path_buf();
        let mut segments = segment_paths(&session_dir)?;
        if segments.is_empty() {
            segments.push((1, segment_path(&session_dir, 1)));
        }
        let base_sequence = load_snapshot(&session_dir)?
            .map(|snapshot| snapshot.boundary)
            .unwrap_or(OutputSequence::ZERO);
        let mut total_bytes = 0_u64;
        let mut latest_sequence = base_sequence;
        let mut has_gap = false;
        for (position, (_, path)) in segments.iter().enumerate() {
            let is_last = position + 1 == segments.len();
            if !path.exists() {
                continue;
            }
            let bytes = read_regular_bounded(path, limits.segment_bytes)?;
            total_bytes = total_bytes
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| JournalError::new(JournalErrorCode::ResourceLimit))?;
            let scan = scan_journal_bytes(&bytes, latest_sequence)?;
            if let Some(last) = scan.frames.last() {
                latest_sequence = last.sequence;
            }
            if scan.issue.is_some() {
                if is_last {
                    preserve_and_truncate_tail(&session_dir, path, &bytes, scan.valid_bytes)?;
                    total_bytes =
                        total_bytes.saturating_sub((bytes.len() - scan.valid_bytes) as u64);
                } else {
                    has_gap = true;
                    break;
                }
            }
        }
        if total_bytes > limits.hard_bytes {
            has_gap = true;
        }
        let (active_index, active_path) = segments
            .last()
            .cloned()
            .ok_or_else(|| JournalError::new(JournalErrorCode::Malformed))?;
        let active = open_append_file(&active_path)?;
        let active_bytes = active.metadata().map_err(JournalError::io)?.len();
        Ok(Self {
            session_dir,
            limits,
            active_index,
            active,
            active_bytes,
            total_bytes,
            latest_sequence,
            base_sequence,
            recording_paused: has_gap || total_bytes >= limits.hard_bytes,
            has_gap,
        })
    }

    pub fn latest_sequence(&self) -> OutputSequence {
        self.latest_sequence
    }

    pub fn recording_paused(&self) -> bool {
        self.recording_paused
    }

    pub fn has_gap(&self) -> bool {
        self.has_gap
    }

    pub fn compaction_due(&self) -> bool {
        let total_trigger = self.limits.segment_bytes.saturating_mul(3);
        self.active_bytes >= self.limits.segment_bytes || self.total_bytes >= total_trigger
    }

    pub fn append(&mut self, frame: &JournalFrame) -> Result<AppendOutcome, JournalError> {
        let expected = self
            .latest_sequence
            .checked_next()
            .ok_or_else(|| JournalError::new(JournalErrorCode::ResourceLimit))?;
        if frame.sequence != expected {
            return Err(JournalError::sequence(expected));
        }
        if self.recording_paused {
            return Ok(AppendOutcome::RecordingPausedDiskLimit);
        }
        let bytes = frame.encode()?;
        let encoded_len = bytes.len() as u64;
        if self.total_bytes.saturating_add(encoded_len) > self.limits.hard_bytes {
            self.recording_paused = true;
            return Ok(AppendOutcome::RecordingPausedDiskLimit);
        }
        if self.active_bytes > 0
            && self.active_bytes.saturating_add(encoded_len) > self.limits.segment_bytes
        {
            self.rotate()?;
        }
        write_frame_bytes(&mut self.active, &bytes)?;
        self.active_bytes = self.active_bytes.saturating_add(encoded_len);
        self.total_bytes = self.total_bytes.saturating_add(encoded_len);
        self.latest_sequence = frame.sequence;
        if matches!(frame.kind, JournalKind::Lifecycle | JournalKind::Exit) {
            self.active.sync_data().map_err(JournalError::io)?;
        }
        Ok(AppendOutcome::Recorded)
    }

    pub fn sync(&mut self) -> Result<OutputSequence, JournalError> {
        self.active.sync_data().map_err(JournalError::io)?;
        Ok(self.latest_sequence)
    }

    pub fn read_from(&self, from: OutputSequence) -> Result<JournalRead, JournalError> {
        let mut frames = Vec::new();
        let mut expected = self.base_sequence;
        let mut has_gap = self.has_gap;
        for (_, path) in segment_paths(&self.session_dir)? {
            let bytes = read_regular_bounded(&path, self.limits.segment_bytes)?;
            let scan = scan_journal_bytes(&bytes, expected)?;
            if let Some(last) = scan.frames.last() {
                expected = last.sequence;
            }
            has_gap |= scan.issue.is_some();
            frames.extend(scan.frames);
            if scan.issue.is_some() {
                break;
            }
        }
        let earliest = frames
            .first()
            .map(|frame| frame.sequence)
            .or_else(|| (self.base_sequence != OutputSequence::ZERO).then_some(self.base_sequence));
        let latest = frames
            .last()
            .map(|frame| frame.sequence)
            .or_else(|| (self.base_sequence != OutputSequence::ZERO).then_some(self.base_sequence));
        if from < self.base_sequence
            || earliest.is_some_and(|earliest| {
                from.checked_next()
                    .is_some_and(|requested| requested < earliest)
            })
        {
            return Err(JournalError {
                code: JournalErrorCode::HistoryUnavailable,
                io_kind: None,
                expected_sequence: earliest,
            });
        }
        frames.retain(|frame| frame.sequence > from);
        Ok(JournalRead {
            earliest,
            latest,
            frames,
            has_gap,
        })
    }

    pub fn compact(&mut self, snapshot: &TerminalSnapshot) -> Result<(), JournalError> {
        if snapshot.boundary == OutputSequence::ZERO
            || snapshot.boundary != self.latest_sequence
            || snapshot.columns == 0
            || snapshot.rows == 0
        {
            return Err(JournalError::new(JournalErrorCode::Malformed));
        }
        let bytes = encode_snapshot(snapshot)?;
        SystemAtomicWriter
            .write(&self.session_dir.join("snapshot.trs"), &bytes)
            .map_err(JournalError::io)?;
        if self.active_bytes > 0 {
            self.rotate()?;
        }
        let mut previous = self.base_sequence;
        for (index, path) in segment_paths(&self.session_dir)? {
            if index == self.active_index {
                continue;
            }
            let bytes = read_regular_bounded(&path, self.limits.segment_bytes)?;
            let scan = scan_journal_bytes(&bytes, previous)?;
            if let Some(last) = scan.frames.last() {
                previous = last.sequence;
            }
            if scan
                .frames
                .last()
                .is_some_and(|frame| frame.sequence <= snapshot.boundary)
                && scan.issue.is_none()
            {
                fs::remove_file(&path).map_err(JournalError::io)?;
                self.total_bytes = self.total_bytes.saturating_sub(bytes.len() as u64);
            }
        }
        self.base_sequence = snapshot.boundary;
        if !self.has_gap && self.total_bytes < self.limits.hard_bytes {
            self.recording_paused = false;
        }
        Ok(())
    }

    fn rotate(&mut self) -> Result<(), JournalError> {
        self.active.sync_data().map_err(JournalError::io)?;
        let next = self
            .active_index
            .checked_add(1)
            .ok_or_else(|| JournalError::new(JournalErrorCode::ResourceLimit))?;
        let path = segment_path(&self.session_dir, next);
        if path.exists() {
            return Err(JournalError::new(JournalErrorCode::UnsafeEntry));
        }
        self.active = open_append_file(&path)?;
        self.active_index = next;
        self.active_bytes = 0;
        if segment_paths(&self.session_dir)?.len() > self.limits.retained_segments {
            self.recording_paused = true;
        }
        Ok(())
    }
}

pub fn scan_journal_bytes(
    bytes: &[u8],
    previous_sequence: OutputSequence,
) -> Result<JournalScan, JournalError> {
    let mut offset = 0_usize;
    let mut previous = previous_sequence;
    let mut frames = Vec::new();
    while offset < bytes.len() {
        if bytes.len() - offset < JOURNAL_HEADER_BYTES + JOURNAL_CHECKSUM_BYTES {
            return Ok(JournalScan {
                frames,
                valid_bytes: offset,
                issue: Some(ScanIssue::TornTail),
            });
        }
        let header = &bytes[offset..offset + JOURNAL_HEADER_BYTES];
        if header[..4] != JOURNAL_MAGIC
            || u16::from_be_bytes([header[4], header[5]]) != FORMAT_VERSION
        {
            return Ok(JournalScan {
                frames,
                valid_bytes: offset,
                issue: Some(ScanIssue::Corrupt),
            });
        }
        let raw_kind = u16::from_be_bytes([header[6], header[7]]);
        let sequence = OutputSequence::new(u64::from_be_bytes(
            header[8..16]
                .try_into()
                .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
        ));
        let monotonic_nanos = u64::from_be_bytes(
            header[16..24]
                .try_into()
                .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
        );
        let payload_len = usize::try_from(u32::from_be_bytes(
            header[24..28]
                .try_into()
                .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
        ))
        .map_err(|_| JournalError::new(JournalErrorCode::FrameTooLarge))?;
        let flags = u32::from_be_bytes(
            header[28..32]
                .try_into()
                .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
        );
        let frame_len = JOURNAL_HEADER_BYTES
            .checked_add(payload_len)
            .and_then(|value| value.checked_add(JOURNAL_CHECKSUM_BYTES))
            .filter(|value| *value <= MAX_JOURNAL_FRAME_BYTES)
            .ok_or_else(|| JournalError::new(JournalErrorCode::FrameTooLarge))?;
        if bytes.len() - offset < frame_len {
            return Ok(JournalScan {
                frames,
                valid_bytes: offset,
                issue: Some(ScanIssue::TornTail),
            });
        }
        let frame_end = offset + frame_len;
        let checksum_offset = frame_end - JOURNAL_CHECKSUM_BYTES;
        let expected_checksum = u32::from_be_bytes(
            bytes[checksum_offset..frame_end]
                .try_into()
                .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
        );
        if crc32c::crc32c(&bytes[offset..checksum_offset]) != expected_checksum {
            return Ok(JournalScan {
                frames,
                valid_bytes: offset,
                issue: Some(ScanIssue::Corrupt),
            });
        }
        let expected_sequence = previous
            .checked_next()
            .ok_or_else(|| JournalError::new(JournalErrorCode::ResourceLimit))?;
        if sequence != expected_sequence {
            return Ok(JournalScan {
                frames,
                valid_bytes: offset,
                issue: Some(ScanIssue::SequenceGap),
            });
        }
        let kind = match JournalKind::try_from(raw_kind) {
            Ok(kind) => kind,
            Err(_) if flags & NONCRITICAL_FLAG != 0 => {
                previous = sequence;
                offset = frame_end;
                continue;
            }
            Err(error) => return Err(error),
        };
        frames.push(JournalFrame {
            kind,
            sequence,
            monotonic_nanos,
            flags,
            payload: bytes[offset + JOURNAL_HEADER_BYTES..checksum_offset].to_vec(),
        });
        previous = sequence;
        offset = frame_end;
    }
    Ok(JournalScan {
        frames,
        valid_bytes: offset,
        issue: None,
    })
}

pub fn encode_snapshot(snapshot: &TerminalSnapshot) -> Result<Vec<u8>, JournalError> {
    if snapshot.boundary == OutputSequence::ZERO
        || snapshot.columns == 0
        || snapshot.rows == 0
        || snapshot.terminal_bytes.len() > MAX_SNAPSHOT_BYTES
    {
        return Err(JournalError::new(JournalErrorCode::ResourceLimit));
    }
    let payload_len = u32::try_from(snapshot.terminal_bytes.len())
        .map_err(|_| JournalError::new(JournalErrorCode::ResourceLimit))?;
    let mut bytes = Vec::with_capacity(SNAPSHOT_HEADER_BYTES + snapshot.terminal_bytes.len() + 4);
    bytes.extend_from_slice(&SNAPSHOT_MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    bytes.extend_from_slice(&0_u16.to_be_bytes());
    bytes.extend_from_slice(&snapshot.boundary.get().to_be_bytes());
    bytes.extend_from_slice(&snapshot.columns.to_be_bytes());
    bytes.extend_from_slice(&snapshot.rows.to_be_bytes());
    bytes.extend_from_slice(&payload_len.to_be_bytes());
    bytes.extend_from_slice(&snapshot.terminal_bytes);
    bytes.extend_from_slice(&crc32c::crc32c(&bytes).to_be_bytes());
    Ok(bytes)
}

pub fn decode_snapshot(bytes: &[u8]) -> Result<TerminalSnapshot, JournalError> {
    if bytes.len() < SNAPSHOT_HEADER_BYTES + 4
        || bytes[..4] != SNAPSHOT_MAGIC
        || u16::from_be_bytes([bytes[4], bytes[5]]) != FORMAT_VERSION
    {
        return Err(JournalError::new(JournalErrorCode::Malformed));
    }
    let payload_len = usize::try_from(u32::from_be_bytes(
        bytes[24..28]
            .try_into()
            .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
    ))
    .map_err(|_| JournalError::new(JournalErrorCode::ResourceLimit))?;
    let expected_len = SNAPSHOT_HEADER_BYTES
        .checked_add(payload_len)
        .and_then(|value| value.checked_add(4))
        .filter(|value| *value <= SNAPSHOT_HEADER_BYTES + MAX_SNAPSHOT_BYTES + 4)
        .ok_or_else(|| JournalError::new(JournalErrorCode::ResourceLimit))?;
    if bytes.len() != expected_len {
        return Err(JournalError::new(JournalErrorCode::Malformed));
    }
    let checksum_offset = bytes.len() - 4;
    let expected_checksum = u32::from_be_bytes(
        bytes[checksum_offset..]
            .try_into()
            .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
    );
    if crc32c::crc32c(&bytes[..checksum_offset]) != expected_checksum {
        return Err(JournalError::new(JournalErrorCode::Checksum));
    }
    Ok(TerminalSnapshot {
        boundary: OutputSequence::new(u64::from_be_bytes(
            bytes[8..16]
                .try_into()
                .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
        )),
        columns: u32::from_be_bytes(
            bytes[16..20]
                .try_into()
                .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
        ),
        rows: u32::from_be_bytes(
            bytes[20..24]
                .try_into()
                .map_err(|_| JournalError::new(JournalErrorCode::Malformed))?,
        ),
        terminal_bytes: bytes[SNAPSHOT_HEADER_BYTES..checksum_offset].to_vec(),
    })
}

pub fn load_snapshot(session_dir: &Path) -> Result<Option<TerminalSnapshot>, JournalError> {
    let path = session_dir.join("snapshot.trs");
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(JournalError::new(JournalErrorCode::UnsafeEntry));
            }
            if metadata.len() > (SNAPSHOT_HEADER_BYTES + MAX_SNAPSHOT_BYTES + 4) as u64 {
                return Err(JournalError::new(JournalErrorCode::ResourceLimit));
            }
            let bytes = fs::read(path).map_err(JournalError::io)?;
            decode_snapshot(&bytes).map(Some)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(JournalError::io(error)),
    }
}

fn segment_paths(session_dir: &Path) -> Result<Vec<(u32, PathBuf)>, JournalError> {
    let mut segments = Vec::new();
    for entry in fs::read_dir(session_dir).map_err(JournalError::io)? {
        let entry = entry.map_err(JournalError::io)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(number) = name
            .strip_prefix("journal-")
            .and_then(|value| value.strip_suffix(".trj"))
            .and_then(|value| (value.len() == 6).then_some(value))
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let metadata = fs::symlink_metadata(entry.path()).map_err(JournalError::io)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(JournalError::new(JournalErrorCode::UnsafeEntry));
        }
        segments.push((number, entry.path()));
    }
    segments.sort_by_key(|(number, _)| *number);
    Ok(segments)
}

fn segment_path(session_dir: &Path, index: u32) -> PathBuf {
    session_dir.join(format!("journal-{index:06}.trj"))
}

fn open_append_file(path: &Path) -> Result<File, JournalError> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path).map_err(JournalError::io)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(JournalError::io)?;
    Ok(file)
}

fn read_regular_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, JournalError> {
    let metadata = fs::symlink_metadata(path).map_err(JournalError::io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(JournalError::new(JournalErrorCode::UnsafeEntry));
    }
    if metadata.len() > limit {
        return Err(JournalError::new(JournalErrorCode::ResourceLimit));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(usize::MAX));
    File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(JournalError::io)?;
    if bytes.len() as u64 > limit {
        return Err(JournalError::new(JournalErrorCode::ResourceLimit));
    }
    Ok(bytes)
}

fn preserve_and_truncate_tail(
    session_dir: &Path,
    active_path: &Path,
    bytes: &[u8],
    valid_bytes: usize,
) -> Result<(), JournalError> {
    let tail = &bytes[valid_bytes..];
    if !tail.is_empty() {
        let bounded = &tail[..tail.len().min(MAX_JOURNAL_FRAME_BYTES)];
        SystemAtomicWriter
            .write(&session_dir.join(RECOVERY_FILE), bounded)
            .map_err(JournalError::io)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .open(active_path)
        .map_err(JournalError::io)?;
    file.set_len(valid_bytes as u64).map_err(JournalError::io)?;
    file.seek(SeekFrom::Start(valid_bytes as u64))
        .map_err(JournalError::io)?;
    file.sync_data().map_err(JournalError::io)
}

fn write_frame_bytes(writer: &mut impl Write, bytes: &[u8]) -> Result<(), JournalError> {
    writer.write_all(bytes).map_err(JournalError::io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::HostInstanceId;

    struct ShortWriter {
        remaining: usize,
    }

    impl Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "fixture short write",
                ));
            }
            let count = bytes.len().min(self.remaining);
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn frame(sequence: u64, payload: &[u8]) -> JournalFrame {
        JournalFrame {
            kind: JournalKind::Output,
            sequence: OutputSequence::new(sequence),
            monotonic_nanos: sequence,
            flags: 0,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn frame_scan_returns_longest_valid_prefix_for_corrupt_and_torn_tails() {
        let first = frame(1, b"first").encode().unwrap();
        let mut second = frame(2, b"second").encode().unwrap();
        let mut bytes = first.clone();
        bytes.extend_from_slice(&second[..second.len() - 2]);
        let torn = scan_journal_bytes(&bytes, OutputSequence::ZERO).unwrap();
        assert_eq!(torn.frames.len(), 1);
        assert_eq!(torn.valid_bytes, first.len());
        assert_eq!(torn.issue, Some(ScanIssue::TornTail));

        *second.last_mut().unwrap() ^= 1;
        let mut bytes = first;
        bytes.extend_from_slice(&second);
        let corrupt = scan_journal_bytes(&bytes, OutputSequence::ZERO).unwrap();
        assert_eq!(corrupt.frames.len(), 1);
        assert_eq!(corrupt.issue, Some(ScanIssue::Corrupt));
    }

    #[test]
    fn active_tail_recovers_with_evidence_and_continues_strict_sequence() {
        let fixture = tempfile::tempdir().unwrap();
        let lease =
            HostLease::acquire(fixture.path().join("session"), HostInstanceId::new()).unwrap();
        let limits = JournalLimits::fixture(1024 * 1024, 2 * 1024 * 1024, 2);
        let mut store = JournalStore::open(&lease, limits).unwrap();
        store.append(&frame(1, b"valid")).unwrap();
        store.active.write_all(b"torn").unwrap();
        drop(store);

        let mut recovered = JournalStore::open(&lease, limits).unwrap();
        assert_eq!(recovered.latest_sequence(), OutputSequence::new(1));
        assert!(lease.session_dir().join(RECOVERY_FILE).exists());
        recovered.append(&frame(2, b"continued")).unwrap();
        assert_eq!(
            recovered
                .read_from(OutputSequence::ZERO)
                .unwrap()
                .frames
                .len(),
            2
        );
    }

    #[test]
    fn segment_rotation_quota_pause_snapshot_and_compaction_are_bounded() {
        let fixture = tempfile::tempdir().unwrap();
        let lease =
            HostLease::acquire(fixture.path().join("session"), HostInstanceId::new()).unwrap();
        let limits = JournalLimits::fixture(1024 * 1024, 2 * 1024 * 1024, 2);
        let payload = vec![7; MAX_JOURNAL_FRAME_BYTES - JOURNAL_HEADER_BYTES - 4];
        let mut store = JournalStore::open(&lease, limits).unwrap();
        assert_eq!(
            store.append(&frame(1, &payload)).unwrap(),
            AppendOutcome::Recorded
        );
        assert_eq!(
            store.append(&frame(2, &payload)).unwrap(),
            AppendOutcome::Recorded
        );
        assert_eq!(
            store.append(&frame(3, b"over quota")).unwrap(),
            AppendOutcome::RecordingPausedDiskLimit
        );
        let snapshot = TerminalSnapshot {
            boundary: OutputSequence::new(2),
            columns: 80,
            rows: 24,
            terminal_bytes: b"screen".to_vec(),
        };
        store.compact(&snapshot).unwrap();
        assert_eq!(load_snapshot(lease.session_dir()).unwrap(), Some(snapshot));
        assert!(segment_paths(lease.session_dir()).unwrap().len() <= limits.retained_segments);
        let error = store.read_from(OutputSequence::ZERO).unwrap_err();
        assert_eq!(error.code, JournalErrorCode::HistoryUnavailable);
        assert_eq!(error.expected_sequence, Some(OutputSequence::new(2)));
    }

    #[test]
    fn journal_files_are_user_only_and_wrong_sequence_has_recovery_data() {
        let fixture = tempfile::tempdir().unwrap();
        let lease =
            HostLease::acquire(fixture.path().join("session"), HostInstanceId::new()).unwrap();
        let mut store = JournalStore::open(&lease, JournalLimits::default()).unwrap();
        let error = store.append(&frame(2, b"gap")).unwrap_err();
        assert_eq!(error.code, JournalErrorCode::Sequence);
        assert_eq!(error.expected_sequence, Some(OutputSequence::new(1)));
        assert_eq!(
            fs::metadata(segment_path(lease.session_dir(), 1))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn exact_maximum_frame_round_trips_and_one_byte_over_fails() {
        let exact = frame(
            1,
            &vec![0; MAX_JOURNAL_FRAME_BYTES - JOURNAL_HEADER_BYTES - JOURNAL_CHECKSUM_BYTES],
        );
        assert_eq!(exact.encode().unwrap().len(), MAX_JOURNAL_FRAME_BYTES);
        let over = frame(
            1,
            &vec![0; MAX_JOURNAL_FRAME_BYTES - JOURNAL_HEADER_BYTES - JOURNAL_CHECKSUM_BYTES + 1],
        );
        assert_eq!(
            over.encode().unwrap_err().code,
            JournalErrorCode::FrameTooLarge
        );
    }

    #[test]
    fn every_truncation_returns_only_a_valid_prefix() {
        let mut bytes = frame(1, b"first").encode().unwrap();
        bytes.extend_from_slice(&frame(2, b"second").encode().unwrap());
        for length in 0..bytes.len() {
            let scan = scan_journal_bytes(&bytes[..length], OutputSequence::ZERO).unwrap();
            assert!(scan.valid_bytes <= length);
            assert!(scan.issue.is_some() || scan.valid_bytes == length);
            assert!(
                scan.frames.windows(2).all(|frames| {
                    frames[0].sequence.checked_next() == Some(frames[1].sequence)
                })
            );
        }
    }

    #[test]
    fn short_write_is_reported_and_sealed_corruption_becomes_a_bounded_gap() {
        let encoded = frame(1, b"payload").encode().unwrap();
        let error = write_frame_bytes(&mut ShortWriter { remaining: 5 }, &encoded).unwrap_err();
        assert_eq!(error.code, JournalErrorCode::Io);
        assert_eq!(error.io_kind, Some(io::ErrorKind::WriteZero));

        let fixture = tempfile::tempdir().unwrap();
        let lease =
            HostLease::acquire(fixture.path().join("session"), HostInstanceId::new()).unwrap();
        let limits = JournalLimits::fixture(1024 * 1024, 3 * 1024 * 1024, 3);
        let payload = vec![9; MAX_JOURNAL_FRAME_BYTES - JOURNAL_HEADER_BYTES - 4];
        let mut store = JournalStore::open(&lease, limits).unwrap();
        store.append(&frame(1, &payload)).unwrap();
        store.append(&frame(2, b"second segment")).unwrap();
        drop(store);
        let first = segment_path(lease.session_dir(), 1);
        let mut bytes = fs::read(&first).unwrap();
        bytes[JOURNAL_HEADER_BYTES] ^= 1;
        fs::write(&first, bytes).unwrap();
        let recovered = JournalStore::open(&lease, limits).unwrap();
        assert!(recovered.has_gap());
        assert!(recovered.recording_paused());
        assert!(recovered.read_from(OutputSequence::ZERO).unwrap().has_gap);
    }

    #[test]
    #[ignore = "repository journal throughput benchmark"]
    fn journal_encode_scan_throughput_meets_reference_target() {
        for mebibytes in [1_usize, 64] {
            let frame_count = mebibytes * 16;
            let payload = vec![0x5a; 64 * 1024 - JOURNAL_HEADER_BYTES - 4];
            let started = std::time::Instant::now();
            let mut bytes = Vec::with_capacity(mebibytes * 1024 * 1024);
            for sequence in 1..=frame_count {
                bytes.extend_from_slice(&frame(sequence as u64, &payload).encode().unwrap());
            }
            let scan = scan_journal_bytes(&bytes, OutputSequence::ZERO).unwrap();
            let seconds = started.elapsed().as_secs_f64();
            let mebibytes_per_second = bytes.len() as f64 / (1024.0 * 1024.0) / seconds;
            println!(
                "journal_benchmark size_mib={mebibytes} throughput_mib_s={mebibytes_per_second:.1}"
            );
            assert_eq!(scan.frames.len(), frame_count);
            assert!(
                mebibytes_per_second >= 100.0,
                "journal throughput {mebibytes_per_second:.1} MiB/s was below target"
            );
        }
    }
}
