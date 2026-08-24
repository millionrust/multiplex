use termirust_domain::OutputSequence;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachPhase {
    Provisioning,
    Attaching,
    Replaying,
    Live,
    RecordingPaused,
    Offline,
    Dead,
    Gap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputDisposition {
    Deliver,
    Duplicate,
    Gap {
        expected: OutputSequence,
        received: OutputSequence,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuiAttachModel {
    phase: AttachPhase,
    watermark: OutputSequence,
    replayed_records: u64,
    replayed_bytes: u64,
    has_writer_lease: bool,
}

impl GpuiAttachModel {
    pub fn new(watermark: OutputSequence) -> Self {
        Self {
            phase: AttachPhase::Attaching,
            watermark,
            replayed_records: 0,
            replayed_bytes: 0,
            has_writer_lease: false,
        }
    }

    pub const fn phase(&self) -> AttachPhase {
        self.phase
    }

    pub const fn watermark(&self) -> OutputSequence {
        self.watermark
    }

    pub const fn replayed_records(&self) -> u64 {
        self.replayed_records
    }

    pub const fn replayed_bytes(&self) -> u64 {
        self.replayed_bytes
    }

    pub const fn has_writer_lease(&self) -> bool {
        self.has_writer_lease
    }

    pub fn begin_replay(&mut self) {
        self.phase = AttachPhase::Replaying;
        self.replayed_records = 0;
        self.replayed_bytes = 0;
    }

    pub fn apply_snapshot(&mut self, boundary: OutputSequence) -> bool {
        if boundary < self.watermark {
            return false;
        }
        self.watermark = boundary;
        self.begin_replay();
        true
    }

    pub fn observe_output(
        &mut self,
        sequence: OutputSequence,
        byte_count: usize,
    ) -> OutputDisposition {
        if sequence <= self.watermark {
            return OutputDisposition::Duplicate;
        }
        let Some(expected) = self.watermark.checked_next() else {
            self.phase = AttachPhase::Gap;
            return OutputDisposition::Gap {
                expected: self.watermark,
                received: sequence,
            };
        };
        if sequence != expected {
            self.phase = AttachPhase::Gap;
            return OutputDisposition::Gap {
                expected,
                received: sequence,
            };
        }
        self.watermark = sequence;
        self.replayed_records = self.replayed_records.saturating_add(1);
        self.replayed_bytes = self.replayed_bytes.saturating_add(byte_count as u64);
        OutputDisposition::Deliver
    }

    pub fn mark_live(&mut self, has_writer_lease: bool, recording_paused: bool) {
        self.has_writer_lease = has_writer_lease;
        self.phase = if recording_paused {
            AttachPhase::RecordingPaused
        } else {
            AttachPhase::Live
        };
    }

    pub fn mark_offline(&mut self) {
        self.has_writer_lease = false;
        self.phase = AttachPhase::Offline;
    }

    pub fn mark_dead(&mut self) {
        self.has_writer_lease = false;
        self.phase = AttachPhase::Dead;
    }
}
