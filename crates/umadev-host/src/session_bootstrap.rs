//! Typed interaction primitives for opening a base session before a
//! [`umadev_runtime::BaseSession`] exists.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, Notify};
use umadev_runtime::SessionError;
use url::{Host, Url};

const MAX_AUTH_URL_BYTES: usize = 4_096;
const MAX_AUTH_CODE_BYTES: usize = 8_192;
const MAX_COMMAND_AUTH_STATUS_BYTES: usize = 8_192;
const MAX_COMMAND_AUTH_STATUS_LINES: usize = 64;
const MAX_AUTH_METHODS: usize = 16;
const MAX_AUTH_METHOD_ID_BYTES: usize = 128;
const MAX_AUTH_METHOD_LABEL_BYTES: usize = 256;
const AUTH_CONTROL_CAPACITY: usize = 2;

/// Number of source-compatible `x.ai/auth/get_url` polls made while an
/// interactive Grok authentication request is in flight.
///
/// The first poll is immediate. An early empty response is not terminal: Grok
/// installs the one-shot URL receiver from inside `authenticate`, so the pager
/// deliberately retries while that request is starting.
pub const GROK_AUTH_URL_POLL_ATTEMPTS: usize = 60;

/// Delay between source-compatible Grok authentication URL polls.
pub const GROK_AUTH_URL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Exact Grok Build authentication method backed by the credential already
/// validated/refreshed during `initialize`.
pub const GROK_CACHED_TOKEN_AUTH_METHOD_ID: &str = "cached_token";

/// Exact Grok Build non-interactive API-key authentication method.
pub const GROK_API_KEY_AUTH_METHOD_ID: &str = "xai.api_key";

/// Exact Grok Build interactive first-party login method.
pub const GROK_COM_AUTH_METHOD_ID: &str = "grok.com";

/// Exact Grok Build interactive enterprise OIDC method.
pub const GROK_OIDC_AUTH_METHOD_ID: &str = "oidc";

/// Caller-owned identity for one session-opening attempt.
///
/// The caller should mint this from its local generation and sequence. It is a
/// correlation token, not a global registry key.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionOpenId(u64);

impl SessionOpenId {
    /// Create an attempt identity from a caller-owned sequence.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the caller-owned numeric value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SessionOpenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Safe, non-secret description of one authentication method advertised by a
/// base.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuthMethodSummary {
    /// Exact method id advertised by the base.
    pub id: String,
    /// Human-readable label supplied by the base.
    pub label: String,
    /// Whether selecting this method can require live user interaction.
    pub interactive: bool,
}

impl AuthMethodSummary {
    /// Build an advertised authentication-method summary.
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>, interactive: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            interactive,
        }
    }
}

/// Authentication choices returned when a non-interactive session open cannot
/// continue safely.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuthOffer {
    /// Stable backend id that produced the offer.
    pub backend_id: String,
    /// Authentication methods advertised by that backend.
    pub methods: Vec<AuthMethodSummary>,
    /// Backend-selected default method, when it is present in `methods`.
    pub default_method_id: Option<String>,
    /// Whether explicitly starting an interactive method may open a browser.
    pub may_open_browser: bool,
}

impl AuthOffer {
    /// Build an authentication offer from one initialize response.
    #[must_use]
    pub fn new(
        backend_id: impl Into<String>,
        methods: Vec<AuthMethodSummary>,
        default_method_id: Option<String>,
        may_open_browser: bool,
    ) -> Self {
        Self {
            backend_id: backend_id.into(),
            methods,
            default_method_id,
            may_open_browser,
        }
    }

    /// Return the advertised default method when the id still belongs to this
    /// offer.
    #[must_use]
    pub fn default_method(&self) -> Option<&AuthMethodSummary> {
        let default = self.default_method_id.as_deref()?;
        self.methods.iter().find(|method| method.id == default)
    }
}

/// Source-audited classification of a Grok Build authentication method.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GrokAuthMethodKind {
    /// API-key credentials already configured in Grok Build.
    ApiKey,
    /// A current session credential validated/refreshed during `initialize`.
    CachedToken,
    /// Interactive first-party Grok login.
    GrokCom,
    /// Interactive enterprise OIDC login.
    Oidc,
    /// A future or vendor-private method UmaDev has not audited.
    Unknown,
}

impl GrokAuthMethodKind {
    /// Classify an exact, case-sensitive method id from Grok Build.
    #[must_use]
    pub fn from_method_id(method_id: &str) -> Self {
        match method_id {
            GROK_API_KEY_AUTH_METHOD_ID => Self::ApiKey,
            GROK_CACHED_TOKEN_AUTH_METHOD_ID => Self::CachedToken,
            GROK_COM_AUTH_METHOD_ID => Self::GrokCom,
            GROK_OIDC_AUTH_METHOD_ID => Self::Oidc,
            _ => Self::Unknown,
        }
    }

    /// Whether this method is known to complete without live user interaction.
    #[must_use]
    pub const fn is_non_interactive(self) -> bool {
        matches!(self, Self::ApiKey | Self::CachedToken)
    }

    /// Whether this exact method is known to require live user interaction.
    #[must_use]
    pub const fn is_interactive(self) -> bool {
        matches!(self, Self::GrokCom | Self::Oidc)
    }
}

/// One validated authentication method from a Grok Build `initialize`
/// response.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GrokAuthMethod {
    summary: AuthMethodSummary,
    kind: GrokAuthMethodKind,
    external_provider_command: bool,
}

impl GrokAuthMethod {
    /// Safe description suitable for an authentication picker.
    #[must_use]
    pub const fn summary(&self) -> &AuthMethodSummary {
        &self.summary
    }

    /// Source-audited method classification.
    #[must_use]
    pub const fn kind(&self) -> GrokAuthMethodKind {
        self.kind
    }

    /// Whether Grok's configured external command owns the browser/login UI.
    #[must_use]
    pub const fn external_provider_command(&self) -> bool {
        self.external_provider_command
    }
}

/// Validation failure while parsing Grok Build's authentication advertisement.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GrokAuthContractError {
    /// `authMethods` was absent or was not an array.
    MissingMethods,
    /// More methods were advertised than the bounded bootstrap accepts.
    TooManyMethods,
    /// One advertised method was not an object.
    InvalidMethod,
    /// A method id was absent, empty, overlong, or contained a control character.
    InvalidMethodId,
    /// Two advertised methods used the same exact id.
    DuplicateMethodId,
    /// A display label was overlong or contained a control character.
    InvalidMethodLabel,
    /// Authentication method metadata had an unexpected type or placement.
    InvalidMethodMetadata,
    /// `defaultAuthMethodId` was present but not a safe string.
    InvalidDefaultMethodId,
    /// An explicitly selected method is no longer advertised.
    MethodNotAdvertised,
    /// An explicitly selected method is not an audited interactive method.
    MethodNotInteractive,
}

impl fmt::Display for GrokAuthContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingMethods => "Grok authentication methods are missing",
            Self::TooManyMethods => "Grok advertised too many authentication methods",
            Self::InvalidMethod => "Grok advertised an invalid authentication method",
            Self::InvalidMethodId => "Grok advertised an invalid authentication method id",
            Self::DuplicateMethodId => "Grok advertised a duplicate authentication method id",
            Self::InvalidMethodLabel => "Grok advertised an invalid authentication method label",
            Self::InvalidMethodMetadata => "Grok advertised invalid authentication method metadata",
            Self::InvalidDefaultMethodId => {
                "Grok advertised an invalid default authentication method id"
            }
            Self::MethodNotAdvertised => "the selected authentication method is not advertised",
            Self::MethodNotInteractive => "the selected authentication method is not interactive",
        };
        f.write_str(message)
    }
}

impl Error for GrokAuthContractError {}

/// Validated authentication catalog from one Grok Build `initialize` response.
///
/// An unadvertised `defaultAuthMethodId` is ignored, matching Grok's own pager:
/// the agent remains the source of truth only while the id still belongs to the
/// accompanying `authMethods` list.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GrokAuthCatalog {
    methods: Vec<GrokAuthMethod>,
    default_method_id: Option<String>,
}

