use crate::models::AgentProvider;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentLaunchModes {
    pub interactive_pty: bool,
    pub structured_events: bool,
    pub approvals: bool,
    pub cancellation: bool,
    pub context_handoff: bool,
    pub remote: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentProviderDescriptor {
    pub provider: AgentProvider,
    pub executable: Option<&'static str>,
    pub version_argument: &'static str,
    pub install_guidance: &'static str,
    /// Implementation routes, not detected semantic capabilities.
    pub launch_modes: AgentLaunchModes,
}

pub fn provider_descriptor(provider: AgentProvider) -> AgentProviderDescriptor {
    match provider {
        AgentProvider::Codex => AgentProviderDescriptor {
            provider,
            executable: Some("codex"),
            version_argument: "--version",
            install_guidance: "Install Codex CLI from https://developers.openai.com/codex/cli, then select Check again.",
            launch_modes: AgentLaunchModes {
                interactive_pty: true,
                structured_events: true,
                approvals: true,
                cancellation: true,
                context_handoff: true,
                remote: true,
            },
        },
        AgentProvider::ClaudeCode => AgentProviderDescriptor {
            provider,
            executable: Some("claude"),
            version_argument: "--version",
            install_guidance: "Install Claude Code from https://code.claude.com/docs/en/setup, then select Check again.",
            launch_modes: AgentLaunchModes {
                interactive_pty: true,
                structured_events: true,
                approvals: false,
                cancellation: true,
                context_handoff: true,
                remote: true,
            },
        },
        AgentProvider::Gemini => AgentProviderDescriptor {
            provider,
            executable: Some("gemini"),
            version_argument: "--version",
            install_guidance: "Install Gemini CLI from https://geminicli.com/docs/get-started/installation, then select Check again.",
            launch_modes: AgentLaunchModes {
                interactive_pty: true,
                structured_events: true,
                approvals: false,
                cancellation: true,
                context_handoff: true,
                remote: true,
            },
        },
        AgentProvider::CustomCli => AgentProviderDescriptor {
            provider,
            executable: None,
            version_argument: "--version",
            install_guidance: "Choose an installed executable. TermiRust will not install or run it through a shell.",
            launch_modes: AgentLaunchModes {
                interactive_pty: true,
                remote: true,
                ..AgentLaunchModes::default()
            },
        },
        AgentProvider::GroqApi => AgentProviderDescriptor {
            provider,
            executable: None,
            version_argument: "--version",
            install_guidance: "Groq API agents are not available in this release. Use a reviewed Custom CLI executable instead.",
            launch_modes: AgentLaunchModes::default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::provider_descriptor;
    use crate::models::AgentProvider;

    #[test]
    fn provider_capabilities_do_not_overstate_custom_or_groq_support() {
        let custom = provider_descriptor(AgentProvider::CustomCli);
        assert!(custom.launch_modes.interactive_pty);
        assert!(!custom.launch_modes.structured_events);
        assert!(!custom.launch_modes.approvals);

        let groq = provider_descriptor(AgentProvider::GroqApi);
        assert!(!groq.launch_modes.interactive_pty);
        assert!(!groq.launch_modes.structured_events);
    }

    #[test]
    fn official_cli_presets_support_interactive_and_structured_modes() {
        for provider in [
            AgentProvider::Codex,
            AgentProvider::ClaudeCode,
            AgentProvider::Gemini,
        ] {
            let descriptor = provider_descriptor(provider);
            assert!(descriptor.executable.is_some());
            assert!(descriptor.launch_modes.interactive_pty);
            assert!(descriptor.launch_modes.structured_events);
            assert!(descriptor.launch_modes.cancellation);
        }
    }

    #[test]
    fn headless_adapters_do_not_implement_interactive_approval() {
        for provider in [AgentProvider::ClaudeCode, AgentProvider::Gemini] {
            let descriptor = provider_descriptor(provider);
            assert!(!descriptor.launch_modes.approvals);
        }
        let codex = provider_descriptor(AgentProvider::Codex);
        assert!(codex.launch_modes.approvals);
    }
}
