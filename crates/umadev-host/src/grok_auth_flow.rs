//! Pure, generation-bound policy for opening a Grok Build authenticated session.
//!
//! This module decides which protocol actions are safe. It performs no I/O,
//! starts no process, and never opens a browser. The ACP driver is responsible
//! for executing returned actions and reaping its child on `AbortAndReap`.

use std::error::Error;
use std::fmt;
use std::time::{Duration, Instant};

use crate::session_bootstrap::{
    AuthChallenge, AuthMode, AuthOffer, AuthOutcome, AuthSettled, AuthUrlPoll, AuthUrlPollError,
    GrokAuthBootstrapDecision, GrokAuthCatalog, GrokAuthContractError, SensitiveAuthCodeParams,
    SensitiveText, SensitiveTextError, SessionOpenId, GROK_AUTH_URL_POLL_ATTEMPTS,
    GROK_AUTH_URL_POLL_INTERVAL,
};

/// Standard ACP authentication method.
pub const GROK_AUTHENTICATE_METHOD: &str = "authenticate";

/// Grok Build extension used to obtain an interactive authentication challenge.
pub const GROK_AUTH_GET_URL_METHOD: &str = "x.ai/auth/get_url";

/// Grok Build extension used to submit a loopback callback URL or code.
pub const GROK_AUTH_SUBMIT_CODE_METHOD: &str = "x.ai/auth/submit_code";

/// Observable phase of one generation-bound authentication attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GrokAuthAttemptPhase {
    /// Waiting for the first initialize result.
    AwaitingInitialize,
    /// Waiting for exact API-key authentication to finish.
    AwaitingApiKeyAuthentication,
    /// Waiting for the user to explicitly choose and confirm an interactive method.
    AwaitingExplicitConfirmation,
    /// Interactive authenticate and URL discovery are in flight.
    AwaitingInteractiveChallenge,
    /// A validated mode-specific challenge has been presented.
    Challenge(AuthMode),
    /// Authentication is ready and session creation may continue.
    Authenticated,
    /// The user cancelled the attempt.
    Cancelled,
    /// The attempt exceeded its deadline.
    TimedOut,
    /// A protocol, authentication, transport, or UI-channel failure stopped the attempt.
    Failed,
}

impl GrokAuthAttemptPhase {
    /// Whether no later event may change this phase.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Authenticated | Self::Cancelled | Self::TimedOut | Self::Failed
        )
    }
}

/// Explicitly confirmed options for an interactive Grok authentication request.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct GrokInteractiveAuthOptions {
    force_loopback: bool,
    force_interactive: bool,
}

impl GrokInteractiveAuthOptions {
    /// Start an initial interactive login, optionally forcing loopback transport.
    #[must_use]
    pub const fn initial(force_loopback: bool) -> Self {
        Self {
            force_loopback,
            force_interactive: false,
        }
    }

    /// Start an account switch without clearing the currently usable credential.
    #[must_use]
    pub const fn account_switch(force_loopback: bool) -> Self {
        Self {
            force_loopback,
            force_interactive: true,
        }
    }

    /// Whether Grok must use its loopback callback transport.
    #[must_use]
    pub const fn force_loopback(self) -> bool {
        self.force_loopback
    }

    /// Whether Grok must skip, but not clear, its current cached credential.
    #[must_use]
    pub const fn force_interactive(self) -> bool {
        self.force_interactive
    }
}

/// Non-secret outbound RPC selected by the authentication policy.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GrokAuthRpc {
    method: &'static str,
    params: serde_json::Value,
    may_open_browser: bool,
}

impl GrokAuthRpc {
    fn new(method: &'static str, params: serde_json::Value, may_open_browser: bool) -> Self {
        Self {
            method,
            params,
            may_open_browser,
        }
    }

    /// Exact ACP or extension method name.
    #[must_use]
    pub const fn method(&self) -> &'static str {
        self.method
    }

    /// Non-secret JSON parameters to send.
    #[must_use]
    pub const fn params(&self) -> &serde_json::Value {
        &self.params
    }

    /// Whether sending this RPC may cause the Grok child to open a browser.
    #[must_use]
    pub const fn may_open_browser(&self) -> bool {
        self.may_open_browser
    }
}

/// Sensitive loopback-code RPC selected by the authentication policy.
pub struct GrokSensitiveAuthRpc {
    method: &'static str,
    params: SensitiveAuthCodeParams,
}

impl GrokSensitiveAuthRpc {
    fn submit_code(params: SensitiveAuthCodeParams) -> Self {
        Self {
            method: GROK_AUTH_SUBMIT_CODE_METHOD,
            params,
        }
    }

    /// Exact Grok extension method name.
    #[must_use]
    pub const fn method(&self) -> &'static str {
        self.method
    }

    /// Reveal the sensitive value only while constructing the outbound frame.
    #[must_use]
    pub const fn reveal_params(&self) -> &serde_json::Value {
        self.params.reveal()
    }
}

impl fmt::Debug for GrokSensitiveAuthRpc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GrokSensitiveAuthRpc([REDACTED])")
    }
}