impl GrokAuthCatalog {
    /// Parse and bound the source-shaped authentication portion of an
    /// `initialize` result.
    pub fn parse_initialize(
        initialize_result: &serde_json::Value,
    ) -> Result<Self, GrokAuthContractError> {
        let methods = initialize_result
            .get("authMethods")
            .or_else(|| initialize_result.get("auth_methods"))
            .and_then(serde_json::Value::as_array)
            .ok_or(GrokAuthContractError::MissingMethods)?;
        if methods.len() > MAX_AUTH_METHODS {
            return Err(GrokAuthContractError::TooManyMethods);
        }

        let mut seen = HashSet::with_capacity(methods.len());
        let mut parsed = Vec::with_capacity(methods.len());
        for method in methods {
            let object = method
                .as_object()
                .ok_or(GrokAuthContractError::InvalidMethod)?;
            let id = object
                .get("id")
                .or_else(|| object.get("methodId"))
                .and_then(serde_json::Value::as_str)
                .ok_or(GrokAuthContractError::InvalidMethodId)?;
            validate_bounded_text(id, MAX_AUTH_METHOD_ID_BYTES, false)
                .map_err(|()| GrokAuthContractError::InvalidMethodId)?;
            if !seen.insert(id.to_string()) {
                return Err(GrokAuthContractError::DuplicateMethodId);
            }

            let label = match object.get("name").or_else(|| object.get("label")) {
                None => id,
                Some(value) => value
                    .as_str()
                    .ok_or(GrokAuthContractError::InvalidMethodLabel)?,
            };
            validate_bounded_text(label, MAX_AUTH_METHOD_LABEL_BYTES, false)
                .map_err(|()| GrokAuthContractError::InvalidMethodLabel)?;

            let kind = GrokAuthMethodKind::from_method_id(id);
            let external_provider_command = match object.get("_meta").or_else(|| object.get("meta"))
            {
                None | Some(serde_json::Value::Null) => false,
                Some(value) => {
                    let meta = value
                        .as_object()
                        .ok_or(GrokAuthContractError::InvalidMethodMetadata)?;
                    match meta.get("external_provider") {
                        None => false,
                        Some(value) => value
                            .as_bool()
                            .ok_or(GrokAuthContractError::InvalidMethodMetadata)?,
                    }
                }
            };
            if external_provider_command && kind != GrokAuthMethodKind::GrokCom {
                return Err(GrokAuthContractError::InvalidMethodMetadata);
            }
            parsed.push(GrokAuthMethod {
                summary: AuthMethodSummary::new(id, label, kind.is_interactive()),
                kind,
                external_provider_command,
            });
        }

        let default_method_id = match initialize_result
            .pointer("/_meta/defaultAuthMethodId")
            .or_else(|| initialize_result.pointer("/_meta/default_auth_method_id"))
        {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => {
                let id = value
                    .as_str()
                    .ok_or(GrokAuthContractError::InvalidDefaultMethodId)?;
                validate_bounded_text(id, MAX_AUTH_METHOD_ID_BYTES, false)
                    .map_err(|()| GrokAuthContractError::InvalidDefaultMethodId)?;
                parsed
                    .iter()
                    .any(|method| method.summary.id == id)
                    .then(|| id.to_string())
            }
        };

        Ok(Self {
            methods: parsed,
            default_method_id,
        })
    }

    /// Validated methods in the exact order Grok advertised them.
    #[must_use]
    pub fn methods(&self) -> &[GrokAuthMethod] {
        &self.methods
    }

    /// Advertised default method, only when it is still present in this catalog.
    #[must_use]
    pub fn default_method(&self) -> Option<&GrokAuthMethod> {
        let id = self.default_method_id.as_deref()?;
        self.method(id)
    }

    /// Find an exact advertised method.
    #[must_use]
    pub fn method(&self, method_id: &str) -> Option<&GrokAuthMethod> {
        self.methods
            .iter()
            .find(|method| method.summary.id == method_id)
    }

    /// Build the safe user-facing offer for this initialize generation.
    #[must_use]
    pub fn offer(&self) -> AuthOffer {
        AuthOffer::new(
            "grok-build",
            self.methods
                .iter()
                .map(|method| method.summary.clone())
                .collect(),
            self.default_method_id.clone(),
            self.methods
                .iter()
                .any(|method| method.kind.is_interactive()),
        )
    }

    /// Decide the only safe automatic action after `initialize`.
    ///
    /// Grok refreshes/validates `cached_token` before advertising it. Selecting
    /// that default therefore means the session is already authenticated and
    /// UmaDev must not send a second `authenticate` request. Interactive and
    /// unknown methods are never started without explicit user authorization.
    #[must_use]
    pub fn bootstrap_decision(&self) -> GrokAuthBootstrapDecision {
        let selected = self.default_method().or_else(|| {
            self.methods
                .iter()
                .find(|method| method.kind == GrokAuthMethodKind::CachedToken)
        });
        let selected = selected.or_else(|| self.methods.first());

        match selected.map(GrokAuthMethod::kind) {
            Some(GrokAuthMethodKind::CachedToken) => {
                GrokAuthBootstrapDecision::UseInitializedCachedToken
            }
            Some(GrokAuthMethodKind::ApiKey) => {
                GrokAuthBootstrapDecision::AuthenticateNonInteractive {
                    method_id: GROK_API_KEY_AUTH_METHOD_ID.to_string(),
                }
            }
            Some(
                GrokAuthMethodKind::GrokCom
                | GrokAuthMethodKind::Oidc
                | GrokAuthMethodKind::Unknown,
            )
            | None => GrokAuthBootstrapDecision::UserActionRequired(self.offer()),
        }
    }

    /// Revalidate an explicitly selected method against the newest initialize
    /// response before an interactive authentication attempt starts.
    pub fn interactive_selection(
        &self,
        method_id: &str,
    ) -> Result<GrokInteractiveAuthSelection, GrokAuthContractError> {
        validate_bounded_text(method_id, MAX_AUTH_METHOD_ID_BYTES, false)
            .map_err(|()| GrokAuthContractError::InvalidMethodId)?;
        let method = self
            .method(method_id)
            .ok_or(GrokAuthContractError::MethodNotAdvertised)?;
        if !method.kind.is_interactive() {
            return Err(GrokAuthContractError::MethodNotInteractive);
        }
        Ok(GrokInteractiveAuthSelection {
            method_id: method.summary.id.clone(),
            external_provider_command: method.external_provider_command,
        })
    }
}

/// Safe automatic action selected from a validated Grok auth catalog.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum GrokAuthBootstrapDecision {
    /// Continue directly: initialize already validated/refreshed the cached
    /// session credential.
    UseInitializedCachedToken,
    /// Send `authenticate` with `headless: true` for this exact method.
    AuthenticateNonInteractive {
        /// Exact advertised method id.
        method_id: String,
    },
    /// Stop before any browser-capable action and ask the user what to do.
    UserActionRequired(AuthOffer),
}

impl GrokAuthBootstrapDecision {
    /// Build non-secret `authenticate` params only for the audited
    /// non-interactive decision.
    #[must_use]
    pub fn non_interactive_authenticate_params(&self) -> Option<serde_json::Value> {
        match self {
            Self::AuthenticateNonInteractive { method_id } => Some(serde_json::json!({
                "methodId": method_id,
                "_meta": { "headless": true }
            })),
            Self::UseInitializedCachedToken | Self::UserActionRequired(_) => None,
        }
    }
}

/// Revalidated, explicitly user-selected interactive method.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GrokInteractiveAuthSelection {
    method_id: String,
    external_provider_command: bool,
}

impl GrokInteractiveAuthSelection {
    /// Exact method id to send to `authenticate`.
    #[must_use]
    pub fn method_id(&self) -> &str {
        &self.method_id
    }

    /// Whether the initialize advertisement says a configured command owns the
    /// external login UI. The later `get_url.mode` remains authoritative.
    #[must_use]
    pub const fn external_provider_command(&self) -> bool {
        self.external_provider_command
    }

    /// Build the non-secret interactive `authenticate` request params.
    ///
    /// Absence of `headless: true` is intentional. Calling this builder does not
    /// itself grant authorization; callers must first hold a matching
    /// [`SessionOpenPolicy::UserAuthorized`] generation.
    #[must_use]
    pub fn authenticate_params(&self) -> serde_json::Value {
        serde_json::json!({ "methodId": self.method_id })
    }
}

fn validate_bounded_text(value: &str, max_bytes: usize, allow_empty: bool) -> Result<(), ()> {
    if (!allow_empty && value.is_empty())
        || value.len() > max_bytes
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return Err(());
    }
    Ok(())
}

/// Presentation mode of an interactive authentication URL.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthMode {
    /// Browser callback flow with optional manual code submission.
    Loopback,
    /// An external authentication command owns its interaction.
    Command,
    /// RFC 8628 device flow; the user confirms a device code in a browser.
    Device,
}

