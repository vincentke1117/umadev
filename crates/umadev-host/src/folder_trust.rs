//! Pure Grok Build Folder Trust wire contracts.
//!
//! Folder Trust is a vendor extension, not an ACP-wide capability. These types
//! deliberately do no filesystem I/O and grant no trust themselves: the ACP
//! driver must bind every reverse request to an authoritative live session
//! scope, and the interactive UI must make the final explicit decision.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

/// Exact Grok Build reverse-request method for Folder Trust.
pub const GROK_FOLDER_TRUST_REQUEST_METHOD: &str = "x.ai/folder_trust/request";

/// Source-defined upper bound for one interactive trust prompt.
pub const GROK_FOLDER_TRUST_TIMEOUT: Duration = Duration::from_secs(30 * 60);

const MAX_SESSION_ID_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 32_768;
const MAX_CONFIG_KINDS: usize = 32;
const MAX_CONFIG_KIND_BYTES: usize = 64;

/// Whether the client actually has a live Folder Trust decision surface.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FolderTrustClientSurface {
    /// No live human decision surface exists (CI, one-shot, daemon, or other
    /// headless session-opening path).
    Headless,
    /// A live interactive UI is wired to receive, display, and settle the
    /// reverse request.
    Interactive,
}

/// Build the Grok-specific part of `clientCapabilities._meta`.
///
/// Headless callers never advertise the capability, even when the pinned source
/// contract is active. Interactive callers advertise only after both the source
/// contract and the live UI bridge are available.
#[must_use]
pub fn folder_trust_client_capabilities_meta(
    source_contract_active: bool,
    surface: FolderTrustClientSurface,
) -> Map<String, Value> {
    let mut meta = Map::new();
    if source_contract_active && matches!(surface, FolderTrustClientSurface::Interactive) {
        meta.insert(
            "x.ai/folderTrust".to_string(),
            serde_json::json!({ "interactive": true }),
        );
    }
    meta
}

/// Validation failure for an authoritative Folder Trust scope or reverse
/// request. Error text never repeats an untrusted wire value.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FolderTrustContractError {
    /// The request is not a JSON object.
    InvalidRequest,
    /// The session id is missing, empty, overlong, or contains controls.
    InvalidSessionId,
    /// The request belongs to a different live session.
    ForeignSession,
    /// The cwd is missing or is not a bounded unambiguous absolute path.
    InvalidCwd,
    /// The cwd does not exactly match the authoritative session cwd.
    ForeignCwd,
    /// The display workspace is missing or is not a bounded unambiguous absolute path.
    InvalidWorkspace,
    /// The request contains too many configuration-kind display labels.
    TooManyConfigKinds,
    /// A configuration-kind label is empty, overlong, duplicated, or contains
    /// terminal controls.
    InvalidConfigKind,
}

impl fmt::Display for FolderTrustContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRequest => "invalid Folder Trust request",
            Self::InvalidSessionId => "invalid Folder Trust session id",
            Self::ForeignSession => "Folder Trust request belongs to a foreign session",
            Self::InvalidCwd => "invalid Folder Trust cwd",
            Self::ForeignCwd => "Folder Trust request belongs to a foreign cwd",
            Self::InvalidWorkspace => "invalid Folder Trust workspace",
            Self::TooManyConfigKinds => "Folder Trust request has too many config kinds",
            Self::InvalidConfigKind => "Folder Trust request has an invalid config kind",
        };
        f.write_str(message)
    }
}

impl Error for FolderTrustContractError {}

/// Authoritative source of ownership for one Folder Trust reverse request.
///
/// The Grok child is the authority for its workspace trust key. UmaDev can bind
/// a reverse request to the pinned connection, exact session id, and exact cwd,
/// but it must not independently reproduce Grok's git/worktree-aware
/// `workspace_key` algorithm and then pretend that approximation is
/// authoritative. The request's bounded workspace therefore remains display
/// data only; Grok persists and applies the actual grant.
#[derive(Clone, Eq, PartialEq)]
pub struct FolderTrustScope {
    session_id: String,
    cwd: PathBuf,
    cwd_wire: String,
}

impl FolderTrustScope {
    /// Construct an authoritative scope from the active pinned session route.
    pub fn new(
        session_id: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> Result<Self, FolderTrustContractError> {
        let session_id = session_id.into();
        validate_text(&session_id, MAX_SESSION_ID_BYTES)
            .map_err(|()| FolderTrustContractError::InvalidSessionId)?;

        let cwd = cwd.into();
        validate_path(&cwd).map_err(|()| FolderTrustContractError::InvalidCwd)?;

        Ok(Self {
            session_id,
            cwd_wire: cwd.to_string_lossy().into_owned(),
            cwd,
        })
    }

    /// Authoritative live session id.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Authoritative session cwd.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

impl fmt::Debug for FolderTrustScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FolderTrustScope([VALIDATED])")
    }
}