/// Why an active pre-session authentication attempt must be aborted and reaped.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GrokAuthStopReason {
    /// The user explicitly cancelled the attempt.
    Cancelled,
    /// The attempt reached its local deadline.
    TimedOut,
    /// The initialize authentication advertisement violated its contract.
    InitializeContract(GrokAuthContractError),
    /// The interactive URL response violated its source contract.
    AuthUrlContract(AuthUrlPollError),
    /// All source-compatible empty URL polls were consumed.
    AuthUrlPollExhausted,
    /// Grok rejected or failed the authenticate request.
    AuthenticationFailed,
    /// The ACP transport disconnected before authentication settled.
    TransportClosed,
    /// The pre-session event receiver disappeared while interaction was active.
    EventReceiverClosed,
}

/// A stale or already-settled event that cannot affect the active generation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GrokAuthIgnoredEvent {
    /// The event belongs to another caller-owned generation.
    StaleGeneration {
        /// Generation owned by this state machine.
        active: SessionOpenId,
        /// Generation carried by the ignored event.
        received: SessionOpenId,
    },
    /// The event arrived after this generation had already settled.
    LateAfterSettlement {
        /// Settled generation that rejected the event.
        attempt_id: SessionOpenId,
        /// Terminal phase preserved by the state machine.
        phase: GrokAuthAttemptPhase,
    },
    /// URL discovery already produced a challenge, so this response is obsolete.
    SupersededUrlResponse {
        /// Generation that already owns a challenge.
        attempt_id: SessionOpenId,
    },
}

/// One pure action for the ACP driver or interactive surface to execute.
#[derive(Debug)]
pub enum GrokAuthAction {
    /// Present an offer without sending a browser-capable RPC.
    PresentOffer(AuthOffer),
    /// Continue session creation using the credential validated during initialize.
    SessionReady(AuthSettled),
    /// Send one non-secret RPC.
    SendRpc(GrokAuthRpc),
    /// Start interactive authenticate and URL discovery concurrently.
    StartInteractive {
        /// Browser-capable authenticate request.
        authenticate: GrokAuthRpc,
        /// Non-browser-capable `x.ai/auth/get_url` request.
        get_url: GrokAuthRpc,
    },
    /// Retry URL discovery after the source-compatible delay.
    PollAuthUrlAfter {
        /// Delay before sending the next request.
        delay: Duration,
        /// Exact empty-params URL request.
        request: GrokAuthRpc,
    },
    /// Present a validated, mode-specific challenge.
    PresentChallenge(Box<AuthChallenge>),
    /// Send a redacted loopback-code request.
    SendSensitiveRpc(GrokSensitiveAuthRpc),
    /// Stop the opener and reap its entire process tree.
    AbortAndReap {
        /// Safe terminal event for the attempt.
        settled: AuthSettled,
        /// Typed, non-secret reason for stopping.
        reason: GrokAuthStopReason,
    },
    /// Discard a stale, late, or superseded event without changing state.
    Ignored(GrokAuthIgnoredEvent),
}

/// Rejected state-machine operation that did not emit any protocol action.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GrokAuthAttemptError {
    /// The operation is not valid in the current phase.
    InvalidTransition {
        /// Current phase left unchanged.
        phase: GrokAuthAttemptPhase,
        /// Static operation name for diagnostics.
        operation: &'static str,
    },
    /// The user selected a method that is absent or not audited as interactive.
    InteractiveSelection(GrokAuthContractError),
    /// A submitted loopback callback/code failed bounded validation.
    InvalidCode(SensitiveTextError),
    /// No validated loopback challenge is active.
    CodeNotAccepted,
}

impl fmt::Display for GrokAuthAttemptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition { operation, .. } => {
                write!(f, "authentication operation `{operation}` is not valid now")
            }
            Self::InteractiveSelection(error) => error.fmt(f),
            Self::InvalidCode(error) => error.fmt(f),
            Self::CodeNotAccepted => {
                f.write_str("no loopback authentication challenge accepts a code")
            }
        }
    }
}

impl Error for GrokAuthAttemptError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InteractiveSelection(error) => Some(error),
            Self::InvalidCode(error) => Some(error),
            Self::InvalidTransition { .. } | Self::CodeNotAccepted => None,
        }
    }
}

/// Pure policy and lifecycle state for one Grok authentication generation.
#[derive(Debug)]
pub struct GrokAuthAttempt {
    attempt_id: SessionOpenId,
    deadline: Instant,
    phase: GrokAuthAttemptPhase,
    catalog: Option<GrokAuthCatalog>,
    selected_method_id: Option<String>,
    explicitly_confirmed: bool,
    auth_url_requests_started: usize,
    outbound_rpc_count: usize,
    browser_capable_rpc_count: usize,
}

impl GrokAuthAttempt {
    /// Create a side-effect-free attempt for one caller-owned generation.
    #[must_use]
    pub const fn new(attempt_id: SessionOpenId, deadline: Instant) -> Self {
        Self {
            attempt_id,
            deadline,
            phase: GrokAuthAttemptPhase::AwaitingInitialize,
            catalog: None,
            selected_method_id: None,
            explicitly_confirmed: false,
            auth_url_requests_started: 0,
            outbound_rpc_count: 0,
            browser_capable_rpc_count: 0,
        }
    }

    /// Generation owned by this state machine.
    #[must_use]
    pub const fn attempt_id(&self) -> SessionOpenId {
        self.attempt_id
    }