impl AuthMode {
    /// Parse the exact wire value used by the base.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "loopback" => Some(Self::Loopback),
            "command" => Some(Self::Command),
            "device" => Some(Self::Device),
            _ => None,
        }
    }

    /// Return the exact wire value used by the base.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Command => "command",
            Self::Device => "device",
        }
    }

    /// Source-audited relationship between Grok's browser launch and URL
    /// delivery for this mode.
    #[must_use]
    pub const fn browser_url_order(self) -> AuthBrowserUrlOrder {
        match self {
            Self::Loopback => AuthBrowserUrlOrder::BrowserBeforeUrl,
            Self::Command => AuthBrowserUrlOrder::ExternalCommandOwnsBrowser,
            Self::Device => AuthBrowserUrlOrder::UrlBeforeBrowser,
        }
    }

    /// Whether UmaDev may show a manual callback/code submission field.
    #[must_use]
    pub const fn accepts_manual_code(self) -> bool {
        matches!(self, Self::Loopback)
    }
}

/// Browser/URL ordering implemented by the pinned Grok Build source.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthBrowserUrlOrder {
    /// Loopback OIDC opens the browser before publishing the URL to the client.
    BrowserBeforeUrl,
    /// Device login publishes its URL/code before a detached browser launch.
    UrlBeforeBrowser,
    /// A configured external provider command owns browser interaction.
    ExternalCommandOwnsBrowser,
}

/// Validation failure for `x.ai/auth/get_url`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthUrlPollError {
    /// The response is not an object.
    InvalidResponse,
    /// The URL and mode fields describe neither pending nor a complete challenge.
    IncompleteChallenge,
    /// The response used an unknown authentication mode.
    UnknownMode,
    /// The compatibility `external_provider` flag contradicted the authoritative mode.
    InconsistentExternalProvider,
    /// The advertised URL failed safe URL validation.
    UnsafeUrl(SafeAuthUrlError),
    /// An external authentication command returned an unusable status.
    UnsafeCommandStatus(SafeCommandAuthStatusError),
}

impl fmt::Display for AuthUrlPollError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidResponse => f.write_str("invalid authentication URL response"),
            Self::IncompleteChallenge => f.write_str("incomplete authentication URL challenge"),
            Self::UnknownMode => f.write_str("unknown authentication URL mode"),
            Self::InconsistentExternalProvider => {
                f.write_str("authentication URL mode contradicts its compatibility flag")
            }
            Self::UnsafeUrl(error) => error.fmt(f),
            Self::UnsafeCommandStatus(error) => error.fmt(f),
        }
    }
}

impl Error for AuthUrlPollError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnsafeUrl(error) => Some(error),
            Self::UnsafeCommandStatus(error) => Some(error),
            Self::InvalidResponse
            | Self::IncompleteChallenge
            | Self::UnknownMode
            | Self::InconsistentExternalProvider => None,
        }
    }
}

/// Validated source-shaped authentication URL response.
#[derive(Debug)]
pub enum AuthUrlPoll {
    /// `authenticate` has not installed/published the one-shot URL yet; retry
    /// within the bounded source-compatible poll window.
    Pending,
    /// A complete challenge is ready for explicit UI presentation.
    Ready(AuthUrlDetails),
}

/// Validated, mode-specific details from `x.ai/auth/get_url`.
///
/// Command authentication deliberately has no URL-bearing variant. Its
/// source field is an external provider's stderr bridge and may contain
/// multi-line status text rather than a URL.
#[derive(Debug)]
pub enum AuthUrlDetails {
    /// Browser callback flow with a validated URL.
    Loopback {
        /// Validated URL with origin-only ordinary formatting.
        url: SafeAuthUrl,
    },
    /// RFC 8628 device flow with a validated URL and optional user code.
    Device {
        /// Validated URL with origin-only ordinary formatting.
        url: SafeAuthUrl,
        /// Device code, when Grok embedded it in the URL.
        user_code: Option<SensitiveText>,
    },
    /// External provider command with terminal-safe, redacted status text.
    Command {
        /// Ephemeral display text. It contains no actionable URL.
        status: SafeCommandAuthStatus,
    },
}

impl AuthUrlDetails {
    /// Return the authoritative source mode.
    #[must_use]
    pub const fn mode(&self) -> AuthMode {
        match self {
            Self::Loopback { .. } => AuthMode::Loopback,
            Self::Command { .. } => AuthMode::Command,
            Self::Device { .. } => AuthMode::Device,
        }
    }

    /// Return the source-audited browser/URL ordering for this challenge.
    #[must_use]
    pub const fn browser_url_order(&self) -> AuthBrowserUrlOrder {
        self.mode().browser_url_order()
    }

    /// Return a validated URL only for loopback and device flows.
    ///
    /// Command-mode status can never cross the open/copy boundary through
    /// this API.
    #[must_use]
    pub const fn safe_url(&self) -> Option<&SafeAuthUrl> {
        match self {
            Self::Loopback { url } | Self::Device { url, .. } => Some(url),
            Self::Command { .. } => None,
        }
    }

    /// Return the device user code when the validated device URL carried one.
    #[must_use]
    pub const fn user_code(&self) -> Option<&SensitiveText> {
        match self {
            Self::Device {
                user_code: Some(code),
                ..
            } => Some(code),
            Self::Loopback { .. }
            | Self::Device {
                user_code: None, ..
            }
            | Self::Command { .. } => None,
        }
    }

    /// Return ephemeral external-command status only for command mode.
    #[must_use]
    pub const fn command_status(&self) -> Option<&SafeCommandAuthStatus> {
        match self {
            Self::Command { status } => Some(status),
            Self::Loopback { .. } | Self::Device { .. } => None,
        }
    }

    /// Whether an explicit UI action may open or copy a validated URL.
    #[must_use]
    pub const fn can_open_or_copy_url(&self) -> bool {
        matches!(self, Self::Loopback { .. } | Self::Device { .. })
    }

    /// Whether the challenge accepts a manually pasted callback/code.
    #[must_use]
    pub const fn accepts_manual_code(&self) -> bool {
        matches!(self, Self::Loopback { .. })
    }
}

/// Parse one Grok `x.ai/auth/get_url` result without logging or persisting its
/// URL. `auth_url: null, mode: null` is a retryable pending response.
pub fn parse_grok_auth_url_poll(
    response: &serde_json::Value,
) -> Result<AuthUrlPoll, AuthUrlPollError> {
    let object = response
        .as_object()
        .ok_or(AuthUrlPollError::InvalidResponse)?;
    let raw_url = match object.get("auth_url") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or(AuthUrlPollError::InvalidResponse)?,
    };
    let raw_mode = match object.get("mode") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or(AuthUrlPollError::InvalidResponse)?,
    };
    let external_provider = match object.get("external_provider") {
        None => None,
        Some(value) => Some(value.as_bool().ok_or(AuthUrlPollError::InvalidResponse)?),
    };

    let (raw_url, raw_mode) = match (raw_url, raw_mode) {
        (None, None) if external_provider == Some(true) => {
            return Err(AuthUrlPollError::InconsistentExternalProvider);
        }
        (None, None) => return Ok(AuthUrlPoll::Pending),
        (Some(url), Some(mode)) => (url, mode),
        (None, Some(_)) | (Some(_), None) => {
            return Err(AuthUrlPollError::IncompleteChallenge);
        }
    };
    let mode = AuthMode::from_wire(raw_mode).ok_or(AuthUrlPollError::UnknownMode)?;
    if external_provider.is_some_and(|external| external != matches!(mode, AuthMode::Command)) {
        return Err(AuthUrlPollError::InconsistentExternalProvider);
    }
    let details = match mode {
        AuthMode::Loopback => AuthUrlDetails::Loopback {
            url: SafeAuthUrl::parse(raw_url).map_err(AuthUrlPollError::UnsafeUrl)?,
        },
        AuthMode::Device => {
            let url = SafeAuthUrl::parse(raw_url).map_err(AuthUrlPollError::UnsafeUrl)?;
            let user_code = url.device_user_code();
            AuthUrlDetails::Device { url, user_code }
        }
        AuthMode::Command => AuthUrlDetails::Command {
            status: SafeCommandAuthStatus::from_untrusted(raw_url)
                .map_err(AuthUrlPollError::UnsafeCommandStatus)?,
        },
    };
    Ok(AuthUrlPoll::Ready(details))
}

/// Validation failure for ephemeral external-command authentication status.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SafeCommandAuthStatusError {
    /// The raw or sanitized status is empty.
    Empty,
    /// The raw or sanitized status exceeds its byte budget.
    TooLong,
    /// The status exceeds its line budget.
    TooManyLines,
    /// The sanitized result is not safe for plain terminal rendering.
    UnsafeRendering,
}