/// Validated Folder Trust reverse request bound to one authoritative scope.
///
/// This type intentionally has no `serde::Deserialize` implementation: callers
/// must use [`FolderTrustRequest::parse_for_scope`] so wire data can never skip
/// ownership checks.
#[derive(Clone, Eq, PartialEq)]
pub struct FolderTrustRequest {
    session_id: String,
    cwd: PathBuf,
    workspace: PathBuf,
    config_kinds: Vec<String>,
}

impl FolderTrustRequest {
    /// Parse, bound, and bind a reverse request to an authoritative live scope.
    pub fn parse_for_scope(
        params: &Value,
        scope: &FolderTrustScope,
    ) -> Result<Self, FolderTrustContractError> {
        let object = params
            .as_object()
            .ok_or(FolderTrustContractError::InvalidRequest)?;

        let session_id = object
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or(FolderTrustContractError::InvalidSessionId)?;
        validate_text(session_id, MAX_SESSION_ID_BYTES)
            .map_err(|()| FolderTrustContractError::InvalidSessionId)?;
        if session_id != scope.session_id {
            return Err(FolderTrustContractError::ForeignSession);
        }

        let cwd_wire = object
            .get("cwd")
            .and_then(Value::as_str)
            .ok_or(FolderTrustContractError::InvalidCwd)?;
        let cwd = PathBuf::from(cwd_wire);
        validate_path(&cwd).map_err(|()| FolderTrustContractError::InvalidCwd)?;
        if cwd_wire != scope.cwd_wire {
            return Err(FolderTrustContractError::ForeignCwd);
        }

        let workspace_wire = object
            .get("workspace")
            .and_then(Value::as_str)
            .ok_or(FolderTrustContractError::InvalidWorkspace)?;
        let workspace = PathBuf::from(workspace_wire);
        validate_path(&workspace).map_err(|()| FolderTrustContractError::InvalidWorkspace)?;
        let config_kinds = object
            .get("configKinds")
            .and_then(Value::as_array)
            .ok_or(FolderTrustContractError::InvalidConfigKind)?;
        if config_kinds.is_empty() {
            return Err(FolderTrustContractError::InvalidConfigKind);
        }
        if config_kinds.len() > MAX_CONFIG_KINDS {
            return Err(FolderTrustContractError::TooManyConfigKinds);
        }
        let mut seen = HashSet::with_capacity(config_kinds.len());
        let mut parsed_kinds = Vec::with_capacity(config_kinds.len());
        for kind in config_kinds {
            let kind = kind
                .as_str()
                .ok_or(FolderTrustContractError::InvalidConfigKind)?;
            validate_text(kind, MAX_CONFIG_KIND_BYTES)
                .map_err(|()| FolderTrustContractError::InvalidConfigKind)?;
            if !seen.insert(kind) {
                return Err(FolderTrustContractError::InvalidConfigKind);
            }
            parsed_kinds.push(kind.to_string());
        }

        Ok(Self {
            session_id: session_id.to_string(),
            cwd,
            workspace,
            config_kinds: parsed_kinds,
        })
    }

    /// Session owning this prompt.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Session cwd being gated.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Source-computed trust key shown to the user.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Display-only reasons the folder is gated.
    #[must_use]
    pub fn config_kinds(&self) -> &[String] {
        &self.config_kinds
    }
}

impl fmt::Debug for FolderTrustRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FolderTrustRequest")
            .field("scope", &"[VALIDATED]")
            .field("config_kind_count", &self.config_kinds.len())
            .finish_non_exhaustive()
    }
}

/// Result of evaluating a Folder Trust round trip.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FolderTrustDisposition {
    /// The exact response was `{ "outcome": "trust" }`.
    Grant,
    /// Stay gated. This covers rejection, cancellation, timeout, transport
    /// error, malformed input, and future/unknown outcome strings.
    KeepGated,
}

/// Pure representation of a completed or failed trust round trip.
#[derive(Debug, Clone, Copy)]
pub enum FolderTrustRoundTrip<'a> {
    /// A client response payload was received.
    Response(&'a Value),
    /// The source-defined human-decision deadline elapsed.
    Timeout,
    /// The client disconnected or the reverse request failed.
    TransportError,
    /// The user closed/cancelled the decision surface.
    Cancelled,
}