    /// Current observable phase.
    #[must_use]
    pub const fn phase(&self) -> GrokAuthAttemptPhase {
        self.phase
    }

    /// Absolute local deadline applied to every event.
    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Whether an audited interactive method has been explicitly confirmed.
    #[must_use]
    pub const fn explicitly_confirmed(&self) -> bool {
        self.explicitly_confirmed
    }

    /// Number of outbound RPCs selected by this state machine.
    #[must_use]
    pub const fn outbound_rpc_count(&self) -> usize {
        self.outbound_rpc_count
    }

    /// Number of selected RPCs capable of causing Grok to open a browser.
    #[must_use]
    pub const fn browser_capable_rpc_count(&self) -> usize {
        self.browser_capable_rpc_count
    }

    /// Number of `x.ai/auth/get_url` requests selected for this attempt.
    #[must_use]
    pub const fn auth_url_requests_started(&self) -> usize {
        self.auth_url_requests_started
    }

    /// Consume the exact authentication catalog from one initialize result.
    pub fn initialized(
        &mut self,
        attempt_id: SessionOpenId,
        initialize_result: &serde_json::Value,
        now: Instant,
    ) -> Result<GrokAuthAction, GrokAuthAttemptError> {
        if let Some(action) = self.preflight(attempt_id, now) {
            return Ok(action);
        }
        self.require_phase(
            GrokAuthAttemptPhase::AwaitingInitialize,
            "initialize authentication catalog",
        )?;

        let catalog = match GrokAuthCatalog::parse_initialize(initialize_result) {
            Ok(catalog) => catalog,
            Err(error) => {
                return Ok(self.abort(GrokAuthStopReason::InitializeContract(error)));
            }
        };
        let decision = catalog.bootstrap_decision();
        self.catalog = Some(catalog);
        match decision {
            GrokAuthBootstrapDecision::UseInitializedCachedToken => Ok(self.ready_without_rpc()),
            GrokAuthBootstrapDecision::AuthenticateNonInteractive { method_id } => {
                let params = serde_json::json!({
                    "methodId": method_id,
                    "_meta": { "headless": true }
                });
                self.phase = GrokAuthAttemptPhase::AwaitingApiKeyAuthentication;
                self.outbound_rpc_count += 1;
                Ok(GrokAuthAction::SendRpc(GrokAuthRpc::new(
                    GROK_AUTHENTICATE_METHOD,
                    params,
                    false,
                )))
            }
            GrokAuthBootstrapDecision::UserActionRequired(offer) => {
                self.phase = GrokAuthAttemptPhase::AwaitingExplicitConfirmation;
                Ok(GrokAuthAction::PresentOffer(offer))
            }
        }
    }

    /// Re-parse a fresh initialize result after an earlier offer was confirmed.
    ///
    /// The selected method is revalidated against this new child generation;
    /// no catalog or credential decision from the offer-producing child is
    /// reused. The returned interactive action is therefore the first point at
    /// which this generation can emit a browser-capable RPC.
    pub fn initialized_with_explicit_confirmation(
        &mut self,
        attempt_id: SessionOpenId,
        initialize_result: &serde_json::Value,
        method_id: &str,
        options: GrokInteractiveAuthOptions,
        now: Instant,
    ) -> Result<GrokAuthAction, GrokAuthAttemptError> {
        if let Some(action) = self.preflight(attempt_id, now) {
            return Ok(action);
        }
        self.require_phase(
            GrokAuthAttemptPhase::AwaitingInitialize,
            "reinitialize confirmed authentication",
        )?;
        let catalog = match GrokAuthCatalog::parse_initialize(initialize_result) {
            Ok(catalog) => catalog,
            Err(error) => {
                return Ok(self.abort(GrokAuthStopReason::InitializeContract(error)));
            }
        };
        catalog
            .interactive_selection(method_id)
            .map_err(GrokAuthAttemptError::InteractiveSelection)?;
        self.catalog = Some(catalog);
        self.phase = GrokAuthAttemptPhase::AwaitingExplicitConfirmation;
        self.confirm_interactive(attempt_id, method_id, options, now)
    }

    /// Confirm one exact interactive method and select concurrent protocol work.
    pub fn confirm_interactive(
        &mut self,
        attempt_id: SessionOpenId,
        method_id: &str,
        options: GrokInteractiveAuthOptions,
        now: Instant,
    ) -> Result<GrokAuthAction, GrokAuthAttemptError> {
        if let Some(action) = self.preflight(attempt_id, now) {
            return Ok(action);
        }
        self.require_phase(
            GrokAuthAttemptPhase::AwaitingExplicitConfirmation,
            "confirm interactive authentication",
        )?;
        let catalog = self
            .catalog
            .as_ref()
            .ok_or(GrokAuthAttemptError::InvalidTransition {
                phase: self.phase,
                operation: "confirm authentication without initialize catalog",
            })?;
        let selection = catalog
            .interactive_selection(method_id)
            .map_err(GrokAuthAttemptError::InteractiveSelection)?;

        let mut meta = serde_json::Map::new();
        meta.insert(
            "use_oauth".to_string(),
            serde_json::Value::Bool(options.force_loopback()),
        );
        if options.force_interactive() {
            meta.insert(
                "force_interactive".to_string(),
                serde_json::Value::Bool(true),
            );
        }
        let authenticate = GrokAuthRpc::new(
            GROK_AUTHENTICATE_METHOD,
            serde_json::json!({
                "methodId": selection.method_id(),
                "_meta": serde_json::Value::Object(meta)
            }),
            true,
        );
        let get_url = Self::get_url_rpc();

        self.selected_method_id = Some(selection.method_id().to_string());
        self.explicitly_confirmed = true;
        self.phase = GrokAuthAttemptPhase::AwaitingInteractiveChallenge;
        self.auth_url_requests_started = 1;
        self.outbound_rpc_count += 2;
        self.browser_capable_rpc_count += 1;
        Ok(GrokAuthAction::StartInteractive {
            authenticate,
            get_url,
        })
    }