impl fmt::Display for SafeCommandAuthStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "external authentication command returned no displayable status",
            Self::TooLong => "external authentication command status exceeds 8192 bytes",
            Self::TooManyLines => "external authentication command status exceeds 64 lines",
            Self::UnsafeRendering => {
                "external authentication command status is unsafe for terminal rendering"
            }
        };
        f.write_str(message)
    }
}

impl Error for SafeCommandAuthStatusError {}

/// Bounded, redacted status from an external authentication command.
///
/// Grok Build transports this value through the field named `auth_url`, but
/// the official source fills it from an external provider's stderr and may
/// return multiple lines of non-URL text. This wrapper strips terminal control
/// sequences, removes bidirectional formatting controls, redacts secret-shaped
/// text, and hides URL-shaped tokens before retaining it.
///
/// The type intentionally implements neither cloning, display, nor
/// serialization. Call [`Self::reveal_for_render`] only at an ephemeral plain
/// text rendering boundary; never persist or pass the result to an opener or
/// clipboard action.
pub struct SafeCommandAuthStatus {
    display: String,
    line_count: usize,
}

impl SafeCommandAuthStatus {
    /// Sanitize untrusted external-command stderr for bounded plain-text
    /// terminal rendering.
    ///
    /// # Errors
    ///
    /// Returns [`SafeCommandAuthStatusError`] when the input is empty, exceeds
    /// the byte/line budgets, or cannot produce safe display text.
    pub fn from_untrusted(value: impl Into<String>) -> Result<Self, SafeCommandAuthStatusError> {
        let raw = value.into();
        if raw.is_empty() {
            return Err(SafeCommandAuthStatusError::Empty);
        }
        if raw.len() > MAX_COMMAND_AUTH_STATUS_BYTES {
            return Err(SafeCommandAuthStatusError::TooLong);
        }
        if line_count(&raw) > MAX_COMMAND_AUTH_STATUS_LINES {
            return Err(SafeCommandAuthStatusError::TooManyLines);
        }

        let terminal_safe = strip_terminal_controls(&raw);
        let redacted = crate::redaction::redact_text(&terminal_safe);
        let display = hide_authentication_urls(&redacted)
            .trim_matches(char::is_whitespace)
            .to_owned();
        if display.is_empty() {
            return Err(SafeCommandAuthStatusError::Empty);
        }
        if display.len() > MAX_COMMAND_AUTH_STATUS_BYTES {
            return Err(SafeCommandAuthStatusError::TooLong);
        }
        let line_count = line_count(&display);
        if line_count > MAX_COMMAND_AUTH_STATUS_LINES {
            return Err(SafeCommandAuthStatusError::TooManyLines);
        }
        if display.chars().any(is_unsafe_render_character) {
            return Err(SafeCommandAuthStatusError::UnsafeRendering);
        }

        Ok(Self {
            display,
            line_count,
        })
    }

    /// Reveal sanitized text only for immediate plain-text rendering.
    #[must_use]
    pub fn reveal_for_render(&self) -> &str {
        &self.display
    }

    /// Return the sanitized UTF-8 byte length without revealing its content.
    #[must_use]
    pub fn len(&self) -> usize {
        self.display.len()
    }

    /// Return whether the sanitized status is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.display.is_empty()
    }

    /// Return the number of sanitized display lines.
    #[must_use]
    pub const fn line_count(&self) -> usize {
        self.line_count
    }
}

impl fmt::Debug for SafeCommandAuthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SafeCommandAuthStatus")
            .field("bytes", &self.display.len())
            .field("lines", &self.line_count)
            .field("content", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy)]
enum TerminalSanitizeState {
    Text,
    Escape,
    EscapeIntermediate,
    ControlSequence,
    OperatingSystemCommand { after_escape: bool },
    ControlString { after_escape: bool },
}

fn strip_terminal_controls(value: &str) -> String {
    use TerminalSanitizeState::{
        ControlSequence, ControlString, Escape, EscapeIntermediate, OperatingSystemCommand, Text,
    };

    let mut output = String::with_capacity(value.len());
    let mut state = Text;
    for character in value.chars() {
        state = match state {
            Text => match character {
                '\u{1b}' => Escape,
                '\u{009b}' => ControlSequence,
                '\u{009d}' => OperatingSystemCommand {
                    after_escape: false,
                },
                '\u{0090}' | '\u{0098}' | '\u{009e}' | '\u{009f}' => ControlString {
                    after_escape: false,
                },
                '\n' | '\u{2028}' | '\u{2029}' => {
                    output.push('\n');
                    Text
                }
                '\t' => {
                    output.push(' ');
                    Text
                }
                character if is_terminal_format_control(character) || character.is_control() => {
                    Text
                }
                character => {
                    output.push(character);
                    Text
                }
            },
            Escape => match character {
                '[' => ControlSequence,
                ']' => OperatingSystemCommand {
                    after_escape: false,
                },
                'P' | 'X' | '^' | '_' => ControlString {
                    after_escape: false,
                },
                '\u{1b}' => Escape,
                '\u{20}'..='\u{2f}' => EscapeIntermediate,
                _ => Text,
            },
            EscapeIntermediate => match character {
                '\u{1b}' => Escape,
                '\u{20}'..='\u{2f}' => EscapeIntermediate,
                _ => Text,
            },
            ControlSequence => match character {
                '\u{1b}' => Escape,
                '\u{40}'..='\u{7e}' => Text,
                _ => ControlSequence,
            },
            OperatingSystemCommand { after_escape } => {
                consume_control_string(character, after_escape, true)
            }
            ControlString { after_escape } => {
                consume_control_string(character, after_escape, false)
            }
        };
    }
    output
}

fn consume_control_string(
    character: char,
    after_escape: bool,
    operating_system_command: bool,
) -> TerminalSanitizeState {
    use TerminalSanitizeState::{ControlString, OperatingSystemCommand, Text};

    if character == '\u{009c}'
        || (operating_system_command && character == '\u{0007}')
        || (after_escape && character == '\\')
    {
        return Text;
    }
    let after_escape = character == '\u{1b}';
    if operating_system_command {
        OperatingSystemCommand { after_escape }
    } else {
        ControlString { after_escape }
    }
}

fn is_terminal_format_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}'
    )
}

fn is_unsafe_render_character(character: char) -> bool {
    (character != '\n' && character.is_control()) || is_terminal_format_control(character)
}

fn line_count(value: &str) -> usize {
    value.bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn hide_authentication_urls(value: &str) -> String {
    const HIDDEN_URL: &str = "[authentication URL hidden]";

    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_start) = find_http_scheme(&value[cursor..]) {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        output.push_str(HIDDEN_URL);
        let end = value[start..]
            .char_indices()
            .find_map(|(offset, character)| {
                (offset > 0 && character.is_whitespace()).then_some(start + offset)
            })
            .unwrap_or(value.len());
        cursor = end;
    }
    output.push_str(&value[cursor..]);
    output
}

fn find_http_scheme(value: &str) -> Option<usize> {
    value.char_indices().find_map(|(offset, _)| {
        let remaining = &value[offset..];
        (remaining
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
            || remaining
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://")))
        .then_some(offset)
    })
}

/// Validation failure for an authentication URL.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SafeAuthUrlError {
    /// The URL exceeds the bounded authentication-URL size.
    TooLong,
    /// The URL contains a control character, including a newline.
    ControlCharacter,
    /// The URL is not syntactically valid.
    Invalid,
    /// The URL does not contain a host.
    MissingHost,
    /// The URL embeds username or password information.
    UserInfo,
    /// The URL uses a scheme other than HTTP or HTTPS.
    UnsupportedScheme,
    /// Plain HTTP targets a host other than an explicit loopback host.
    InsecureHost,
}

impl fmt::Display for SafeAuthUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::TooLong => "authentication URL exceeds 4096 bytes",
            Self::ControlCharacter => "authentication URL contains a control character",
            Self::Invalid => "authentication URL is invalid",
            Self::MissingHost => "authentication URL has no host",
            Self::UserInfo => "authentication URL must not contain user information",
            Self::UnsupportedScheme => "authentication URL must use HTTP or HTTPS",
            Self::InsecureHost => "plain HTTP authentication URL must target loopback",
        };
        f.write_str(message)
    }
}

impl Error for SafeAuthUrlError {}

