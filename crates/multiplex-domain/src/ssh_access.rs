use serde::{Deserialize, Serialize};

pub const SSH_ACCESS_CONTRACT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthenticationKind {
    Password,
    PrivateKey,
    OpenSshCertificate,
    LocalAgent,
    SecurityKey,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshCertificateSigner {
    PrivateKey,
    LocalAgent,
    SecurityKey,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAgentForwardingPolicy {
    #[default]
    Disabled,
    ConfirmEachConnection,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAccessCapability {
    PasswordAuthentication,
    PrivateKeyAuthentication,
    OpenSshCertificateAuthentication,
    LocalAgentAuthentication,
    SecurityKeyAuthentication,
    AgentForwarding,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshCapabilityAvailability {
    Available,
    ProviderUnavailable,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SshAccessRuntimeCapabilities {
    pub password_authentication: SshCapabilityAvailability,
    pub private_key_authentication: SshCapabilityAvailability,
    pub openssh_certificate_authentication: SshCapabilityAvailability,
    pub local_agent_authentication: SshCapabilityAvailability,
    pub security_key_authentication: SshCapabilityAvailability,
    pub agent_forwarding: SshCapabilityAvailability,
}

impl SshAccessRuntimeCapabilities {
    pub const fn availability(self, capability: SshAccessCapability) -> SshCapabilityAvailability {
        match capability {
            SshAccessCapability::PasswordAuthentication => self.password_authentication,
            SshAccessCapability::PrivateKeyAuthentication => self.private_key_authentication,
            SshAccessCapability::OpenSshCertificateAuthentication => {
                self.openssh_certificate_authentication
            }
            SshAccessCapability::LocalAgentAuthentication => self.local_agent_authentication,
            SshAccessCapability::SecurityKeyAuthentication => self.security_key_authentication,
            SshAccessCapability::AgentForwarding => self.agent_forwarding,
        }
    }

    pub fn projection(self) -> Vec<SshAccessCapabilityState> {
        const CAPABILITIES: [SshAccessCapability; 6] = [
            SshAccessCapability::PasswordAuthentication,
            SshAccessCapability::PrivateKeyAuthentication,
            SshAccessCapability::OpenSshCertificateAuthentication,
            SshAccessCapability::LocalAgentAuthentication,
            SshAccessCapability::SecurityKeyAuthentication,
            SshAccessCapability::AgentForwarding,
        ];
        CAPABILITIES
            .into_iter()
            .map(|capability| SshAccessCapabilityState {
                capability,
                availability: self.availability(capability),
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SshAccessCapabilityState {
    pub capability: SshAccessCapability,
    pub availability: SshCapabilityAvailability,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SshAccessPolicy {
    pub authentication: SshAuthenticationKind,
    pub certificate_signer: Option<SshCertificateSigner>,
    #[serde(default)]
    pub agent_forwarding: SshAgentForwardingPolicy,
}

impl SshAccessPolicy {
    pub const fn legacy_password() -> Self {
        Self {
            authentication: SshAuthenticationKind::Password,
            certificate_signer: None,
            agent_forwarding: SshAgentForwardingPolicy::Disabled,
        }
    }

    pub const fn legacy_private_key() -> Self {
        Self {
            authentication: SshAuthenticationKind::PrivateKey,
            certificate_signer: None,
            agent_forwarding: SshAgentForwardingPolicy::Disabled,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SshAccessMaterial {
    pub password_credential: bool,
    pub private_key: bool,
    pub openssh_certificate: bool,
    pub security_key_handle: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SshAccessRequest {
    pub policy: SshAccessPolicy,
    pub material: SshAccessMaterial,
    pub runtime: SshAccessRuntimeCapabilities,
    #[serde(default)]
    pub agent_forwarding_confirmed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SshAccessPlan {
    pub authentication: SshAuthenticationKind,
    pub certificate_signer: Option<SshCertificateSigner>,
    pub forward_agent: bool,
}

impl SshAccessRequest {
    pub fn validate(self) -> Result<SshAccessPlan, SshAccessError> {
        let SshAccessPolicy {
            authentication,
            certificate_signer,
            agent_forwarding,
        } = self.policy;

        match authentication {
            SshAuthenticationKind::Password => {
                reject_unexpected_signer(certificate_signer)?;
                require_available(self.runtime, SshAccessCapability::PasswordAuthentication)?;
                if !self.material.password_credential {
                    return Err(SshAccessError::MissingPasswordCredential);
                }
            }
            SshAuthenticationKind::PrivateKey => {
                reject_unexpected_signer(certificate_signer)?;
                require_available(self.runtime, SshAccessCapability::PrivateKeyAuthentication)?;
                if !self.material.private_key {
                    return Err(SshAccessError::MissingPrivateKey);
                }
            }
            SshAuthenticationKind::OpenSshCertificate => {
                require_available(
                    self.runtime,
                    SshAccessCapability::OpenSshCertificateAuthentication,
                )?;
                if !self.material.openssh_certificate {
                    return Err(SshAccessError::MissingOpenSshCertificate);
                }
                match certificate_signer.ok_or(SshAccessError::MissingCertificateSigner)? {
                    SshCertificateSigner::PrivateKey => {
                        require_available(
                            self.runtime,
                            SshAccessCapability::PrivateKeyAuthentication,
                        )?;
                        if !self.material.private_key {
                            return Err(SshAccessError::MissingPrivateKey);
                        }
                    }
                    SshCertificateSigner::LocalAgent => require_available(
                        self.runtime,
                        SshAccessCapability::LocalAgentAuthentication,
                    )?,
                    SshCertificateSigner::SecurityKey => {
                        require_available(
                            self.runtime,
                            SshAccessCapability::SecurityKeyAuthentication,
                        )?;
                        if !self.material.security_key_handle {
                            return Err(SshAccessError::MissingSecurityKeyHandle);
                        }
                    }
                }
            }
            SshAuthenticationKind::LocalAgent => {
                reject_unexpected_signer(certificate_signer)?;
                require_available(self.runtime, SshAccessCapability::LocalAgentAuthentication)?;
            }
            SshAuthenticationKind::SecurityKey => {
                reject_unexpected_signer(certificate_signer)?;
                require_available(self.runtime, SshAccessCapability::SecurityKeyAuthentication)?;
                if !self.material.security_key_handle {
                    return Err(SshAccessError::MissingSecurityKeyHandle);
                }
            }
        }

        let forward_agent = match agent_forwarding {
            SshAgentForwardingPolicy::Disabled => false,
            SshAgentForwardingPolicy::ConfirmEachConnection => {
                require_available(self.runtime, SshAccessCapability::LocalAgentAuthentication)?;
                require_available(self.runtime, SshAccessCapability::AgentForwarding)?;
                if !self.agent_forwarding_confirmed {
                    return Err(SshAccessError::AgentForwardingConfirmationRequired);
                }
                true
            }
        };

        Ok(SshAccessPlan {
            authentication,
            certificate_signer,
            forward_agent,
        })
    }
}

fn reject_unexpected_signer(signer: Option<SshCertificateSigner>) -> Result<(), SshAccessError> {
    if signer.is_some() {
        Err(SshAccessError::UnexpectedCertificateSigner)
    } else {
        Ok(())
    }
}

fn require_available(
    runtime: SshAccessRuntimeCapabilities,
    capability: SshAccessCapability,
) -> Result<(), SshAccessError> {
    match runtime.availability(capability) {
        SshCapabilityAvailability::Available => Ok(()),
        availability => Err(SshAccessError::CapabilityUnavailable {
            capability,
            availability,
        }),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshAccessError {
    MissingPasswordCredential,
    MissingPrivateKey,
    MissingOpenSshCertificate,
    MissingCertificateSigner,
    MissingSecurityKeyHandle,
    UnexpectedCertificateSigner,
    AgentForwardingConfirmationRequired,
    CapabilityUnavailable {
        capability: SshAccessCapability,
        availability: SshCapabilityAvailability,
    },
}

impl SshAccessError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::MissingPasswordCredential => "ssh_password_credential_missing",
            Self::MissingPrivateKey => "ssh_private_key_missing",
            Self::MissingOpenSshCertificate => "ssh_certificate_missing",
            Self::MissingCertificateSigner => "ssh_certificate_signer_missing",
            Self::MissingSecurityKeyHandle => "ssh_security_key_handle_missing",
            Self::UnexpectedCertificateSigner => "ssh_certificate_signer_unexpected",
            Self::AgentForwardingConfirmationRequired => {
                "ssh_agent_forwarding_confirmation_required"
            }
            Self::CapabilityUnavailable { .. } => "ssh_capability_unavailable",
        }
    }
}

impl std::fmt::Display for SshAccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for SshAccessError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Fixture {
        schema_version: u32,
        cases: Vec<FixtureCase>,
        projections: Vec<ProjectionCase>,
    }

    #[derive(Deserialize)]
    struct FixtureCase {
        name: String,
        request: SshAccessRequest,
        expected_plan: Option<SshAccessPlan>,
        expected_error: Option<String>,
    }

    #[derive(Deserialize)]
    struct ProjectionCase {
        name: String,
        runtime: SshAccessRuntimeCapabilities,
        expected: Vec<SshAccessCapabilityState>,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/ssh-access/access-policy-v1.json"
        ))
        .unwrap()
    }

    #[test]
    fn shared_access_policy_fixture_passes_normal_and_hostile_cases() {
        let fixture = fixture();
        assert_eq!(fixture.schema_version, SSH_ACCESS_CONTRACT_VERSION);
        for case in fixture.cases {
            match (case.request.validate(), case.expected_plan) {
                (Ok(actual), Some(expected)) => assert_eq!(actual, expected, "{}", case.name),
                (Err(error), None) => {
                    assert_eq!(
                        error.code(),
                        case.expected_error.as_deref().unwrap(),
                        "{}",
                        case.name
                    )
                }
                result => panic!("{}: unexpected result {result:?}", case.name),
            }
        }
    }

    #[test]
    fn capability_projection_preserves_unavailable_reasons() {
        for case in fixture().projections {
            assert_eq!(case.runtime.projection(), case.expected, "{}", case.name);
        }
    }

    #[test]
    fn legacy_policies_disable_agent_forwarding() {
        assert_eq!(
            SshAccessPolicy::legacy_password().agent_forwarding,
            SshAgentForwardingPolicy::Disabled
        );
        assert_eq!(
            SshAccessPolicy::legacy_private_key().agent_forwarding,
            SshAgentForwardingPolicy::Disabled
        );
    }

    #[test]
    fn serialized_contract_contains_presence_only_and_no_secret_locations() {
        let request = SshAccessRequest {
            policy: SshAccessPolicy::legacy_private_key(),
            material: SshAccessMaterial {
                private_key: true,
                ..SshAccessMaterial::default()
            },
            runtime: SshAccessRuntimeCapabilities {
                password_authentication: SshCapabilityAvailability::Available,
                private_key_authentication: SshCapabilityAvailability::Available,
                openssh_certificate_authentication: SshCapabilityAvailability::Available,
                local_agent_authentication: SshCapabilityAvailability::ProviderUnavailable,
                security_key_authentication: SshCapabilityAvailability::Unsupported,
                agent_forwarding: SshCapabilityAvailability::ProviderUnavailable,
            },
            agent_forwarding_confirmed: false,
        };
        let encoded = serde_json::to_string(&request).unwrap();
        for forbidden in [
            "BEGIN OPENSSH PRIVATE KEY",
            "SSH_AUTH_SOCK",
            "/Users/",
            "/home/",
            "password-value",
            "certificate-body",
        ] {
            assert!(!encoded.contains(forbidden));
        }
    }
}