    /// Consume one source-shaped `x.ai/auth/get_url` result.
    pub fn observed_auth_url(
        &mut self,
        attempt_id: SessionOpenId,
        response: &serde_json::Value,
        now: Instant,
    ) -> Result<GrokAuthAction, GrokAuthAttemptError> {
        if let Some(action) = self.preflight(attempt_id, now) {
            return Ok(action);
        }
        if matches!(self.phase, GrokAuthAttemptPhase::Challenge(_)) {
            return Ok(GrokAuthAction::Ignored(
                GrokAuthIgnoredEvent::SupersededUrlResponse { attempt_id },
            ));
        }
        self.require_phase(
            GrokAuthAttemptPhase::AwaitingInteractiveChallenge,
            "observe authentication URL",
        )?;

        match crate::session_bootstrap::parse_grok_auth_url_poll(response) {
            Err(error) => Ok(self.abort(GrokAuthStopReason::AuthUrlContract(error))),
            Ok(AuthUrlPoll::Pending) => {
                if self.auth_url_requests_started >= GROK_AUTH_URL_POLL_ATTEMPTS {
                    return Ok(self.abort(GrokAuthStopReason::AuthUrlPollExhausted));
                }
                self.auth_url_requests_started += 1;
                self.outbound_rpc_count += 1;
                Ok(GrokAuthAction::PollAuthUrlAfter {
                    delay: GROK_AUTH_URL_POLL_INTERVAL,
                    request: Self::get_url_rpc(),
                })
            }
            Ok(AuthUrlPoll::Ready(details)) => {
                let mode = details.mode();
                let method_id = self.selected_method_id.clone().ok_or(
                    GrokAuthAttemptError::InvalidTransition {
                        phase: self.phase,
                        operation: "present challenge without a selected method",
                    },
                )?;
                self.phase = GrokAuthAttemptPhase::Challenge(mode);
                Ok(GrokAuthAction::PresentChallenge(Box::new(AuthChallenge {
                    attempt_id: self.attempt_id,
                    method_id,
                    details,
                    deadline: self.deadline,
                })))
            }
        }
    }

    /// Build a sensitive submit-code action only for an active loopback challenge.
    pub fn submit_code(
        &mut self,
        attempt_id: SessionOpenId,
        code: SensitiveText,
        now: Instant,
    ) -> Result<GrokAuthAction, GrokAuthAttemptError> {
        if let Some(action) = self.preflight(attempt_id, now) {
            return Ok(action);
        }
        if self.phase != GrokAuthAttemptPhase::Challenge(AuthMode::Loopback) {
            return Err(GrokAuthAttemptError::CodeNotAccepted);
        }
        let params =
            SensitiveAuthCodeParams::new(code).map_err(GrokAuthAttemptError::InvalidCode)?;
        self.outbound_rpc_count += 1;
        Ok(GrokAuthAction::SendSensitiveRpc(
            GrokSensitiveAuthRpc::submit_code(params),
        ))
    }

    /// Settle successfully after the active authenticate RPC completes.
    pub fn authentication_succeeded(
        &mut self,
        attempt_id: SessionOpenId,
        now: Instant,
    ) -> Result<GrokAuthAction, GrokAuthAttemptError> {
        if let Some(action) = self.preflight(attempt_id, now) {
            return Ok(action);
        }
        if !matches!(
            self.phase,
            GrokAuthAttemptPhase::AwaitingApiKeyAuthentication
                | GrokAuthAttemptPhase::AwaitingInteractiveChallenge
                | GrokAuthAttemptPhase::Challenge(_)
        ) {
            return Err(GrokAuthAttemptError::InvalidTransition {
                phase: self.phase,
                operation: "complete authentication",
            });
        }
        Ok(self.ready_without_rpc())
    }

    /// Fail and reap after the active authenticate RPC rejects or errors.
    pub fn authentication_failed(
        &mut self,
        attempt_id: SessionOpenId,
        now: Instant,
    ) -> Result<GrokAuthAction, GrokAuthAttemptError> {
        if let Some(action) = self.preflight(attempt_id, now) {
            return Ok(action);
        }
        if !matches!(
            self.phase,
            GrokAuthAttemptPhase::AwaitingApiKeyAuthentication
                | GrokAuthAttemptPhase::AwaitingInteractiveChallenge
                | GrokAuthAttemptPhase::Challenge(_)
        ) {
            return Err(GrokAuthAttemptError::InvalidTransition {
                phase: self.phase,
                operation: "fail authentication",
            });
        }
        Ok(self.abort(GrokAuthStopReason::AuthenticationFailed))
    }

