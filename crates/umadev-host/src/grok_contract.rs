//! Pinned, source-audited compatibility classification for Grok Build.
//!
//! This module performs no I/O and no runtime capability probe. It only compares
//! an ACP `initialize` result with the upstream source baseline that UmaDev has
//! audited. A positive result means that UmaDev may apply the corresponding
//! source-shaped parser or attempt an optional private method. It does not replace
//! standard ACP capability negotiation, a method response, or an end-to-end test.

use semver::{Version, VersionReq};
use serde_json::Value;

/// Official Grok Build source repository used by this compatibility profile.
pub const GROK_BUILD_SOURCE_REPOSITORY: &str = "https://github.com/xai-org/grok-build";

/// Exact upstream commit audited for this compatibility profile.
pub const GROK_BUILD_SOURCE_COMMIT: &str = "8adf9013a0929e5c7f1d4e849492d2387837a28d";

/// Grok Build version declared by the audited upstream commit.
pub const GROK_BUILD_SOURCE_VERSION: &str = "0.2.101";

/// `agent-client-protocol` version used by the audited upstream commit.
pub const GROK_BUILD_SOURCE_ACP_VERSION: &str = "0.10.4";

/// `agent-client-protocol-schema` version resolved by the audited upstream lockfile.
pub const GROK_BUILD_SOURCE_ACP_SCHEMA_VERSION: &str = "0.11.4";

/// Conservative SemVer requirement for source-specific behavior.
///
/// The range is intentionally exact. A later prerelease or the stable release
/// for the same patch may change a private `x.ai` payload without changing ACP
/// protocol V1. Such a version remains outside the audited range until the drift
/// audit and acceptance matrix are rerun.
pub const GROK_BUILD_AUDITED_VERSION_REQUIREMENT: &str = "=0.2.101";

const MAX_AGENT_VERSION_BYTES: usize = 128;

/// Why an initialize result did or did not enter the audited source profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrokSourceMatch {
    /// `_meta.grokShell` was not exactly `true`.
    NotGrokShell,
    /// A Grok shell identity was present, but `agentVersion` was absent or empty.
    MissingAgentVersion,
    /// `agentVersion` was not a bounded, valid semantic version.
    MalformedAgentVersion,
    /// The version parsed, but is outside the exact source-audited range.
    OutsideAuditedRange,
    /// The reported version label is inside the source-audited range.
    ///
    /// This is a static classification, not proof of the running binary's commit
    /// and not evidence that an optional method probe succeeded.
    AuditedVersion,
}

/// One Grok-private behavior whose wire contract is tied to audited source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrokSourceCapability {
    /// Image prompt blocks accepted despite the pinned initialize response omitting
    /// the standard image flag.
    ImagePromptFallback,
    /// Agent-selected authentication through `_meta.defaultAuthMethodId`.
    DefaultAuthMethod,
    /// Private `x.ai/ask_user_question` reverse requests.
    AskUserQuestion,
    /// Private `x.ai/exit_plan_mode` reverse requests.
    ExitPlanMode,
    /// Private `x.ai/interject` requests and their queued acknowledgement shape.
    Interject,
    /// Server-authoritative `x.ai/queue/*` snapshots and mutations.
    PromptQueue,
    /// Reverse `x.ai/folder_trust/request` with explicit human settlement.
    FolderTrust,
    /// Rich live and persisted `x.ai/session_*` update rails.
    RichSessionUpdates,
    /// Source-specific replay ordering around advertised standard `session/load`.
    SessionLoadReplay,
    /// Whole-prompt `_meta.usage` and durable turn usage semantics.
    PromptUsage,
    /// Background task lifecycle carried by rich private updates.
    BackgroundTasks,
    /// Native `x.ai/task/list` and `x.ai/task/kill` background-process control.
    BackgroundProcessControl,
    /// Native subagent lifecycle carried by rich private updates.
    SubagentLifecycle,
    /// Source-specific incremental terminal/tool output semantics.
    IncrementalTerminalOutput,
    /// Model state and available-command metadata plus their private updates.
    ModelAndCommandCatalog,
}

impl GrokSourceCapability {
    /// Every source-specific capability represented by this profile.
    pub const ALL: [Self; 15] = [
        Self::ImagePromptFallback,
        Self::DefaultAuthMethod,
        Self::AskUserQuestion,
        Self::ExitPlanMode,
        Self::Interject,
        Self::PromptQueue,
        Self::FolderTrust,
        Self::RichSessionUpdates,
        Self::SessionLoadReplay,
        Self::PromptUsage,
        Self::BackgroundTasks,
        Self::BackgroundProcessControl,
        Self::SubagentLifecycle,
        Self::IncrementalTerminalOutput,
        Self::ModelAndCommandCatalog,
    ];