/// Validated authentication URL whose ordinary formatting exposes only its
/// origin. Paths as well as query and fragment data may contain OAuth state,
/// tenant ids, or one-time codes.
///
/// Query parameters may carry OAuth state or device codes. Use [`Self::reveal`]
/// only at the explicit user display/copy boundary.
#[derive(Clone, Eq, PartialEq)]
pub struct SafeAuthUrl {
    raw: String,
    parsed: Url,
}

impl SafeAuthUrl {
    /// Validate and retain an authentication URL.
    pub fn parse(value: impl Into<String>) -> Result<Self, SafeAuthUrlError> {
        let raw = value.into();
        if raw.len() > MAX_AUTH_URL_BYTES {
            return Err(SafeAuthUrlError::TooLong);
        }
        if raw.chars().any(char::is_control) {
            return Err(SafeAuthUrlError::ControlCharacter);
        }

        let parsed = Url::parse(&raw).map_err(|_| SafeAuthUrlError::Invalid)?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(SafeAuthUrlError::UnsupportedScheme);
        }
        if parsed.host().is_none() {
            return Err(SafeAuthUrlError::MissingHost);
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(SafeAuthUrlError::UserInfo);
        }

        match parsed.scheme() {
            "https" => {}
            "http" if is_explicit_loopback(&parsed) => {}
            "http" => return Err(SafeAuthUrlError::InsecureHost),
            _ => return Err(SafeAuthUrlError::UnsupportedScheme),
        }

        Ok(Self { raw, parsed })
    }

    /// Return the full validated URL for an explicit user display, copy, or open
    /// action. Callers must not log or persist this value.
    #[must_use]
    pub fn reveal(&self) -> &str {
        &self.raw
    }

    /// Extract the RFC 8628 user code when Grok embeds it in the validated
    /// device-flow URL. The returned value remains redacted under `Debug` and
    /// has no serialization or display implementation.
    #[must_use]
    pub fn device_user_code(&self) -> Option<SensitiveText> {
        self.parsed
            .query_pairs()
            .find(|(key, value)| key == "user_code" && !value.is_empty())
            .and_then(|(_, value)| SensitiveText::auth_code(value.into_owned()).ok())
    }

    fn redacted(&self) -> String {
        self.parsed.origin().ascii_serialization()
    }
}

impl fmt::Debug for SafeAuthUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SafeAuthUrl")
            .field(&self.redacted())
            .finish()
    }
}

impl fmt::Display for SafeAuthUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

fn is_explicit_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(host)) => host == Ipv4Addr::LOCALHOST,
        Some(Host::Ipv6(host)) => host == Ipv6Addr::LOCALHOST,
        None => false,
    }
}

/// Authentication text whose debug representation is always redacted.
///
/// This type deliberately implements no serialization, cloning, or display
/// trait. Its only wire conversion is the explicitly redacted
/// [`SensitiveAuthCodeParams`] boundary.
#[derive(Eq, PartialEq)]
pub struct SensitiveText(String);

impl SensitiveText {
    /// Wrap sensitive user input before validation at a protocol boundary.
    /// Prefer [`Self::auth_code`] for new authentication code paths.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Validate and wrap a callback URL, OAuth code, or device user code.
    pub fn auth_code(value: impl Into<String>) -> Result<Self, SensitiveTextError> {
        let value = value.into();
        validate_bounded_text(&value, MAX_AUTH_CODE_BYTES, false)
            .map_err(|()| sensitive_text_error(&value))?;
        Ok(Self(value))
    }

    /// Reveal the sensitive value only at its protocol-send boundary.
    #[must_use]
    pub fn reveal(&self) -> &str {
        &self.0
    }

    /// Return whether the sensitive value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Return the UTF-8 byte length without revealing the value.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn validate_auth_code(&self) -> Result<(), SensitiveTextError> {
        validate_bounded_text(&self.0, MAX_AUTH_CODE_BYTES, false)
            .map_err(|()| sensitive_text_error(&self.0))
    }
}

impl fmt::Debug for SensitiveText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SensitiveText([REDACTED])")
    }
}

/// Validation failure for sensitive authentication text.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SensitiveTextError {
    /// Empty input is not a valid submission.
    Empty,
    /// The submission exceeds the bounded callback/code size.
    TooLong,
    /// The submission contains a terminal/log control character.
    ControlCharacter,
}

impl fmt::Display for SensitiveTextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("authentication code is empty"),
            Self::TooLong => f.write_str("authentication code exceeds 8192 bytes"),
            Self::ControlCharacter => {
                f.write_str("authentication code contains a control character")
            }
        }
    }
}

impl Error for SensitiveTextError {}

fn sensitive_text_error(value: &str) -> SensitiveTextError {
    if value.is_empty() {
        SensitiveTextError::Empty
    } else if value.len() > MAX_AUTH_CODE_BYTES {
        SensitiveTextError::TooLong
    } else {
        SensitiveTextError::ControlCharacter
    }
}

/// Explicit send-boundary wrapper for `x.ai/auth/submit_code` parameters.
///
/// Its debug representation is always redacted and the wrapper intentionally
/// implements neither `Display` nor `serde::Serialize`. Call [`Self::reveal`]
/// only while constructing the outbound ACP frame; never log or persist the
/// returned JSON value.
pub struct SensitiveAuthCodeParams(serde_json::Value);

impl SensitiveAuthCodeParams {
    /// Validate a sensitive submission and build correctly escaped wire params.
    pub fn new(code: SensitiveText) -> Result<Self, SensitiveTextError> {
        code.validate_auth_code()?;
        let SensitiveText(code) = code;
        Ok(Self(serde_json::json!({ "code": code })))
    }

    /// Reveal the wire value only at the outbound protocol boundary.
    #[must_use]
    pub const fn reveal(&self) -> &serde_json::Value {
        &self.0
    }
}

impl fmt::Debug for SensitiveAuthCodeParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SensitiveAuthCodeParams([REDACTED])")
    }
}

/// One non-blocking user command sent to an authentication bootstrap.
#[derive(Debug, Eq, PartialEq)]
pub enum AuthCommand {
    /// Submit a manually pasted loopback code or callback URL.
    SubmitCode(SensitiveText),
    /// Cancel authentication and tear down the opening process.
    Cancel,
}

/// Failure to enqueue a command on an [`AuthControl`].
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthControlError {
    /// The bounded command channel is currently full.
    Full,
    /// The authentication bootstrap has already closed its receiver.
    Closed,
    /// A submitted callback/code failed bounded validation.
    InvalidCode(SensitiveTextError),
    /// The active device/command challenge does not accept manual code input.
    CodeNotAccepted,
}

impl fmt::Display for AuthControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => f.write_str("authentication command channel is full"),
            Self::Closed => f.write_str("authentication bootstrap is closed"),
            Self::InvalidCode(error) => error.fmt(f),
            Self::CodeNotAccepted => {
                f.write_str("this authentication mode does not accept a manual code")
            }
        }
    }
}

impl Error for AuthControlError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidCode(error) => Some(error),
            Self::Full | Self::Closed | Self::CodeNotAccepted => None,
        }
    }
}

/// Non-blocking control endpoint for one interactive authentication attempt.
#[derive(Clone)]
pub struct AuthControl {
    tx: mpsc::Sender<AuthCommand>,
    cancelled: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
    accepts_code: bool,
}

impl AuthControl {
    /// Create one bounded control endpoint and its host-side receiver.
    ///
    /// This compatibility constructor creates a loopback-capable control. New
    /// challenge paths should use [`Self::channel_for_mode`].
    #[must_use]
    pub fn channel() -> (Self, AuthControlReceiver) {
        Self::channel_for_mode(AuthMode::Loopback)
    }

    /// Create a bounded endpoint that enforces the challenge mode's manual-code
    /// contract.
    #[must_use]
    pub fn channel_for_mode(mode: AuthMode) -> (Self, AuthControlReceiver) {
        let (tx, rx) = mpsc::channel(AUTH_CONTROL_CAPACITY);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_notify = Arc::new(Notify::new());
        (
            Self {
                tx,
                cancelled: Arc::clone(&cancelled),
                cancel_notify: Arc::clone(&cancel_notify),
                accepts_code: mode.accepts_manual_code(),
            },
            AuthControlReceiver {
                rx,
                cancelled,
                cancel_notify,
                terminated: false,
            },
        )
    }