    /// Cancel the active attempt and require process-tree reaping.
    pub fn cancel(&mut self, attempt_id: SessionOpenId, now: Instant) -> GrokAuthAction {
        if let Some(action) = self.preflight(attempt_id, now) {
            return action;
        }
        self.abort(GrokAuthStopReason::Cancelled)
    }

    /// Apply the deadline even when no protocol event arrives.
    pub fn check_timeout(&mut self, now: Instant) -> Option<GrokAuthAction> {
        if self.phase.is_terminal() || now < self.deadline {
            return None;
        }
        Some(self.abort(GrokAuthStopReason::TimedOut))
    }

    /// Fail and reap when the ACP transport closes before settlement.
    pub fn transport_closed(&mut self, attempt_id: SessionOpenId, now: Instant) -> GrokAuthAction {
        if let Some(action) = self.preflight(attempt_id, now) {
            return action;
        }
        self.abort(GrokAuthStopReason::TransportClosed)
    }

    /// Fail and reap when the interactive event receiver disappears.
    pub fn event_receiver_closed(
        &mut self,
        attempt_id: SessionOpenId,
        now: Instant,
    ) -> GrokAuthAction {
        if let Some(action) = self.preflight(attempt_id, now) {
            return action;
        }
        self.abort(GrokAuthStopReason::EventReceiverClosed)
    }

    fn preflight(&mut self, received: SessionOpenId, now: Instant) -> Option<GrokAuthAction> {
        if received != self.attempt_id {
            return Some(GrokAuthAction::Ignored(
                GrokAuthIgnoredEvent::StaleGeneration {
                    active: self.attempt_id,
                    received,
                },
            ));
        }
        if self.phase.is_terminal() {
            return Some(GrokAuthAction::Ignored(
                GrokAuthIgnoredEvent::LateAfterSettlement {
                    attempt_id: self.attempt_id,
                    phase: self.phase,
                },
            ));
        }
        (now >= self.deadline).then(|| self.abort(GrokAuthStopReason::TimedOut))
    }

