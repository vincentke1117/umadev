//! The chat-turn read-only execution gate.
//!
//! Decides whether a routed natural-language turn must run under the base's
//! read-only (`--permission-mode plan`) sandbox, or may run under the trust mode's
//! own permissions. Kept out of `lib.rs` so the hotspot file does not grow and so
//! the rule is unit-testable in isolation.

use umadev_agent::RouteSource;

/// Whether a routed turn must execute under the base's read-only plan sandbox.
///
/// A non-mutating route (`Chat` / `Explain`) is normally jailed read-only: the turn
/// reuses the sandboxed intent fork, or reopens a claude `--permission-mode plan`
/// session. That is correct when the read-only verdict is TRUSTWORTHY.
///
/// It is NOT trustworthy when it came from the DETERMINISTIC FALLBACK — the brain
/// intent consult was unavailable or TIMED OUT, so a bounded keyword classifier
/// guessed the class. When those keywords miss a real build request, the fallback
/// lands on a read-only class and jails the base in claude's native plan mode. The
/// base then drafts a plan, tries to call `ExitPlanMode` (a tool UmaDev does not
/// expose in its tool set), cannot, and re-plans forever — the reported deadlock,
/// which cannot be widened by approving in place because a running
/// `--permission-mode plan` session is fixed for its lifetime.
///
/// So a low-confidence fallback read-only verdict is NOT jailed unless the USER
/// explicitly asked to stay read-only (`demands_read_only`). This applies ONLY when
/// the trust mode would otherwise hand the base a WRITABLE session
/// (`mode_permits_execution` — Guarded / Auto, the two modes the deadlock was
/// reported in). Under the read-only trust mode (Plan) the base is read-only by the
/// user's own explicit choice, so the fallback guess cannot NEWLY trap it, and
/// keeping the jail lets the resident read-only session be reused instead of forcing
/// a needless reopen that would discard its primed context. Letting a Guarded / Auto
/// turn run un-jailed is safe: `Guarded` still approval-gates every write, `Auto` is
/// already pre-authorized, and `react_to_first_write` plus governance handle any
/// actual writes. A CONFIDENT brain verdict (the base agreed the turn is
/// chat/explain, so it will not try to act) and explicit user read-only wording both
/// KEEP the jail — neither can spring the plan-mode trap.
///
/// `non_mutating_route` is the historical read-only determination
/// (`!native_command && !route.class.mutates_workspace()`), computed at the call
/// site so native commands and mutating routes stay writable exactly as before.
#[must_use]
pub(crate) fn turn_executes_read_only(
    non_mutating_route: bool,
    route_source: Option<RouteSource>,
    demands_read_only: bool,
    mode_permits_execution: bool,
) -> bool {
    if !non_mutating_route {
        // A mutating route or a native command always runs writable / native.
        return false;
    }
    // The one behavior change vs. the historical `!native_command && !mutates`: under
    // an execution-capable trust mode (Guarded / Auto), an unreliable
    // deterministic-fallback read-only guess does not jail the base in plan mode
    // unless the user explicitly asked for read-only/observation. A brain verdict, a
    // `None`/unclassified source, or the read-only Plan mode all keep the
    // conservative jail.
    if mode_permits_execution
        && route_source == Some(RouteSource::DeterministicFallback)
        && !demands_read_only
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{turn_executes_read_only, RouteSource};

    #[test]
    fn deterministic_fallback_read_only_without_explicit_wording_is_not_jailed_under_execution_mode(
    ) {
        // The reported deadlock path under Guarded / Auto: the brain consult timed
        // out, the keyword fallback missed a real build and landed non-mutating. This
        // MUST NOT jail the base in claude's plan mode (the ExitPlanMode re-plan loop);
        // the execution-capable trust mode governs the turn instead.
        assert!(!turn_executes_read_only(
            true,
            Some(RouteSource::DeterministicFallback),
            false,
            true,
        ));
    }

    #[test]
    fn deterministic_fallback_read_only_stays_read_only_under_plan_mode() {
        // Plan trust mode is read-only by the user's explicit choice, so the same
        // fallback verdict keeps the jail — the resident read-only session is reused
        // rather than needlessly reopened, and the read-only guardrail holds.
        assert!(turn_executes_read_only(
            true,
            Some(RouteSource::DeterministicFallback),
            false,
            false,
        ));
    }

    #[test]
    fn deterministic_fallback_with_explicit_read_only_wording_stays_read_only() {
        // "只分析别改" is the user's own choice — the base will not try to act, so the
        // jail cannot trap it. Explicit read-only wording keeps read-only in every mode.
        assert!(turn_executes_read_only(
            true,
            Some(RouteSource::DeterministicFallback),
            true,
            true,
        ));
    }

    #[test]
    fn confident_brain_non_mutating_verdict_stays_read_only() {
        // A brain "this is chat/explain" verdict is reliable (the base agreed it will
        // not act) — commit 13676af9's path is untouched and still runs read-only even
        // under an execution-capable mode.
        assert!(turn_executes_read_only(
            true,
            Some(RouteSource::Brain),
            false,
            true,
        ));
    }

    #[test]
    fn mutating_route_is_never_read_only() {
        // A build (or any native command) has `non_mutating_route == false`, so it stays
        // writable regardless of provenance or mode.
        assert!(!turn_executes_read_only(
            false,
            Some(RouteSource::Brain),
            false,
            true,
        ));
        assert!(!turn_executes_read_only(
            false,
            Some(RouteSource::DeterministicFallback),
            false,
            true,
        ));
    }

    #[test]
    fn unclassified_source_keeps_the_conservative_read_only_jail() {
        // A non-mutating route with no recorded provenance is conservatively jailed —
        // only the specific deterministic-fallback guess is exempted.
        assert!(turn_executes_read_only(true, None, false, true));
    }
}