/// Resolve a Folder Trust result fail-closed. Only the exact string `trust`
/// grants; every unknown, malformed, timeout, cancellation, or transport path
/// stays gated.
#[must_use]
pub fn folder_trust_disposition(round_trip: FolderTrustRoundTrip<'_>) -> FolderTrustDisposition {
    let FolderTrustRoundTrip::Response(response) = round_trip else {
        return FolderTrustDisposition::KeepGated;
    };
    if response
        .as_object()
        .and_then(|object| object.get("outcome"))
        .and_then(Value::as_str)
        == Some("trust")
    {
        FolderTrustDisposition::Grant
    } else {
        FolderTrustDisposition::KeepGated
    }
}

/// Explicit local user decision to send back to Grok Build.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FolderTrustUserDecision {
    /// The live user explicitly trusted the validated scope.
    Trust,
    /// The user rejected, cancelled, or closed the prompt.
    KeepGated,
}

impl FolderTrustUserDecision {
    /// Build the source-shaped response. Only [`Self::Trust`] emits the granting
    /// wire outcome.
    #[must_use]
    pub fn to_wire_value(self) -> Value {
        match self {
            Self::Trust => serde_json::json!({ "outcome": "trust" }),
            Self::KeepGated => serde_json::json!({ "outcome": "reject" }),
        }
    }
}

fn validate_text(value: &str, max_bytes: usize) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '\u{200e}'
                        | '\u{200f}'
                        | '\u{2028}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
    {
        return Err(());
    }
    Ok(())
}