    /// Try to enqueue a command without waiting on the UI thread.
    pub fn try_send(&self, command: AuthCommand) -> Result<(), AuthControlError> {
        if matches!(&command, AuthCommand::Cancel) {
            return self.try_cancel();
        }
        if let AuthCommand::SubmitCode(code) = &command {
            if !self.accepts_code {
                return Err(AuthControlError::CodeNotAccepted);
            }
            code.validate_auth_code()
                .map_err(AuthControlError::InvalidCode)?;
        }
        self.tx.try_send(command).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => AuthControlError::Full,
            mpsc::error::TrySendError::Closed(_) => AuthControlError::Closed,
        })
    }

    /// Try to submit one sensitive loopback code without blocking.
    pub fn try_submit_code(&self, code: SensitiveText) -> Result<(), AuthControlError> {
        self.try_send(AuthCommand::SubmitCode(code))
    }

    /// Try to cancel authentication without blocking.
    pub fn try_cancel(&self) -> Result<(), AuthControlError> {
        if self.tx.is_closed() {
            return Err(AuthControlError::Closed);
        }
        self.cancelled.store(true, Ordering::Release);
        self.cancel_notify.notify_one();
        Ok(())
    }

    /// Return whether the host-side receiver has closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Host-side receiver for one authentication attempt.
///
/// Submitted codes remain bounded, while cancellation uses a coalescing
/// priority signal so it cannot be rejected merely because the code queue is
/// full.
pub struct AuthControlReceiver {
    rx: mpsc::Receiver<AuthCommand>,
    cancelled: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
    terminated: bool,
}

impl AuthControlReceiver {
    /// Receive the next control command. Cancellation is observed before queued
    /// code submissions, discards every queued secret, closes the sender, and is
    /// returned exactly once as the terminal command.
    pub async fn recv(&mut self) -> Option<AuthCommand> {
        if self.terminated {
            return None;
        }
        if self.cancelled.swap(false, Ordering::AcqRel) {
            return Some(self.terminate_cancelled());
        }
        loop {
            tokio::select! {
                biased;
                () = self.cancel_notify.notified() => {
                    if self.cancelled.swap(false, Ordering::AcqRel) {
                        return Some(self.terminate_cancelled());
                    }
                }
                command = self.rx.recv() => return command,
            }
        }
    }

    fn terminate_cancelled(&mut self) -> AuthCommand {
        self.terminated = true;
        self.rx.close();
        while self.rx.try_recv().is_ok() {}
        AuthCommand::Cancel
    }
}

impl fmt::Debug for AuthControlReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthControlReceiver(..)")
    }
}

impl fmt::Debug for AuthControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthControl(..)")
    }
}

/// Validated interactive authentication challenge produced before a live
/// session is ready.
#[derive(Debug)]
pub struct AuthChallenge {
    /// Session-opening attempt that owns this challenge.
    pub attempt_id: SessionOpenId,
    /// Exact authentication method selected from the advertised methods.
    pub method_id: String,
    /// Mode-specific presentation details. Command challenges contain only
    /// terminal-safe status and cannot expose a URL/code action.
    pub details: AuthUrlDetails,
    /// Local deadline after which the opener will cancel authentication.
    pub deadline: Instant,
}

impl AuthChallenge {
    /// Return the authoritative challenge mode.
    #[must_use]
    pub const fn mode(&self) -> AuthMode {
        self.details.mode()
    }

    /// Return a validated URL only for loopback and device challenges.
    #[must_use]
    pub const fn safe_url(&self) -> Option<&SafeAuthUrl> {
        self.details.safe_url()
    }

    /// Return ephemeral external-command status only for command challenges.
    #[must_use]
    pub const fn command_status(&self) -> Option<&SafeCommandAuthStatus> {
        self.details.command_status()
    }

    /// Return a device user code only for device challenges that supplied one.
    #[must_use]
    pub const fn user_code(&self) -> Option<&SensitiveText> {
        self.details.user_code()
    }

    /// Whether an explicit UI action may open or copy a validated URL.
    #[must_use]
    pub const fn can_open_or_copy_url(&self) -> bool {
        self.details.can_open_or_copy_url()
    }

    /// Whether the challenge accepts a manually pasted callback/code.
    #[must_use]
    pub const fn accepts_manual_code(&self) -> bool {
        self.details.accepts_manual_code()
    }
}

/// Terminal state of one authentication attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthOutcome {
    /// Authentication completed successfully.
    Authenticated,
    /// The user or caller cancelled the attempt.
    Cancelled,
    /// Authentication failed or timed out.
    Failed,
}

/// Terminal authentication event for a session-opening attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AuthSettled {
    /// Session-opening attempt that settled.
    pub attempt_id: SessionOpenId,
    /// Safe terminal outcome; detailed errors travel through the open result.
    pub outcome: AuthOutcome,
}

/// Pre-session event delivered while a base session is still opening.
#[derive(Debug)]
pub enum SessionOpenEvent {
    /// Authentication requires live user interaction.
    Challenge {
        /// Validated challenge safe for explicit UI presentation.
        challenge: Box<AuthChallenge>,
        /// Bounded non-blocking command endpoint for this challenge.
        control: AuthControl,
    },
    /// Authentication reached a terminal state.
    Settled(AuthSettled),
}

/// Non-blocking sender used by a session opener to publish pre-session events.
pub type SessionOpenEventSender = mpsc::UnboundedSender<SessionOpenEvent>;

/// Whether a session open may begin interactive authentication.
#[derive(Debug, Clone)]
pub enum SessionOpenPolicy {
    /// Never start an authentication flow that may require user interaction.
    NonInteractive,
    /// The user explicitly authorized an interactive attempt.
    UserAuthorized {
        /// Caller-owned attempt identity used to reject stale UI actions.
        attempt_id: SessionOpenId,
        /// Exact interactive method id selected from the most recent
        /// [`AuthOffer`]. The opener rejects ids the new initialize response no
        /// longer advertises instead of silently choosing another account path.
        method_id: String,
        /// Event channel polled by the interactive surface before a session exists.
        events: SessionOpenEventSender,
    },
}

impl SessionOpenPolicy {
    /// Return the interactive attempt identity, when user authorization exists.
    #[must_use]
    pub const fn attempt_id(&self) -> Option<SessionOpenId> {
        match self {
            Self::NonInteractive => None,
            Self::UserAuthorized { attempt_id, .. } => Some(*attempt_id),
        }
    }

    /// Return the explicitly selected authentication method, when interactive
    /// authorization exists.
    #[must_use]
    pub fn method_id(&self) -> Option<&str> {
        match self {
            Self::NonInteractive => None,
            Self::UserAuthorized { method_id, .. } => Some(method_id),
        }
    }

    /// Return the pre-session event sender for a user-authorized attempt.
    #[must_use]
    pub fn event_sender(&self) -> Option<&SessionOpenEventSender> {
        match self {
            Self::NonInteractive => None,
            Self::UserAuthorized { events, .. } => Some(events),
        }
    }
}

/// Typed failure returned while opening a base session.
#[derive(Debug)]
pub enum SessionOpenError {
    /// The underlying process or session protocol failed.
    Session(SessionError),
    /// Opening stopped before interaction and returned the advertised choices.
    AuthRequired(AuthOffer),
}

impl fmt::Display for SessionOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(error) => error.fmt(f),
            Self::AuthRequired(offer) => {
                write!(
                    f,
                    "authentication required for backend `{}`",
                    offer.backend_id
                )
            }
        }
    }
}

impl Error for SessionOpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Session(error) => Some(error),
            Self::AuthRequired(_) => None,
        }
    }
}

