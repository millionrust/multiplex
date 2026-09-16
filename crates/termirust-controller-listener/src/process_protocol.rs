use std::io::{BufRead, Read as _, Write};

use serde::{Deserialize, Serialize};
use termirust_domain::{ControllerDeviceId, ListeningAddress, PairingOfferId};

use crate::{FirewallObservation, ListenerError, ListenerErrorCode};

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
    /// Opens pairing mode with a new six-digit code.
    BeginCodePairing {
        schema_version: u16,
    },
    /// Closes pairing mode and discards its code.
    CancelCodePairing {
        schema_version: u16,
        offer_id: PairingOfferId,
    },
}

impl ListenerControlCommand {
    pub fn begin_pairing() -> Self {
        Self::BeginPairing {
            schema_version: PROCESS_PROTOCOL_VERSION,
        }
    }

    pub fn begin_code_pairing() -> Self {
        Self::BeginCodePairing {
            schema_version: PROCESS_PROTOCOL_VERSION,
        }
    }

    pub fn cancel_code_pairing(offer_id: PairingOfferId) -> Self {
        Self::CancelCodePairing {
            schema_version: PROCESS_PROTOCOL_VERSION,
            offer_id,
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
            Self::BeginPairing { schema_version }
            | Self::BeginCodePairing { schema_version }
            | Self::DecidePairing { schema_version, .. }
            | Self::CancelCodePairing { schema_version, .. } => *schema_version,
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ListenerProcessEvent {
    Ready {
        schema_version: u16,
        port: u16,
        addresses: Vec<ListeningAddress>,
        firewall: ProcessFirewallObservation,
    },
    /// The private addresses the listener accepts on changed after it became ready.
    ListeningAddresses {
        schema_version: u16,
        addresses: Vec<ListeningAddress>,
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
    /// Pairing mode is open. The code is shown on the desktop only.
    PairingCode {
        schema_version: u16,
        offer_id: PairingOfferId,
        code: String,
        expires_at_unix_seconds: u64,
        attempts_left: u8,
    },
    /// A phone tried the code and did not pair; the code stays open while attempts remain.
    PairingCodeAttemptFailed {
        schema_version: u16,
        offer_id: PairingOfferId,
        attempts_left: u8,
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
    /// Who is watching this computer's screens, whenever that changes.
    ScreenWatchers {
        schema_version: u16,
        watchers: Vec<crate::ScreenWatcherReport>,
    },
}

impl ListenerProcessEvent {
    pub fn ready(port: u16, addresses: Vec<ListeningAddress>) -> Self {
        Self::ready_with_firewall(port, addresses, FirewallObservation::Unknown)
    }

    pub fn ready_with_firewall(
        port: u16,
        addresses: Vec<ListeningAddress>,
        firewall: FirewallObservation,
    ) -> Self {
        Self::Ready {
            schema_version: PROCESS_PROTOCOL_VERSION,
            port,
            addresses,
            firewall: firewall.into(),
        }
    }

    pub fn listening_addresses(addresses: Vec<ListeningAddress>) -> Self {
        Self::ListeningAddresses {
            schema_version: PROCESS_PROTOCOL_VERSION,
            addresses,
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

    pub fn pairing_code(
        offer_id: PairingOfferId,
        code: String,
        expires_at_unix_seconds: u64,
        attempts_left: u8,
    ) -> Self {
        Self::PairingCode {
            schema_version: PROCESS_PROTOCOL_VERSION,
            offer_id,
            code,
            expires_at_unix_seconds,
            attempts_left,
        }
    }

    pub fn pairing_code_attempt_failed(offer_id: PairingOfferId, attempts_left: u8) -> Self {
        Self::PairingCodeAttemptFailed {
            schema_version: PROCESS_PROTOCOL_VERSION,
            offer_id,
            attempts_left,
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

    /// Reports who is watching this computer's screens, newest listing wins.
    pub fn screen_watchers(watchers: Vec<crate::ScreenWatcherReport>) -> Self {
        Self::ScreenWatchers {
            schema_version: PROCESS_PROTOCOL_VERSION,
            watchers,
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
            | Self::ListeningAddresses { schema_version, .. }
            | Self::PairingOffer { schema_version, .. }
            | Self::PairingSasReady { schema_version, .. }
            | Self::PairingCode { schema_version, .. }
            | Self::PairingCodeAttemptFailed { schema_version, .. }
            | Self::PairingComplete { schema_version, .. }
            | Self::PairingFailed { schema_version, .. }
            | Self::ScreenWatchers { schema_version, .. } => *schema_version,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessFirewallObservation {
    Allowed,
    Blocked,
    Unknown,
}

impl From<FirewallObservation> for ProcessFirewallObservation {
    fn from(value: FirewallObservation) -> Self {
        match value {
            FirewallObservation::Allowed => Self::Allowed,
            FirewallObservation::Blocked => Self::Blocked,
            FirewallObservation::Unknown => Self::Unknown,
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
                    Self::BeginCodePairing { .. } => "begin_code_pairing",
                    Self::CancelCodePairing { .. } => "cancel_code_pairing",
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
            Self::ListeningAddresses { .. } => "listening_addresses",
            Self::PairingOffer { .. } => "pairing_offer",
            Self::PairingSasReady { .. } => "pairing_sas_ready",
            Self::PairingCode { .. } => "pairing_code",
            Self::PairingCodeAttemptFailed { .. } => "pairing_code_attempt_failed",
            Self::PairingComplete { .. } => "pairing_complete",
            Self::PairingFailed { .. } => "pairing_failed",
            Self::ScreenWatchers { .. } => "screen_watchers",
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
    fn watcher_reports_round_trip_and_carry_no_screen_content() {
        let watching = ListenerProcessEvent::screen_watchers(vec![crate::ScreenWatcherReport {
            device_id: termirust_domain::ControllerDeviceId::new(),
            controlling: true,
        }]);
        let mut bytes = Vec::new();
        watching.write(&mut bytes).unwrap();
        assert_eq!(
            ListenerProcessEvent::read(&mut bytes.as_slice()).unwrap(),
            Some(watching.clone())
        );
        assert!(
            !format!("{watching:?}").contains("device_id"),
            "debug output stays redacted"
        );

        let empty = ListenerProcessEvent::screen_watchers(Vec::new());
        let mut bytes = Vec::new();
        empty.write(&mut bytes).unwrap();
        assert_eq!(
            ListenerProcessEvent::read(&mut bytes.as_slice()).unwrap(),
            Some(empty),
            "the last watcher leaving is itself a report"
        );
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