fn validate_path(path: &Path) -> Result<(), ()> {
    let wire = path.to_string_lossy();
    validate_text(&wire, MAX_PATH_BYTES)?;
    if !path.is_absolute() || path.parent().is_none() {
        return Err(());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cwd_path() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\repo\worktree\app")
        } else {
            PathBuf::from("/repo/worktree/app")
        }
    }

    fn workspace_path() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\repo\main")
        } else {
            PathBuf::from("/repo/main")
        }
    }

    fn other_path() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\repo\other")
        } else {
            PathBuf::from("/repo/other")
        }
    }

    fn scope() -> FolderTrustScope {
        FolderTrustScope::new("session-7", cwd_path()).unwrap()
    }

    fn valid_request() -> Value {
        serde_json::json!({
            "sessionId": "session-7",
            "cwd": cwd_path().to_string_lossy(),
            "workspace": workspace_path().to_string_lossy(),
            "configKinds": ["mcp", "hooks", "lsp"]
        })
    }

    #[test]
    fn headless_never_advertises_interactive_folder_trust() {
        for source_contract_active in [false, true] {
            let meta = folder_trust_client_capabilities_meta(
                source_contract_active,
                FolderTrustClientSurface::Headless,
            );
            assert!(!meta.contains_key("x.ai/folderTrust"));
        }

        assert!(folder_trust_client_capabilities_meta(
            false,
            FolderTrustClientSurface::Interactive
        )
        .is_empty());
        assert_eq!(
            folder_trust_client_capabilities_meta(true, FolderTrustClientSurface::Interactive)
                .get("x.ai/folderTrust"),
            Some(&serde_json::json!({ "interactive": true }))
        );
    }

    #[test]
    fn request_is_bound_to_exact_session_and_cwd_while_workspace_stays_display_only() {
        let request = FolderTrustRequest::parse_for_scope(&valid_request(), &scope()).unwrap();
        assert_eq!(request.session_id(), "session-7");
        assert_eq!(request.cwd(), cwd_path());
        assert_eq!(request.workspace(), workspace_path());
        assert_eq!(request.config_kinds(), ["mcp", "hooks", "lsp"]);

        // Grok can collapse a worktree onto a main-checkout trust key, so the
        // two authoritative paths need not have an ancestor relationship.
        assert!(!request.cwd().starts_with(request.workspace()));
    }

    #[test]
    fn foreign_ownership_is_rejected_before_user_presentation() {
        for (field, value, expected) in [
            (
                "sessionId",
                serde_json::json!("other-session"),
                FolderTrustContractError::ForeignSession,
            ),
            (
                "cwd",
                serde_json::json!(other_path().to_string_lossy()),
                FolderTrustContractError::ForeignCwd,
            ),
        ] {
            let mut request = valid_request();
            request[field] = value;
            assert_eq!(
                FolderTrustRequest::parse_for_scope(&request, &scope()),
                Err(expected)
            );
        }

        // Grok alone computes the git/worktree-aware trust key. A different,
        // still-safe absolute display workspace must not be rejected by an
        // UmaDev approximation; the pinned child applies the actual grant.
        let mut different_workspace = valid_request();
        different_workspace["workspace"] = serde_json::json!(other_path().to_string_lossy());
        let parsed = FolderTrustRequest::parse_for_scope(&different_workspace, &scope()).unwrap();
        assert_eq!(parsed.workspace(), other_path());
    }

    #[test]
    fn ambiguous_paths_and_unsafe_display_fields_are_rejected() {
        let mut relative = valid_request();
        relative["cwd"] = serde_json::json!("repo/app");
        assert_eq!(
            FolderTrustRequest::parse_for_scope(&relative, &scope()),
            Err(FolderTrustContractError::InvalidCwd)
        );

        let ambiguous = cwd_path().join("..").join("escape");
        assert_eq!(
            FolderTrustScope::new("session", ambiguous),
            Err(FolderTrustContractError::InvalidCwd)
        );
        assert_eq!(
            FolderTrustScope::new("session\nforged", cwd_path()),
            Err(FolderTrustContractError::InvalidSessionId)
        );

        let mut control = valid_request();
        control["configKinds"] = serde_json::json!(["mcp\nforged"]);
        assert_eq!(
            FolderTrustRequest::parse_for_scope(&control, &scope()),
            Err(FolderTrustContractError::InvalidConfigKind)
        );

        let mut bidi_spoof = valid_request();
        bidi_spoof["workspace"] = if cfg!(windows) {
            serde_json::json!(format!(r"C:\repo\safe{}exe.txt", '\u{202e}'))
        } else {
            serde_json::json!("/repo/safe\u{202e}exe.txt")
        };
        assert_eq!(
            FolderTrustRequest::parse_for_scope(&bidi_spoof, &scope()),
            Err(FolderTrustContractError::InvalidWorkspace)
        );

        let mut duplicate = valid_request();
        duplicate["configKinds"] = serde_json::json!(["mcp", "mcp"]);
        assert_eq!(
            FolderTrustRequest::parse_for_scope(&duplicate, &scope()),
            Err(FolderTrustContractError::InvalidConfigKind)
        );

        let mut empty = valid_request();
        empty["configKinds"] = serde_json::json!([]);
        assert_eq!(
            FolderTrustRequest::parse_for_scope(&empty, &scope()),
            Err(FolderTrustContractError::InvalidConfigKind)
        );

        let mut too_many = valid_request();
        too_many["configKinds"] = Value::Array(
            (0..=MAX_CONFIG_KINDS)
                .map(|index| Value::String(format!("kind-{index}")))
                .collect(),
        );
        assert_eq!(
            FolderTrustRequest::parse_for_scope(&too_many, &scope()),
            Err(FolderTrustContractError::TooManyConfigKinds)
        );

        assert_eq!(
            FolderTrustScope::new("s".repeat(MAX_SESSION_ID_BYTES + 1), cwd_path()),
            Err(FolderTrustContractError::InvalidSessionId)
        );
    }

    #[test]
    fn only_exact_trust_response_grants() {
        let trust = serde_json::json!({ "outcome": "trust" });
        assert_eq!(
            folder_trust_disposition(FolderTrustRoundTrip::Response(&trust)),
            FolderTrustDisposition::Grant
        );

        for response in [
            serde_json::json!({ "outcome": "reject" }),
            serde_json::json!({ "outcome": "TRUST" }),
            serde_json::json!({ "outcome": "future_value" }),
            serde_json::json!({ "outcome": true }),
            serde_json::json!({}),
            serde_json::json!(null),
        ] {
            assert_eq!(
                folder_trust_disposition(FolderTrustRoundTrip::Response(&response)),
                FolderTrustDisposition::KeepGated
            );
        }
        for failure in [
            FolderTrustRoundTrip::Timeout,
            FolderTrustRoundTrip::TransportError,
            FolderTrustRoundTrip::Cancelled,
        ] {
            assert_eq!(
                folder_trust_disposition(failure),
                FolderTrustDisposition::KeepGated
            );
        }
    }

    #[test]
    fn response_builder_never_invents_a_grant() {
        assert_eq!(
            FolderTrustUserDecision::Trust.to_wire_value(),
            serde_json::json!({ "outcome": "trust" })
        );
        assert_eq!(
            FolderTrustUserDecision::KeepGated.to_wire_value(),
            serde_json::json!({ "outcome": "reject" })
        );
    }

    #[test]
    fn debug_output_does_not_repeat_local_paths() {
        let scope = scope();
        let request = FolderTrustRequest::parse_for_scope(&valid_request(), &scope).unwrap();
        for debug in [format!("{scope:?}"), format!("{request:?}")] {
            assert!(!debug.contains(&workspace_path().to_string_lossy().to_string()));
            assert!(!debug.contains("session-7"));
        }
    }
}