    const fn bit(self) -> u16 {
        match self {
            Self::ImagePromptFallback => 1 << 0,
            Self::DefaultAuthMethod => 1 << 1,
            Self::AskUserQuestion => 1 << 2,
            Self::ExitPlanMode => 1 << 3,
            Self::Interject => 1 << 4,
            Self::RichSessionUpdates => 1 << 5,
            Self::SessionLoadReplay => 1 << 6,
            Self::PromptUsage => 1 << 7,
            Self::BackgroundTasks => 1 << 8,
            Self::SubagentLifecycle => 1 << 9,
            Self::IncrementalTerminalOutput => 1 << 10,
            Self::ModelAndCommandCatalog => 1 << 11,
            Self::PromptQueue => 1 << 12,
            Self::FolderTrust => 1 << 13,
            Self::BackgroundProcessControl => 1 << 14,
        }
    }
}

/// Per-capability source contract enabled by a [`GrokSourceProfile`].
///
/// This set is deliberately separate from negotiated ACP capabilities. For
/// example, [`GrokSourceCapability::SessionLoadReplay`] describes how the pinned
/// source behaves, while `initialize.agentCapabilities.loadSession` still decides
/// whether the client may send `session/load` at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrokSourceCapabilities {
    bits: u16,
}

impl GrokSourceCapabilities {
    /// No source-specific behavior is enabled.
    pub const NONE: Self = Self { bits: 0 };

    const AUDITED_BASELINE: Self = Self {
        bits: (1 << 15) - 1,
    };

    /// Whether the pinned source contract covers one private behavior.
    ///
    /// A `true` value authorizes source-shaped handling only. It does not claim
    /// that standard negotiation or an optional runtime method probe occurred.
    #[must_use]
    pub const fn contains(self, capability: GrokSourceCapability) -> bool {
        self.bits & capability.bit() != 0
    }

    /// Whether no source-specific behavior is enabled.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }
}

impl Default for GrokSourceCapabilities {
    fn default() -> Self {
        Self::NONE
    }
}

/// Static source-compatibility profile derived from an ACP initialize result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokSourceProfile {
    source_match: GrokSourceMatch,
    reported_version: Option<Version>,
    capabilities: GrokSourceCapabilities,
}

impl GrokSourceProfile {
    fn disabled(source_match: GrokSourceMatch, reported_version: Option<Version>) -> Self {
        Self {
            source_match,
            reported_version,
            capabilities: GrokSourceCapabilities::NONE,
        }
    }

    fn audited(reported_version: Version) -> Self {
        Self {
            source_match: GrokSourceMatch::AuditedVersion,
            reported_version: Some(reported_version),
            capabilities: GrokSourceCapabilities::AUDITED_BASELINE,
        }
    }

    /// Source classification for the initialize result.
    #[must_use]
    pub const fn source_match(&self) -> GrokSourceMatch {
        self.source_match
    }

    /// Parsed version reported by the agent, when it was valid SemVer.
    #[must_use]
    pub fn reported_version(&self) -> Option<&Version> {
        self.reported_version.as_ref()
    }

    /// Source-specific capability set selected by the audited version gate.
    #[must_use]
    pub const fn capabilities(&self) -> GrokSourceCapabilities {
        self.capabilities
    }

    /// Whether one private behavior has an audited source contract.
    ///
    /// This does not assert that a runtime probe or standard capability
    /// negotiation succeeded.
    #[must_use]
    pub const fn supports(&self, capability: GrokSourceCapability) -> bool {
        self.capabilities.contains(capability)
    }

    /// Whether the reported version label falls in the audited source range.
    #[must_use]
    pub const fn is_audited_version(&self) -> bool {
        matches!(self.source_match, GrokSourceMatch::AuditedVersion)
    }
}