impl From<SessionError> for SessionOpenError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_mode_accepts_only_the_documented_wire_values() {
        for (wire, expected) in [
            ("loopback", AuthMode::Loopback),
            ("command", AuthMode::Command),
            ("device", AuthMode::Device),
        ] {
            assert_eq!(AuthMode::from_wire(wire), Some(expected));
            assert_eq!(expected.as_wire_str(), wire);
        }
        assert_eq!(AuthMode::from_wire("browser"), None);
        assert_eq!(AuthMode::from_wire("Loopback"), None);
    }

    #[test]
    fn auth_offer_resolves_only_an_advertised_default() {
        let methods = vec![
            AuthMethodSummary::new("cached_token", "Cached token", false),
            AuthMethodSummary::new("grok.com", "Grok", true),
        ];
        let offer = AuthOffer::new(
            "grok-build",
            methods.clone(),
            Some("cached_token".to_string()),
            true,
        );
        assert_eq!(offer.default_method(), methods.first());

        let invalid = AuthOffer::new(
            "grok-build",
            methods,
            Some("not-advertised".to_string()),
            true,
        );
        assert_eq!(invalid.default_method(), None);
    }

    #[test]
    fn grok_catalog_honors_audited_default_and_never_reauthenticates_cached_token() {
        let catalog = GrokAuthCatalog::parse_initialize(&serde_json::json!({
            "authMethods": [
                { "id": "xai.api_key", "name": "API key" },
                { "id": "cached_token", "name": "Cached token" },
                { "id": "grok.com", "name": "Grok" }
            ],
            "_meta": { "defaultAuthMethodId": "cached_token" }
        }))
        .unwrap();

        assert_eq!(
            catalog.bootstrap_decision(),
            GrokAuthBootstrapDecision::UseInitializedCachedToken
        );
        assert_eq!(
            catalog
                .bootstrap_decision()
                .non_interactive_authenticate_params(),
            None,
            "initialize already validated/refreshed cached_token"
        );
        assert_eq!(
            catalog.default_method().map(GrokAuthMethod::kind),
            Some(GrokAuthMethodKind::CachedToken)
        );
    }

    #[test]
    fn grok_catalog_builds_only_audited_automatic_auth_request() {
        let api_key = GrokAuthCatalog::parse_initialize(&serde_json::json!({
            "authMethods": [
                { "id": "xai.api_key", "name": "API key" },
                { "id": "grok.com", "name": "Grok" }
            ],
            "_meta": { "defaultAuthMethodId": "xai.api_key" }
        }))
        .unwrap();
        let decision = api_key.bootstrap_decision();
        assert_eq!(
            decision.non_interactive_authenticate_params(),
            Some(serde_json::json!({
                "methodId": "xai.api_key",
                "_meta": { "headless": true }
            }))
        );

        let interactive = GrokAuthCatalog::parse_initialize(&serde_json::json!({
            "authMethods": [
                {
                    "id": "grok.com",
                    "name": "Company login",
                    "_meta": { "external_provider": true }
                }
            ],
            "_meta": { "defaultAuthMethodId": null }
        }))
        .unwrap();
        assert!(matches!(
            interactive.bootstrap_decision(),
            GrokAuthBootstrapDecision::UserActionRequired(_)
        ));
        assert!(interactive
            .bootstrap_decision()
            .non_interactive_authenticate_params()
            .is_none());
        let selected = interactive.interactive_selection("grok.com").unwrap();
        assert!(selected.external_provider_command());
        assert_eq!(
            selected.authenticate_params(),
            serde_json::json!({ "methodId": "grok.com" })
        );
        assert_eq!(
            interactive.interactive_selection("cached_token"),
            Err(GrokAuthContractError::MethodNotAdvertised)
        );
    }

    #[test]
    fn grok_catalog_revalidates_defaults_selections_and_display_fields() {
        let catalog = GrokAuthCatalog::parse_initialize(&serde_json::json!({
            "authMethods": [
                { "id": "cached_token", "name": "Cached token" },
                { "id": "oidc", "name": "Enterprise SSO" }
            ],
            "_meta": { "defaultAuthMethodId": "removed-method" }
        }))
        .unwrap();
        assert!(catalog.default_method().is_none());
        assert_eq!(
            catalog.bootstrap_decision(),
            GrokAuthBootstrapDecision::UseInitializedCachedToken
        );
        assert_eq!(
            catalog.interactive_selection("cached_token"),
            Err(GrokAuthContractError::MethodNotInteractive)
        );
        assert_eq!(
            catalog.interactive_selection("removed-method"),
            Err(GrokAuthContractError::MethodNotAdvertised)
        );

        for (value, expected) in [
            (
                serde_json::json!({ "authMethods": [{"id":"bad\nid"}] }),
                GrokAuthContractError::InvalidMethodId,
            ),
            (
                serde_json::json!({
                    "authMethods": [{"id":"oidc","name":"bad\nlabel"}]
                }),
                GrokAuthContractError::InvalidMethodLabel,
            ),
            (
                serde_json::json!({
                    "authMethods": [{"id":"oidc"},{"id":"oidc"}]
                }),
                GrokAuthContractError::DuplicateMethodId,
            ),
            (
                serde_json::json!({
                    "authMethods": [{
                        "id":"xai.api_key",
                        "_meta":{"external_provider":true}
                    }]
                }),
                GrokAuthContractError::InvalidMethodMetadata,
            ),
            (
                serde_json::json!({
                    "authMethods": [{"id": "a".repeat(MAX_AUTH_METHOD_ID_BYTES + 1)}]
                }),
                GrokAuthContractError::InvalidMethodId,
            ),
        ] {
            assert_eq!(GrokAuthCatalog::parse_initialize(&value), Err(expected));
        }
    }

    #[test]
    fn safe_auth_url_accepts_https_and_explicit_loopback_http() {
        for value in [
            "https://accounts.x.ai/oauth?state=secret#callback",
            "http://localhost:43123/callback?code=secret",
            "http://127.0.0.1:43123/callback?code=secret",
            "http://[::1]:43123/callback?code=secret",
        ] {
            assert_eq!(SafeAuthUrl::parse(value).unwrap().reveal(), value);
        }
    }

    #[test]
    fn safe_auth_url_rejects_unsafe_inputs() {
        let too_long = format!("https://example.com/{}", "a".repeat(MAX_AUTH_URL_BYTES));
        for (value, expected) in [
            (
                "https://example.com/\nnext",
                SafeAuthUrlError::ControlCharacter,
            ),
            ("javascript:alert(1)", SafeAuthUrlError::UnsupportedScheme),
            ("http://example.com/login", SafeAuthUrlError::InsecureHost),
            ("https://user@example.com/login", SafeAuthUrlError::UserInfo),
            (too_long.as_str(), SafeAuthUrlError::TooLong),
        ] {
            assert_eq!(SafeAuthUrl::parse(value), Err(expected), "{value}");
        }
    }

    #[test]
    fn safe_auth_url_formatting_hides_query_and_fragment() {
        let url = SafeAuthUrl::parse(
            "https://accounts.x.ai/oauth/start?state=private&user_code=ABCD#private-fragment",
        )
        .unwrap();
        let display = url.to_string();
        let debug = format!("{url:?}");

        assert_eq!(display, "https://accounts.x.ai");
        assert!(!debug.contains("private"));
        assert!(!debug.contains("ABCD"));
        assert!(url.reveal().contains("state=private"));
    }

    #[test]
    fn grok_auth_url_poll_preserves_source_modes_and_browser_order() {
        for (mode, raw_value, expected_order, accepts_code) in [
            (
                "loopback",
                "http://127.0.0.1:43123/callback?state=private",
                AuthBrowserUrlOrder::BrowserBeforeUrl,
                true,
            ),
            (
                "command",
                "Launching company SSO...\nWaiting for browser callback",
                AuthBrowserUrlOrder::ExternalCommandOwnsBrowser,
                false,
            ),
            (
                "device",
                "https://accounts.x.ai/device?user_code=ABCD-EFGH&state=private",
                AuthBrowserUrlOrder::UrlBeforeBrowser,
                false,
            ),
        ] {
            let response = serde_json::json!({
                "auth_url": raw_value,
                "external_provider": mode == "command",
                "mode": mode
            });
            let AuthUrlPoll::Ready(details) = parse_grok_auth_url_poll(&response).unwrap() else {
                panic!("expected a ready challenge");
            };
            assert_eq!(details.mode().browser_url_order(), expected_order);
            assert_eq!(details.browser_url_order(), expected_order);
            assert_eq!(details.accepts_manual_code(), accepts_code);
            assert_eq!(details.safe_url().is_some(), mode != "command");
            assert_eq!(details.can_open_or_copy_url(), mode != "command");
            assert_eq!(details.command_status().is_some(), mode == "command");
            assert_eq!(details.user_code().is_some(), mode == "device");
            assert!(!format!("{details:?}").contains("private"));
        }

        assert!(matches!(
            parse_grok_auth_url_poll(&serde_json::json!({
                "auth_url": null,
                "external_provider": false,
                "mode": null
            })),
            Ok(AuthUrlPoll::Pending)
        ));
        assert_eq!(GROK_AUTH_URL_POLL_ATTEMPTS, 60);
        assert_eq!(
            GROK_AUTH_URL_POLL_INTERVAL,
            std::time::Duration::from_millis(50)
        );
    }

    #[test]
    fn command_auth_status_is_multiline_terminal_safe_redacted_and_not_actionable() {
        let response = serde_json::json!({
            "auth_url": concat!(
                "\u{1b}[2JLaunching company SSO...\n",
                "api_key=sk-live-secret\n",
                "Visit https://idp.example/login?state=private-token\n",
                "\u{1b}]8;;https://phish.example/session?code=secret\u{7}",
                "Waiting for browser callback",
                "\u{1b}]8;;\u{7}\u{202e}"
            ),
            "external_provider": true,
            "mode": "command"
        });
        let AuthUrlPoll::Ready(details) = parse_grok_auth_url_poll(&response).unwrap() else {
            panic!("expected command status");
        };
        let status = details.command_status().unwrap();
        let rendered = status.reveal_for_render();

        assert_eq!(details.mode(), AuthMode::Command);
        assert!(details.safe_url().is_none());
        assert!(details.user_code().is_none());
        assert!(!details.can_open_or_copy_url());
        assert!(!details.accepts_manual_code());
        assert!(rendered.contains("Launching company SSO...\n"));
        assert!(rendered.contains("\nWaiting for browser callback"));
        assert!(rendered.contains("[authentication URL hidden]"));
        assert!(!rendered.contains("sk-live-secret"));
        assert!(!rendered.contains("private-token"));
        assert!(!rendered.contains("phish.example"));
        assert!(!rendered.contains("http://"));
        assert!(!rendered.contains("https://"));
        assert!(!rendered.chars().any(is_unsafe_render_character));
        assert_eq!(status.line_count(), 4);
        assert!(!status.is_empty());
        assert_eq!(status.len(), rendered.len());

        let debug = format!("{details:?}");
        assert!(!debug.contains("Launching company SSO"));
        assert!(!debug.contains("sk-live-secret"));
        assert!(!debug.contains("private-token"));
    }

    #[test]
    fn command_auth_status_enforces_raw_and_sanitized_bounds() {
        assert_eq!(
            SafeCommandAuthStatus::from_untrusted("").unwrap_err(),
            SafeCommandAuthStatusError::Empty
        );
        assert_eq!(
            SafeCommandAuthStatus::from_untrusted("a".repeat(MAX_COMMAND_AUTH_STATUS_BYTES + 1))
                .unwrap_err(),
            SafeCommandAuthStatusError::TooLong
        );
        assert_eq!(
            SafeCommandAuthStatus::from_untrusted(
                vec!["status"; MAX_COMMAND_AUTH_STATUS_LINES + 1].join("\n")
            )
            .unwrap_err(),
            SafeCommandAuthStatusError::TooManyLines
        );
        assert_eq!(
            SafeCommandAuthStatus::from_untrusted("\u{1b}]unterminated control string")
                .unwrap_err(),
            SafeCommandAuthStatusError::Empty
        );
    }

    #[test]
    fn grok_auth_url_poll_fails_closed_on_inconsistent_or_unsafe_responses() {
        for (response, expected) in [
            (
                serde_json::json!({
                    "auth_url": "https://accounts.x.ai/login",
                    "external_provider": false,
                    "mode": "command"
                }),
                AuthUrlPollError::InconsistentExternalProvider,
            ),
            (
                serde_json::json!({
                    "auth_url": "https://accounts.x.ai/login",
                    "external_provider": false,
                    "mode": "future"
                }),
                AuthUrlPollError::UnknownMode,
            ),
            (
                serde_json::json!({
                    "auth_url": "http://attacker.example/login",
                    "external_provider": false,
                    "mode": "loopback"
                }),
                AuthUrlPollError::UnsafeUrl(SafeAuthUrlError::InsecureHost),
            ),
            (
                serde_json::json!({
                    "auth_url": "http://attacker.example/device",
                    "external_provider": false,
                    "mode": "device"
                }),
                AuthUrlPollError::UnsafeUrl(SafeAuthUrlError::InsecureHost),
            ),
            (
                serde_json::json!({
                    "auth_url": "",
                    "external_provider": true,
                    "mode": "command"
                }),
                AuthUrlPollError::UnsafeCommandStatus(SafeCommandAuthStatusError::Empty),
            ),
            (
                serde_json::json!({
                    "auth_url": "https://accounts.x.ai/login",
                    "external_provider": false,
                    "mode": null
                }),
                AuthUrlPollError::IncompleteChallenge,
            ),
        ] {
            assert_eq!(parse_grok_auth_url_poll(&response).unwrap_err(), expected);
        }
    }

    #[test]
    fn device_user_code_stays_sensitive() {
        let url =
            SafeAuthUrl::parse("https://accounts.x.ai/device?user_code=ABCD-EFGH&state=private")
                .unwrap();
        let code = url.device_user_code().unwrap();
        assert_eq!(code.reveal(), "ABCD-EFGH");
        assert!(!format!("{code:?}").contains("ABCD-EFGH"));
        assert!(!url.to_string().contains("ABCD-EFGH"));
    }

    #[test]
    fn sensitive_text_debug_is_redacted() {
        let value = SensitiveText::new("one-time-secret");
        assert_eq!(value.reveal(), "one-time-secret");
        assert_eq!(value.len(), 15);
        assert_eq!(format!("{value:?}"), "SensitiveText([REDACTED])");
        assert!(!format!("{value:?}").contains(value.reveal()));
    }

    #[test]
    fn auth_code_validation_and_wire_wrapper_do_not_leak_in_formatting() {
        for (value, expected) in [
            ("", SensitiveTextError::Empty),
            ("line\nfeed", SensitiveTextError::ControlCharacter),
        ] {
            assert_eq!(SensitiveText::auth_code(value), Err(expected));
        }
        assert_eq!(
            SensitiveText::auth_code("a".repeat(MAX_AUTH_CODE_BYTES + 1)),
            Err(SensitiveTextError::TooLong)
        );

        let secret = "callback-secret-value";
        let params =
            SensitiveAuthCodeParams::new(SensitiveText::auth_code(secret).unwrap()).unwrap();
        assert_eq!(params.reveal(), &serde_json::json!({ "code": secret }));
        assert!(!format!("{params:?}").contains(secret));
    }

    #[tokio::test]
    async fn auth_control_is_bounded_and_cancel_has_priority() {
        let (control, mut rx) = AuthControl::channel();
        control
            .try_submit_code(SensitiveText::new("callback-code"))
            .unwrap();
        control
            .try_submit_code(SensitiveText::new("second-code"))
            .unwrap();
        assert_eq!(
            control.try_submit_code(SensitiveText::new("queue-is-full")),
            Err(AuthControlError::Full)
        );
        control.try_cancel().unwrap();

        assert_eq!(rx.recv().await, Some(AuthCommand::Cancel));
        assert_eq!(rx.recv().await, None, "cancel is terminal");
        assert!(control.is_closed());
        assert_eq!(
            control.try_submit_code(SensitiveText::new("stale-code")),
            Err(AuthControlError::Closed)
        );
    }

    #[test]
    fn auth_control_rejects_invalid_codes_before_queueing() {
        let (control, _rx) = AuthControl::channel();
        assert_eq!(
            control.try_submit_code(SensitiveText::new("forged\ncode")),
            Err(AuthControlError::InvalidCode(
                SensitiveTextError::ControlCharacter
            ))
        );

        for mode in [AuthMode::Device, AuthMode::Command] {
            let (control, _rx) = AuthControl::channel_for_mode(mode);
            assert_eq!(
                control.try_submit_code(SensitiveText::new("valid-but-not-applicable")),
                Err(AuthControlError::CodeNotAccepted)
            );
        }
    }

    #[test]
    fn auth_control_reports_closed_receiver() {
        let (control, rx) = AuthControl::channel();
        drop(rx);
        assert_eq!(control.try_cancel(), Err(AuthControlError::Closed));
        assert!(control.is_closed());
    }

    #[test]
    fn policy_exposes_only_user_authorized_event_channels() {
        let non_interactive = SessionOpenPolicy::NonInteractive;
        assert_eq!(non_interactive.attempt_id(), None);
        assert!(non_interactive.event_sender().is_none());

        let (tx, _rx) = mpsc::unbounded_channel();
        let interactive = SessionOpenPolicy::UserAuthorized {
            attempt_id: SessionOpenId::new(7),
            method_id: "grok.com".to_string(),
            events: tx,
        };
        assert_eq!(interactive.attempt_id(), Some(SessionOpenId::new(7)));
        assert_eq!(interactive.method_id(), Some("grok.com"));
        assert!(interactive.event_sender().is_some());
    }

    #[test]
    fn session_open_error_preserves_its_typed_cause() {
        let session = SessionOpenError::from(SessionError::Closed);
        assert!(matches!(
            session,
            SessionOpenError::Session(SessionError::Closed)
        ));

        let offer = AuthOffer::new("grok-build", Vec::new(), None, true);
        let required = SessionOpenError::AuthRequired(offer.clone());
        assert!(matches!(
            required,
            SessionOpenError::AuthRequired(ref actual) if actual == &offer
        ));
        assert_eq!(
            required.to_string(),
            "authentication required for backend `grok-build`"
        );
    }
}
