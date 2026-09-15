use std::fmt;
use std::io::{BufRead, Read as _, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use termirust_domain::PairingOfferId;

use crate::{HostPairingDecision, ListenerError, ListenerErrorCode};

const BROKER_SCHEMA_VERSION: u16 = 1;
const MAX_BROKER_LINE_BYTES: u64 = 1024;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostPairingPrompt {
    pub schema_version: u16,
    pub offer_id: PairingOfferId,
    pub sas: String,
    pub expires_at_unix_seconds: u64,
}

impl SshHostPairingPrompt {
    pub fn new(
        offer_id: PairingOfferId,
        sas: String,
        expires_at_unix_seconds: u64,
    ) -> Result<Self, ListenerError> {
        let value = Self {
            schema_version: BROKER_SCHEMA_VERSION,
            offer_id,
            sas,
            expires_at_unix_seconds,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn read(reader: &mut impl BufRead) -> Result<Self, ListenerError> {
        let value: Self = read_line(reader)?;
        value.validate()?;
        Ok(value)
    }

    pub fn write(&self, writer: &mut impl Write) -> Result<(), ListenerError> {
        self.validate()?;
        write_line(self, writer)
    }

    fn validate(&self) -> Result<(), ListenerError> {
        let bytes = self.sas.as_bytes();
        if self.schema_version != BROKER_SCHEMA_VERSION
            || self.expires_at_unix_seconds == 0
            || bytes.len() != 9
            || bytes[4] != b'-'
            || bytes.iter().enumerate().any(|(index, byte)| {
                index != 4 && !byte.is_ascii_uppercase() && !byte.is_ascii_digit()
            })
        {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        Ok(())
    }
}

impl fmt::Debug for SshHostPairingPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshHostPairingPrompt")
            .field("offer_id", &self.offer_id)
            .field("sas", &"[REDACTED]")
            .field("expires_at_unix_seconds", &self.expires_at_unix_seconds)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshHostPairingDecisionValue {
    Confirm,
    Reject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshHostPairingDecision {
    pub schema_version: u16,
    pub offer_id: PairingOfferId,
    pub decision: SshHostPairingDecisionValue,
}

impl SshHostPairingDecision {
    pub fn new(offer_id: PairingOfferId, decision: SshHostPairingDecisionValue) -> Self {
        Self {
            schema_version: BROKER_SCHEMA_VERSION,
            offer_id,
            decision,
        }
    }

    pub fn read(reader: &mut impl BufRead) -> Result<Self, ListenerError> {
        let value: Self = read_line(reader)?;
        if value.schema_version != BROKER_SCHEMA_VERSION {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        Ok(value)
    }

    pub fn write(&self, writer: &mut impl Write) -> Result<(), ListenerError> {
        if self.schema_version != BROKER_SCHEMA_VERSION {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        write_line(self, writer)
    }
}

impl From<SshHostPairingDecisionValue> for HostPairingDecision {
    fn from(value: SshHostPairingDecisionValue) -> Self {
        match value {
            SshHostPairingDecisionValue::Confirm => Self::Confirm,
            SshHostPairingDecisionValue::Reject => Self::Reject,
        }
    }
}

#[cfg(unix)]
pub async fn request_ssh_host_pairing_decision(
    broker_path: &Path,
    prompt: &SshHostPairingPrompt,
) -> Result<HostPairingDecision, ListenerError> {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

    let metadata = std::fs::symlink_metadata(broker_path)
        .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(ListenerError::new(ListenerErrorCode::PermissionDenied));
    }
    let mut stream = tokio::net::UnixStream::connect(broker_path)
        .await
        .map_err(ListenerError::from)?;
    let mut bytes = serde_json::to_vec(prompt)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
    if bytes.len() as u64 >= MAX_BROKER_LINE_BYTES {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .await
        .map_err(ListenerError::from)?;
    stream.flush().await.map_err(ListenerError::from)?;
    let mut reader = BufReader::new(stream.take(MAX_BROKER_LINE_BYTES + 1));
    let mut response = Vec::new();
    reader
        .read_until(b'\n', &mut response)
        .await
        .map_err(ListenerError::from)?;
    if response.is_empty()
        || response.len() as u64 > MAX_BROKER_LINE_BYTES
        || response.last() != Some(&b'\n')
    {
        return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
    }
    let decision: SshHostPairingDecision = serde_json::from_slice(&response)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
    if decision.schema_version != BROKER_SCHEMA_VERSION || decision.offer_id != prompt.offer_id {
        return Err(ListenerError::new(ListenerErrorCode::AuthenticationFailed));
    }
    Ok(decision.decision.into())
}

#[cfg(not(unix))]
pub async fn request_ssh_host_pairing_decision(
    _broker_path: &Path,
    _prompt: &SshHostPairingPrompt,
) -> Result<HostPairingDecision, ListenerError> {
    Err(ListenerError::new(ListenerErrorCode::HostUnavailable))
}

fn read_line<T: for<'de> Deserialize<'de>>(reader: &mut impl BufRead) -> Result<T, ListenerError> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_BROKER_LINE_BYTES + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(ListenerError::from)?;
    if bytes.is_empty()
        || bytes.len() as u64 > MAX_BROKER_LINE_BYTES
        || bytes.last() != Some(&b'\n')
    {
        return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))
}

fn write_line(value: &impl Serialize, writer: &mut impl Write) -> Result<(), ListenerError> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
    if bytes.len() as u64 >= MAX_BROKER_LINE_BYTES {
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
    fn broker_messages_are_bounded_exact_and_redacted() {
        let offer_id = PairingOfferId::new();
        let prompt = SshHostPairingPrompt::new(offer_id, "ABCD-1234".into(), 500).unwrap();
        let mut bytes = Vec::new();
        prompt.write(&mut bytes).unwrap();
        let decoded = SshHostPairingPrompt::read(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded, prompt);
        assert!(!format!("{prompt:?}").contains("ABCD-1234"));

        let decision = SshHostPairingDecision::new(offer_id, SshHostPairingDecisionValue::Reject);
        let mut bytes = Vec::new();
        decision.write(&mut bytes).unwrap();
        assert_eq!(
            SshHostPairingDecision::read(&mut bytes.as_slice()).unwrap(),
            decision
        );
    }
}