/// Classify a Grok Build ACP initialize result against the pinned source audit.
///
/// Only `_meta.grokShell` and `_meta.agentVersion` are read. The function is
/// deterministic and side-effect free: it does not execute Grok, contact xAI,
/// authenticate, or probe an extension method.
#[must_use]
pub fn source_profile_from_initialize(initialize: &Value) -> GrokSourceProfile {
    if initialize
        .pointer("/_meta/grokShell")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return GrokSourceProfile::disabled(GrokSourceMatch::NotGrokShell, None);
    }

    let Some(raw_version_value) = initialize.pointer("/_meta/agentVersion") else {
        return GrokSourceProfile::disabled(GrokSourceMatch::MissingAgentVersion, None);
    };
    let Some(raw_version) = raw_version_value.as_str() else {
        return GrokSourceProfile::disabled(GrokSourceMatch::MalformedAgentVersion, None);
    };
    if raw_version.is_empty() {
        return GrokSourceProfile::disabled(GrokSourceMatch::MissingAgentVersion, None);
    }

    if raw_version.len() > MAX_AGENT_VERSION_BYTES {
        return GrokSourceProfile::disabled(GrokSourceMatch::MalformedAgentVersion, None);
    }

    let Ok(version) = Version::parse(raw_version) else {
        return GrokSourceProfile::disabled(GrokSourceMatch::MalformedAgentVersion, None);
    };
    let in_audited_range = VersionReq::parse(GROK_BUILD_AUDITED_VERSION_REQUIREMENT)
        .is_ok_and(|requirement| requirement.matches(&version));

    // Build metadata was not present in the audited version label. Reject it
    // rather than treating an unexamined artifact label as the pinned source.
    if in_audited_range && version.build.is_empty() {
        GrokSourceProfile::audited(version)
    } else {
        GrokSourceProfile::disabled(GrokSourceMatch::OutsideAuditedRange, Some(version))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn profile(version: &str) -> GrokSourceProfile {
        source_profile_from_initialize(&json!({
            "_meta": {"grokShell": true, "agentVersion": version}
        }))
    }

    fn assert_private_capabilities_disabled(profile: &GrokSourceProfile) {
        assert!(profile.capabilities().is_empty());
        for capability in GrokSourceCapability::ALL {
            assert!(!profile.supports(capability), "{capability:?}");
        }
    }

    #[test]
    fn exact_audited_version_enables_each_source_contract() {
        let profile = profile(GROK_BUILD_SOURCE_VERSION);

        assert_eq!(profile.source_match(), GrokSourceMatch::AuditedVersion);
        assert_eq!(
            profile.reported_version(),
            Some(&Version::parse(GROK_BUILD_SOURCE_VERSION).unwrap())
        );
        assert!(profile.is_audited_version());
        for capability in GrokSourceCapability::ALL {
            assert!(profile.supports(capability), "{capability:?}");
        }
    }

    #[test]
    fn later_same_patch_prerelease_is_outside_exact_audit() {
        for version in ["0.1.220-alpha.5", "0.1.220-beta.1", "0.1.220-rc.1"] {
            let profile = profile(version);
            assert_eq!(profile.source_match(), GrokSourceMatch::OutsideAuditedRange);
            assert_private_capabilities_disabled(&profile);
        }
    }

    #[test]
    fn same_patch_stable_release_is_outside_exact_audit() {
        let profile = profile("0.1.220");
        assert_eq!(profile.source_match(), GrokSourceMatch::OutsideAuditedRange);
        assert_private_capabilities_disabled(&profile);
    }

    #[test]
    fn older_next_patch_and_next_minor_are_outside_exact_audit() {
        for version in [
            "0.1.220-alpha.3",
            "0.1.219",
            "0.1.221-alpha.1",
            "0.1.221",
            "0.2.0",
        ] {
            let profile = profile(version);
            assert_eq!(profile.source_match(), GrokSourceMatch::OutsideAuditedRange);
            assert_private_capabilities_disabled(&profile);
        }
    }

    #[test]
    fn build_metadata_is_outside_the_exact_audited_label() {
        let profile = profile("0.2.101+unverified");
        assert_eq!(profile.source_match(), GrokSourceMatch::OutsideAuditedRange);
        assert_private_capabilities_disabled(&profile);
    }

    #[test]
    fn malformed_non_string_and_oversized_versions_fail_closed() {
        for initialize in [
            json!({"_meta":{"grokShell":true,"agentVersion":"source-fixture"}}),
            json!({"_meta":{"grokShell":true,"agentVersion":220}}),
            json!({"_meta":{"grokShell":true,"agentVersion":"x".repeat(129)}}),
        ] {
            let profile = source_profile_from_initialize(&initialize);
            assert_eq!(
                profile.source_match(),
                GrokSourceMatch::MalformedAgentVersion
            );
            assert_private_capabilities_disabled(&profile);
        }
    }

    #[test]
    fn missing_or_empty_version_fails_closed() {
        for initialize in [
            json!({"_meta":{"grokShell":true}}),
            json!({"_meta":{"grokShell":true,"agentVersion":""}}),
        ] {
            let profile = source_profile_from_initialize(&initialize);
            assert_eq!(profile.source_match(), GrokSourceMatch::MissingAgentVersion);
            assert_private_capabilities_disabled(&profile);
        }

        let whitespace = source_profile_from_initialize(
            &json!({"_meta":{"grokShell":true,"agentVersion":"  "}}),
        );
        assert_eq!(
            whitespace.source_match(),
            GrokSourceMatch::MalformedAgentVersion
        );
        assert_private_capabilities_disabled(&whitespace);
    }

    #[test]
    fn non_grok_shell_never_enables_private_capabilities() {
        for initialize in [
            json!({"_meta":{"grokShell":false,"agentVersion":GROK_BUILD_SOURCE_VERSION}}),
            json!({"_meta":{"agentVersion":GROK_BUILD_SOURCE_VERSION}}),
            json!({"grokShell":true,"agentVersion":GROK_BUILD_SOURCE_VERSION}),
            json!({"_meta":{"grokShell":"true","agentVersion":GROK_BUILD_SOURCE_VERSION}}),
        ] {
            let profile = source_profile_from_initialize(&initialize);
            assert_eq!(profile.source_match(), GrokSourceMatch::NotGrokShell);
            assert_private_capabilities_disabled(&profile);
        }
    }
}