    fn require_phase(
        &self,
        expected: GrokAuthAttemptPhase,
        operation: &'static str,
    ) -> Result<(), GrokAuthAttemptError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(GrokAuthAttemptError::InvalidTransition {
                phase: self.phase,
                operation,
            })
        }
    }

    fn ready_without_rpc(&mut self) -> GrokAuthAction {
        self.phase = GrokAuthAttemptPhase::Authenticated;
        GrokAuthAction::SessionReady(AuthSettled {
            attempt_id: self.attempt_id,
            outcome: AuthOutcome::Authenticated,
        })
    }

    fn abort(&mut self, reason: GrokAuthStopReason) -> GrokAuthAction {
        let outcome = match reason {
            GrokAuthStopReason::Cancelled => {
                self.phase = GrokAuthAttemptPhase::Cancelled;
                AuthOutcome::Cancelled
            }
            GrokAuthStopReason::TimedOut => {
                self.phase = GrokAuthAttemptPhase::TimedOut;
                AuthOutcome::Failed
            }
            GrokAuthStopReason::InitializeContract(_)
            | GrokAuthStopReason::AuthUrlContract(_)
            | GrokAuthStopReason::AuthUrlPollExhausted
            | GrokAuthStopReason::AuthenticationFailed
            | GrokAuthStopReason::TransportClosed
            | GrokAuthStopReason::EventReceiverClosed => {
                self.phase = GrokAuthAttemptPhase::Failed;
                AuthOutcome::Failed
            }
        };
        GrokAuthAction::AbortAndReap {
            settled: AuthSettled {
                attempt_id: self.attempt_id,
                outcome,
            },
            reason,
        }
    }

    fn get_url_rpc() -> GrokAuthRpc {
        GrokAuthRpc::new(GROK_AUTH_GET_URL_METHOD, serde_json::json!({}), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> (Instant, Instant) {
        let now = Instant::now();
        (now, now + Duration::from_secs(600))
    }

    fn interactive_initialize() -> serde_json::Value {
        serde_json::json!({
            "authMethods": [
                { "id": "grok.com", "name": "Grok" },
                { "id": "oidc", "name": "Enterprise SSO" }
            ],
            "_meta": { "defaultAuthMethodId": null }
        })
    }

    fn confirmed_attempt(mode: &str, auth_url: &str) -> (GrokAuthAttempt, Instant, GrokAuthAction) {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(41);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let _offer = attempt
            .initialized(id, &interactive_initialize(), now)
            .unwrap();
        let _start = attempt
            .confirm_interactive(
                id,
                "grok.com",
                GrokInteractiveAuthOptions::initial(false),
                now,
            )
            .unwrap();
        let action = attempt
            .observed_auth_url(
                id,
                &serde_json::json!({
                    "auth_url": auth_url,
                    "external_provider": mode == "command",
                    "mode": mode
                }),
                now,
            )
            .unwrap();
        (attempt, now, action)
    }

    #[test]
    fn cached_default_is_ready_with_zero_authenticate_rpcs() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(1);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let action = attempt
            .initialized(
                id,
                &serde_json::json!({
                    "authMethods": [
                        { "id": "xai.api_key", "name": "API key" },
                        { "id": "cached_token", "name": "Cached token" },
                        { "id": "grok.com", "name": "Grok" }
                    ],
                    "_meta": { "defaultAuthMethodId": "cached_token" }
                }),
                now,
            )
            .unwrap();

        assert!(matches!(action, GrokAuthAction::SessionReady(_)));
        assert_eq!(attempt.phase(), GrokAuthAttemptPhase::Authenticated);
        assert_eq!(attempt.outbound_rpc_count(), 0);
        assert_eq!(attempt.browser_capable_rpc_count(), 0);
    }

    #[test]
    fn api_key_uses_one_exact_headless_non_browser_request() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(2);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let action = attempt
            .initialized(
                id,
                &serde_json::json!({
                    "authMethods": [
                        { "id": "xai.api_key", "name": "API key" },
                        { "id": "grok.com", "name": "Grok" }
                    ],
                    "_meta": { "defaultAuthMethodId": "xai.api_key" }
                }),
                now,
            )
            .unwrap();
        let GrokAuthAction::SendRpc(request) = action else {
            panic!("expected API-key authenticate");
        };

        assert_eq!(request.method(), "authenticate");
        assert_eq!(
            request.params(),
            &serde_json::json!({
                "methodId": "xai.api_key",
                "_meta": { "headless": true }
            })
        );
        assert!(!request.may_open_browser());
        assert_eq!(attempt.outbound_rpc_count(), 1);
        assert_eq!(attempt.browser_capable_rpc_count(), 0);
        assert!(matches!(
            attempt.authentication_succeeded(id, now).unwrap(),
            GrokAuthAction::SessionReady(_)
        ));
    }

    #[test]
    fn interactive_path_emits_no_rpc_until_explicit_confirmation() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(3);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let action = attempt
            .initialized(id, &interactive_initialize(), now)
            .unwrap();

        assert!(matches!(action, GrokAuthAction::PresentOffer(_)));
        assert!(!attempt.explicitly_confirmed());
        assert_eq!(attempt.outbound_rpc_count(), 0);
        assert_eq!(attempt.browser_capable_rpc_count(), 0);
        assert!(matches!(
            attempt.observed_auth_url(
                id,
                &serde_json::json!({
                    "auth_url": null,
                    "external_provider": false,
                    "mode": null
                }),
                now
            ),
            Err(GrokAuthAttemptError::InvalidTransition { .. })
        ));
        assert_eq!(attempt.browser_capable_rpc_count(), 0);

        let action = attempt
            .confirm_interactive(
                id,
                "grok.com",
                GrokInteractiveAuthOptions::initial(false),
                now,
            )
            .unwrap();
        let GrokAuthAction::StartInteractive {
            authenticate,
            get_url,
        } = action
        else {
            panic!("expected concurrent interactive start");
        };
        assert_eq!(
            authenticate.params(),
            &serde_json::json!({
                "methodId": "grok.com",
                "_meta": { "use_oauth": false }
            })
        );
        assert!(authenticate.may_open_browser());
        assert_eq!(get_url.method(), "x.ai/auth/get_url");
        assert_eq!(get_url.params(), &serde_json::json!({}));
        assert!(!get_url.may_open_browser());
        assert!(attempt.explicitly_confirmed());
        assert_eq!(attempt.outbound_rpc_count(), 2);
        assert_eq!(attempt.browser_capable_rpc_count(), 1);
    }

    #[test]
    fn account_switch_uses_force_interactive_without_reauth_or_headless() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(4);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let _ = attempt
            .initialized(id, &interactive_initialize(), now)
            .unwrap();
        let action = attempt
            .confirm_interactive(
                id,
                "oidc",
                GrokInteractiveAuthOptions::account_switch(true),
                now,
            )
            .unwrap();
        let GrokAuthAction::StartInteractive { authenticate, .. } = action else {
            panic!("expected interactive start");
        };
        assert_eq!(
            authenticate.params(),
            &serde_json::json!({
                "methodId": "oidc",
                "_meta": {
                    "use_oauth": true,
                    "force_interactive": true
                }
            })
        );
        assert!(authenticate.params().pointer("/_meta/reauth").is_none());
        assert!(authenticate.params().pointer("/_meta/headless").is_none());
    }

    #[test]
    fn non_exact_or_noninteractive_selection_never_starts_a_browser_rpc() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(5);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let _ = attempt
            .initialized(
                id,
                &serde_json::json!({
                    "authMethods": [
                        { "id": "cached_token", "name": "Cached token" },
                        { "id": "GROK.COM", "name": "Not exact" },
                        { "id": "grok.com", "name": "Grok" }
                    ],
                    "_meta": { "defaultAuthMethodId": "GROK.COM" }
                }),
                now,
            )
            .unwrap();

        // A valid cached credential wins before an unknown advertised default
        // only when the source did not select that unknown default. Here the
        // unknown default fails closed as an offer and cannot be confirmed.
        assert_ne!(
            attempt.phase(),
            GrokAuthAttemptPhase::Authenticated,
            "unknown default must not override the explicit source decision"
        );
        assert!(matches!(
            attempt.confirm_interactive(
                id,
                "GROK.COM",
                GrokInteractiveAuthOptions::initial(false),
                now
            ),
            Err(GrokAuthAttemptError::InteractiveSelection(
                GrokAuthContractError::MethodNotInteractive
            ))
        ));
        assert_eq!(attempt.browser_capable_rpc_count(), 0);
    }

    #[test]
    fn null_mode_retries_with_exact_empty_params_until_the_bound() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(6);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let _ = attempt
            .initialized(id, &interactive_initialize(), now)
            .unwrap();
        let _ = attempt
            .confirm_interactive(
                id,
                "grok.com",
                GrokInteractiveAuthOptions::initial(false),
                now,
            )
            .unwrap();
        let pending = serde_json::json!({
            "auth_url": null,
            "external_provider": false,
            "mode": null
        });

        for request_number in 2..=GROK_AUTH_URL_POLL_ATTEMPTS {
            let action = attempt.observed_auth_url(id, &pending, now).unwrap();
            let GrokAuthAction::PollAuthUrlAfter { delay, request } = action else {
                panic!("request {request_number} should be scheduled");
            };
            assert_eq!(delay, Duration::from_millis(50));
            assert_eq!(request.method(), "x.ai/auth/get_url");
            assert_eq!(request.params(), &serde_json::json!({}));
        }
        let exhausted = attempt.observed_auth_url(id, &pending, now).unwrap();
        assert!(matches!(
            exhausted,
            GrokAuthAction::AbortAndReap {
                reason: GrokAuthStopReason::AuthUrlPollExhausted,
                ..
            }
        ));
        assert_eq!(attempt.phase(), GrokAuthAttemptPhase::Failed);
        assert_eq!(
            attempt.auth_url_requests_started(),
            GROK_AUTH_URL_POLL_ATTEMPTS
        );
    }

    #[test]
    fn all_modes_produce_safe_mode_specific_challenges() {
        let (loopback, _, loopback_action) =
            confirmed_attempt("loopback", "http://127.0.0.1:43123/callback?state=private");
        let GrokAuthAction::PresentChallenge(loopback_challenge) = loopback_action else {
            panic!("expected loopback challenge");
        };
        assert_eq!(loopback_challenge.mode(), AuthMode::Loopback);
        assert!(loopback_challenge.accepts_manual_code());
        assert!(loopback_challenge.safe_url().is_some());
        assert_eq!(
            loopback.phase(),
            GrokAuthAttemptPhase::Challenge(AuthMode::Loopback)
        );

        let (_, _, device_action) = confirmed_attempt(
            "device",
            "https://accounts.x.ai/device?user_code=ABCD-EFGH&state=private",
        );
        let GrokAuthAction::PresentChallenge(device_challenge) = device_action else {
            panic!("expected device challenge");
        };
        assert_eq!(device_challenge.mode(), AuthMode::Device);
        assert!(!device_challenge.accepts_manual_code());
        assert_eq!(
            device_challenge.user_code().map(SensitiveText::reveal),
            Some("ABCD-EFGH")
        );

        let (_, _, command_action) = confirmed_attempt(
            "command",
            "Launching SSO...\nVisit https://idp.example/?state=private",
        );
        let GrokAuthAction::PresentChallenge(command_challenge) = command_action else {
            panic!("expected command challenge");
        };
        assert_eq!(command_challenge.mode(), AuthMode::Command);
        assert!(!command_challenge.can_open_or_copy_url());
        assert!(command_challenge.safe_url().is_none());
        assert!(command_challenge.command_status().is_some());
        assert!(!format!("{command_challenge:?}").contains("private"));
    }

    #[test]
    fn unsafe_challenge_fails_closed_and_requires_reaping() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(7);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let _ = attempt
            .initialized(id, &interactive_initialize(), now)
            .unwrap();
        let _ = attempt
            .confirm_interactive(
                id,
                "grok.com",
                GrokInteractiveAuthOptions::initial(false),
                now,
            )
            .unwrap();
        let action = attempt
            .observed_auth_url(
                id,
                &serde_json::json!({
                    "auth_url": "http://attacker.example/login",
                    "external_provider": false,
                    "mode": "loopback"
                }),
                now,
            )
            .unwrap();
        assert!(matches!(
            action,
            GrokAuthAction::AbortAndReap {
                reason: GrokAuthStopReason::AuthUrlContract(_),
                ..
            }
        ));
        assert_eq!(attempt.phase(), GrokAuthAttemptPhase::Failed);
    }

    #[test]
    fn code_submission_is_redacted_and_only_loopback_eligible() {
        let (mut loopback, now, _) =
            confirmed_attempt("loopback", "http://127.0.0.1:43123/callback?state=private");
        let id = loopback.attempt_id();
        let secret = "http://127.0.0.1/callback?code=one-time-secret";
        let action = loopback
            .submit_code(id, SensitiveText::new(secret), now)
            .unwrap();
        let GrokAuthAction::SendSensitiveRpc(request) = action else {
            panic!("expected submit-code request");
        };
        assert_eq!(request.method(), "x.ai/auth/submit_code");
        assert_eq!(
            request.reveal_params(),
            &serde_json::json!({ "code": secret })
        );
        assert!(!format!("{request:?}").contains("one-time-secret"));

        for (mode, value) in [
            ("device", "https://accounts.x.ai/device?user_code=ABCD-EFGH"),
            ("command", "Waiting for company SSO"),
        ] {
            let (mut attempt, now, _) = confirmed_attempt(mode, value);
            let id = attempt.attempt_id();
            assert!(matches!(
                attempt.submit_code(id, SensitiveText::new("valid-code"), now),
                Err(GrokAuthAttemptError::CodeNotAccepted)
            ));
        }
    }

    #[test]
    fn confirmed_open_revalidates_the_method_against_a_fresh_initialize() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(11);
        let mut removed = GrokAuthAttempt::new(id, deadline);
        assert!(matches!(
            removed.initialized_with_explicit_confirmation(
                id,
                &serde_json::json!({
                    "authMethods": [{ "id": "oidc", "name": "Enterprise SSO" }],
                    "_meta": { "defaultAuthMethodId": null }
                }),
                "grok.com",
                GrokInteractiveAuthOptions::initial(false),
                now,
            ),
            Err(GrokAuthAttemptError::InteractiveSelection(
                GrokAuthContractError::MethodNotAdvertised
            ))
        ));
        assert_eq!(removed.outbound_rpc_count(), 0);
        assert_eq!(removed.browser_capable_rpc_count(), 0);

        let mut present = GrokAuthAttempt::new(id, deadline);
        let action = present
            .initialized_with_explicit_confirmation(
                id,
                &interactive_initialize(),
                "grok.com",
                GrokInteractiveAuthOptions::initial(false),
                now,
            )
            .unwrap();
        assert!(matches!(action, GrokAuthAction::StartInteractive { .. }));
        assert!(present.explicitly_confirmed());
        assert_eq!(present.browser_capable_rpc_count(), 1);
    }

    #[test]
    fn stale_generation_and_late_events_cannot_change_the_active_outcome() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(8);
        let stale = SessionOpenId::new(7);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let stale_action = attempt
            .initialized(stale, &interactive_initialize(), now)
            .unwrap();
        assert!(matches!(
            stale_action,
            GrokAuthAction::Ignored(GrokAuthIgnoredEvent::StaleGeneration { .. })
        ));
        assert_eq!(attempt.phase(), GrokAuthAttemptPhase::AwaitingInitialize);
        assert_eq!(attempt.outbound_rpc_count(), 0);

        let _ = attempt
            .initialized(id, &interactive_initialize(), now)
            .unwrap();
        let cancelled = attempt.cancel(id, now);
        assert!(matches!(
            cancelled,
            GrokAuthAction::AbortAndReap {
                reason: GrokAuthStopReason::Cancelled,
                ..
            }
        ));
        let late = attempt.authentication_succeeded(id, now).unwrap();
        assert!(matches!(
            late,
            GrokAuthAction::Ignored(GrokAuthIgnoredEvent::LateAfterSettlement {
                phase: GrokAuthAttemptPhase::Cancelled,
                ..
            })
        ));
        assert_eq!(attempt.phase(), GrokAuthAttemptPhase::Cancelled);
    }

    #[test]
    fn deadline_receiver_close_and_transport_close_all_fail_closed() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(9);
        let mut timed_out = GrokAuthAttempt::new(id, deadline);
        let _ = timed_out
            .initialized(id, &interactive_initialize(), now)
            .unwrap();
        let timeout = timed_out.check_timeout(deadline).unwrap();
        assert!(matches!(
            timeout,
            GrokAuthAction::AbortAndReap {
                reason: GrokAuthStopReason::TimedOut,
                ..
            }
        ));
        assert_eq!(timed_out.phase(), GrokAuthAttemptPhase::TimedOut);

        let mut receiver_closed = GrokAuthAttempt::new(id, deadline);
        let _ = receiver_closed
            .initialized(id, &interactive_initialize(), now)
            .unwrap();
        assert!(matches!(
            receiver_closed.event_receiver_closed(id, now),
            GrokAuthAction::AbortAndReap {
                reason: GrokAuthStopReason::EventReceiverClosed,
                ..
            }
        ));

        let mut transport_closed = GrokAuthAttempt::new(id, deadline);
        let _ = transport_closed
            .initialized(id, &interactive_initialize(), now)
            .unwrap();
        assert!(matches!(
            transport_closed.transport_closed(id, now),
            GrokAuthAction::AbortAndReap {
                reason: GrokAuthStopReason::TransportClosed,
                ..
            }
        ));
    }

    #[test]
    fn initialize_contract_failure_never_emits_a_protocol_rpc() {
        let (now, deadline) = clock();
        let id = SessionOpenId::new(10);
        let mut attempt = GrokAuthAttempt::new(id, deadline);
        let action = attempt
            .initialized(id, &serde_json::json!({ "authMethods": "bad" }), now)
            .unwrap();
        assert!(matches!(
            action,
            GrokAuthAction::AbortAndReap {
                reason: GrokAuthStopReason::InitializeContract(_),
                ..
            }
        ));
        assert_eq!(attempt.phase(), GrokAuthAttemptPhase::Failed);
        assert_eq!(attempt.outbound_rpc_count(), 0);
        assert_eq!(attempt.browser_capable_rpc_count(), 0);
    }
}
