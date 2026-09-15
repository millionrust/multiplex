use termirust_domain::OutputSequence;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceDecision {
    Accept,
    IdenticalDuplicate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceError {
    Gap {
        expected: OutputSequence,
        actual: OutputSequence,
    },
    ConflictingDuplicate {
        sequence: OutputSequence,
    },
    Overflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SequenceTracker {
    last_sequence: OutputSequence,
    last_hash: Option<u32>,
}

impl SequenceTracker {
    pub const fn new(last_sequence: OutputSequence) -> Self {
        Self {
            last_sequence,
            last_hash: None,
        }
    }

    pub fn with_last(last_sequence: OutputSequence, payload: &[u8]) -> Self {
        Self {
            last_sequence,
            last_hash: Some(crc32c::crc32c(payload)),
        }
    }

    pub const fn last_sequence(self) -> OutputSequence {
        self.last_sequence
    }

    pub fn observe(
        &mut self,
        sequence: OutputSequence,
        payload: &[u8],
    ) -> Result<SequenceDecision, SequenceError> {
        let hash = crc32c::crc32c(payload);
        if sequence == self.last_sequence {
            return if self.last_hash == Some(hash) {
                Ok(SequenceDecision::IdenticalDuplicate)
            } else {
                Err(SequenceError::ConflictingDuplicate { sequence })
            };
        }
        let expected = self
            .last_sequence
            .checked_next()
            .ok_or(SequenceError::Overflow)?;
        if sequence != expected {
            return Err(SequenceError::Gap {
                expected,
                actual: sequence,
            });
        }
        self.last_sequence = sequence;
        self.last_hash = Some(hash);
        Ok(SequenceDecision::Accept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_accepts_identical_duplicate_and_rejects_gap_or_conflict() {
        let mut tracker = SequenceTracker::new(OutputSequence::ZERO);
        assert_eq!(
            tracker.observe(OutputSequence::new(1), b"one"),
            Ok(SequenceDecision::Accept)
        );
        assert_eq!(
            tracker.observe(OutputSequence::new(1), b"one"),
            Ok(SequenceDecision::IdenticalDuplicate)
        );
        assert_eq!(
            tracker.observe(OutputSequence::new(1), b"changed"),
            Err(SequenceError::ConflictingDuplicate {
                sequence: OutputSequence::new(1)
            })
        );
        assert_eq!(
            tracker.observe(OutputSequence::new(3), b"three"),
            Err(SequenceError::Gap {
                expected: OutputSequence::new(2),
                actual: OutputSequence::new(3)
            })
        );
    }
}
