use std::io::{BufRead, Read as _, Write};

use serde::{Deserialize, Serialize};
use termirust_domain::{ControllerDeviceId, PairingOfferId};

use crate::{ListenerError, ListenerErrorCode};

const PROCESS_PROTOCOL_VERSION: u16 = 1;
const MAX_PROCESS_LINE_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPairingDecision {
    Confirm,
    Reject,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ListenerControlCommand {
    BeginPairing {
        schema_version: u16,
    },
    DecidePairing {
        schema_version: u16,
        offer_id: PairingOfferId,
        decision: ProcessPairingDecision,
    },
}

impl ListenerControlCommand {
    pub fn begin_pairing() -> Self {
        Self::BeginPairing {
            schema_version: PROCESS_PROTOCOL_VERSION,
        }
    }

    pub fn decide_pairing(offer_id: PairingOfferId, decision: ProcessPairingDecision) -> Self {
        Self::DecidePairing {
            schema_version: PROCESS_PROTOCOL_VERSION,
            offer_id,
            decision,
        }
    }

    pub fn read(reader: &mut impl BufRead) -> Result<Option<Self>, ListenerError> {
        let Some(bytes) = read_line(reader)? else {
            return Ok(None);
        };
        let command: Self = serde_json::from_slice(&bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        if command.schema_version() != PROCESS_PROTOCOL_VERSION {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        Ok(Some(command))
    }

    pub fn write(&self, writer: &mut impl Write) -> Result<(), ListenerError> {
        if self.schema_version() != PROCESS_PROTOCOL_VERSION {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        write_line(self, writer)
    }

    const fn schema_version(&self) -> u16 {
        match self {
            Self::BeginPairing { schema_version } | Self::DecidePairing { schema_version, .. } => {
                *schema_version
            }
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ListenerProcessEvent {
    Ready {
        schema_version: u16,
        port: u16,
    },
    PairingOffer {
        schema_version: u16,
        offer_id: PairingOfferId,
        offer_text: String,
        expires_at_unix_seconds: u64,
    },
    PairingSasReady {
        schema_version: u16,
        offer_id: PairingOfferId,
        sas: String,
    },
    PairingComplete {
        schema_version: u16,
        offer_id: PairingOfferId,
        device_id: ControllerDeviceId,
    },
    PairingFailed {
        schema_version: u16,
        offer_id: Option<PairingOfferId>,
        code: String,
    },
}

impl ListenerProcessEvent {
    pub fn ready(port: u16) -> Self {
        Self::Ready {
            schema_version: PROCESS_PROTOCOL_VERSION,
            port,
        }
    }

    pub fn pairing_offer(
        offer_id: PairingOfferId,
        offer_text: String,
        expires_at_unix_seconds: u64,
    ) -> Self {
        Self::PairingOffer {
            schema_version: PROCESS_PROTOCOL_VERSION,
            offer_id,
            offer_text,
            expires_at_unix_seconds,
        }
    }

    pub fn pairing_sas_ready(offer_id: PairingOfferId, sas: String) -> Self {
        Self::PairingSasReady {
            schema_version: PROCESS_PROTOCOL_VERSION,
            offer_id,
            sas,
        }
    }

    pub fn pairing_complete(offer_id: PairingOfferId, device_id: ControllerDeviceId) -> Self {
        Self::PairingComplete {
            schema_version: PROCESS_PROTOCOL_VERSION,
            offer_id,
            device_id,
        }
    }

    pub fn pairing_failed(offer_id: Option<PairingOfferId>, code: &str) -> Self {
        Self::PairingFailed {
            schema_version: PROCESS_PROTOCOL_VERSION,
            offer_id,
            code: code.to_owned(),
        }
    }

    pub fn read(reader: &mut impl BufRead) -> Result<Option<Self>, ListenerError> {
        let Some(bytes) = read_line(reader)? else {
            return Ok(None);
        };
        let event: Self = serde_json::from_slice(&bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        if event.schema_version() != PROCESS_PROTOCOL_VERSION {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        Ok(Some(event))
    }

    pub fn write(&self, writer: &mut impl Write) -> Result<(), ListenerError> {
        if self.schema_version() != PROCESS_PROTOCOL_VERSION {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        write_line(self, writer)
    }

    const fn schema_version(&self) -> u16 {
        match self {
            Self::Ready { schema_version, .. }
            | Self::PairingOffer { schema_version, .. }
            | Self::PairingSasReady { schema_version, .. }
            | Self::PairingComplete { schema_version, .. }
            | Self::PairingFailed { schema_version, .. } => *schema_version,
        }
    }
}

fn read_line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, ListenerError> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_PROCESS_LINE_BYTES + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(ListenerError::from)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() as u64 > MAX_PROCESS_LINE_BYTES || bytes.last() != Some(&b'\n') {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    Ok(Some(bytes))
}

impl std::fmt::Debug for ProcessPairingDecision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Confirm => "Confirm",
            Self::Reject => "Reject",
        })
    }
}

impl std::fmt::Debug for ListenerControlCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ListenerControlCommand")
            .field(
                "kind",
                &match self {
                    Self::BeginPairing { .. } => "begin_pairing",
                    Self::DecidePairing { .. } => "decide_pairing",
                },
            )
            .field("payload", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for ListenerProcessEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::Ready { .. } => "ready",
            Self::PairingOffer { .. } => "pairing_offer",
            Self::PairingSasReady { .. } => "pairing_sas_ready",
            Self::PairingComplete { .. } => "pairing_complete",
            Self::PairingFailed { .. } => "pairing_failed",
        };
        formatter
            .debug_struct("ListenerProcessEvent")
            .field("kind", &kind)
            .field("payload", &"[REDACTED]")
            .finish()
    }
}

fn write_line(value: &impl Serialize, writer: &mut impl Write) -> Result<(), ListenerError> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
    if bytes.len() as u64 >= MAX_PROCESS_LINE_BYTES {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    bytes.push(b'\n');
    writer.write_all(&bytes).map_err(ListenerError::from)?;
    writer.flush().map_err(ListenerError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_commands_and_events_round_trip_and_reject_unknown_fields() {
        let offer_id = PairingOfferId::new();
        let command =
            ListenerControlCommand::decide_pairing(offer_id, ProcessPairingDecision::Confirm);
        let mut bytes = Vec::new();
        command.write(&mut bytes).unwrap();
        assert_eq!(
            ListenerControlCommand::read(&mut bytes.as_slice()).unwrap(),
            Some(command)
        );

        let event = ListenerProcessEvent::pairing_sas_ready(offer_id, "ABCD-1234".into());
        let mut bytes = Vec::new();
        event.write(&mut bytes).unwrap();
        assert_eq!(
            ListenerProcessEvent::read(&mut bytes.as_slice()).unwrap(),
            Some(event)
        );

        let hostile = b"{\"kind\":\"begin_pairing\",\"schema_version\":1,\"extra\":true}\n";
        assert!(ListenerControlCommand::read(&mut &hostile[..]).is_err());
    }

    #[test]
    fn process_protocol_rejects_oversize_and_unterminated_lines() {
        let oversized = vec![b'x'; MAX_PROCESS_LINE_BYTES as usize + 1];
        assert_eq!(
            ListenerControlCommand::read(&mut oversized.as_slice())
                .unwrap_err()
                .code,
            ListenerErrorCode::FrameTooLarge
        );
        assert!(ListenerControlCommand::read(&mut &b"{}"[..]).is_err());
    }
}
