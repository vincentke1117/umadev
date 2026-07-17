//! Outstanding background sub-agent tracking — the "don't settle while your own
//! agents are still working" guard.
//!
//! **The reported failure:** the base dispatches N of its OWN background
//! sub-agents (claude's `Agent`/`Task` tool with `run_in_background`), then ends
//! its turn — either after writing a premature "final report", or with no report
//! at all — while those agents are still running. UmaDev treats the base's
//! turn-end as completion, settles, and (on run end) tears the session down,
//! killing the outstanding agents mid-write: their files never land and the
//! honesty guard prints "claimed changes but the tree is unchanged".
//!
//! **What is observable** (claude 2.1.x stream-json, surfaced by the driver as
//! [`SessionEvent::BackgroundTask`]):
//! - `system/task_started` — a background sub-agent launched (edge);
//! - `system/task_notification` — a task reached a terminal state (edge);
//! - `system/background_tasks_changed` — the FULL live set (level; claude's own
//!   contract says consumers should REPLACE their set with each payload so a
//!   missed edge can never wedge a stale count).
//!
//! Plus, as a version-proof FALLBACK when no such frame ever arrives, the
//! launch placeholder the `Agent`/`Task` tool returns immediately for a
//! background spawn — a [`SessionEvent::ToolResult`] whose summary starts with
//! `"Async agent launched successfully"` — and the collection acknowledgement a
//! `TaskOutput` call returns (`<retrieval_status>success` — only ever returned
//! for a TERMINAL task).
//!
//! [`BgAgentTracker`] fuses these into one `outstanding()` count; the turn
//! pumps consult it at `TurnDone{Completed}` and, instead of settling, re-drive
//! the base with a bounded "wait for your agents, collect their results, THEN
//! report" directive ([`BgAgentTracker::wait_directive`]) — at most
//! [`MAX_BG_REDRIVES`] times per turn. If known agents are still live after the
//! bound, the caller must settle the turn as incomplete/failed rather than
//! publishing a false success. **Fail-open by contract:** a base that emits no
//! lifecycle signal keeps a zero count and today's behavior; a positive live set
//! is evidence and may never be silently discarded.

use std::collections::BTreeSet;

use umadev_runtime::{BackgroundTaskSignal, SessionEvent};

/// Base-native child work observed during one logical turn.
///
/// Raw vendor task identifiers are kept only in memory. Callers that persist
/// this observation must derive opaque hashes instead of writing the identifiers
/// themselves: a vendor is free to put account or session material in an id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseAgentObservation {
    agent_ids: BTreeSet<String>,
    anonymous_count: u32,
}

impl BaseAgentObservation {
    /// Merge another turn's observations without double-counting stable ids.
    pub fn merge(&mut self, other: Self) {
        self.agent_ids.extend(other.agent_ids);
        self.anonymous_count = self.anonymous_count.saturating_add(other.anonymous_count);
    }

    /// Stable vendor ids observed through structured lifecycle frames.
    pub(crate) fn agent_ids(&self) -> impl Iterator<Item = &str> {
        self.agent_ids.iter().map(String::as_str)
    }

    /// Launches known only through a count-bearing fallback marker.
    #[must_use]
    pub(crate) fn anonymous_count(&self) -> u32 {
        self.anonymous_count
    }

    /// Whether no base-native child work was observed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agent_ids.is_empty() && self.anonymous_count == 0
    }

    /// Total observed child count, saturating at the platform's `usize` range.
    #[must_use]
    pub fn len(&self) -> usize {
        self.agent_ids
            .len()
            .saturating_add(usize::try_from(self.anonymous_count).unwrap_or(usize::MAX))
    }
}

/// Maximum "wait for your background agents" re-drives per logical turn. The
/// hard bound keeps the settle path terminating; exhausting it turns the
/// logical turn into an incomplete result, never a successful settle.
pub const MAX_BG_REDRIVES: u8 = 2;

/// The immediate tool_result placeholder claude returns when the `Agent`/`Task`
/// tool spawns a BACKGROUND sub-agent (ground truth, claude 2.1.x: `"Async
/// agent launched successfully. (This tool result is internal metadata — never
/// quote or paste …)"`). Matching the head keeps working under the driver's
/// 200-char summary clip.
const ASYNC_LAUNCH_MARKER: &str = "Async agent launched successfully";

/// The `TaskOutput` tool's collection acknowledgement head — claude only ever
/// returns `retrieval_status: success` for a task in a TERMINAL state, so one
/// occurrence means one background task's result was actually collected.
const COLLECTED_MARKER: &str = "<retrieval_status>success";

/// Tracks how many of the base's OWN background sub-agents are outstanding
/// (launched but not yet finished/collected) across one logical turn — the
/// turn pumps feed it every [`SessionEvent`] and consult
/// [`outstanding`](Self::outstanding) at the settle boundary.
///
/// Two channels, most-precise wins:
/// - **frames** (id-based, from [`SessionEvent::BackgroundTask`]): live set of
///   agent task ids, maintained edge+level;
/// - **markers** (count-based, from tool_result text): launch placeholders
///   minus collected results — used ONLY while no frame has been seen (an
///   older base that emits no task frames).
///
/// Deterministic and fail-open: unknown events are ignored; counts saturate.
#[derive(Debug, Default)]
pub struct BgAgentTracker {
    /// Live background sub-agent task ids (the frame channel).
    live: BTreeSet<String>,
    /// Every structured id seen during the turn, including already-finished
    /// children. `live` alone is empty at the successful settle boundary and
    /// therefore cannot support durable task history.
    seen: BTreeSet<String>,
    /// Whether ANY task frame was seen — once true, the frame channel is
    /// authoritative and the marker channel is ignored (avoids double count).
    saw_frames: bool,
    /// Marker channel: background-agent launch placeholders observed.
    marker_launched: u32,
    /// Marker channel: terminal `TaskOutput` collections observed.
    marker_collected: u32,
    /// "Wait for your agents" re-drives already spent this turn.
    redrives: u8,
}

impl BgAgentTracker {
    /// Fresh tracker for one logical turn.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one session event. Cheap, never fails, never panics.
    pub fn observe(&mut self, ev: &SessionEvent) {
        match ev {
            SessionEvent::BackgroundTask(signal) => {
                self.saw_frames = true;
                match signal {
                    BackgroundTaskSignal::Started { id } => {
                        self.live.insert(id.clone());
                        self.seen.insert(id.clone());
                    }
                    BackgroundTaskSignal::Finished { id } => {
                        self.live.remove(id);
                        self.seen.insert(id.clone());
                    }
                    BackgroundTaskSignal::Live { agent_ids } => {
                        // The LEVEL signal REPLACES the set (claude's own
                        // contract) — a missed edge can never wedge a stale id.
                        self.seen.extend(agent_ids.iter().cloned());
                        self.live = agent_ids.iter().cloned().collect();
                    }
                }
            }
            SessionEvent::ToolResult { ok: true, summary }
            | SessionEvent::ToolResultCorrelated {
                ok: true, summary, ..
            } => {
                // The launch placeholder / the collection ack. `contains` (not
                // starts_with): a nested sub-agent's result row carries a
                // visual attribution prefix.
                if summary.contains(ASYNC_LAUNCH_MARKER) {
                    self.marker_launched = self.marker_launched.saturating_add(1);
                } else if summary.contains(COLLECTED_MARKER) {
                    self.marker_collected = self.marker_collected.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    /// How many background sub-agents are outstanding right now. Zero for any
    /// base that surfaced no background-agent signal (the fail-open floor).
    #[must_use]
    pub fn outstanding(&self) -> usize {
        if self.saw_frames {
            self.live.len()
        } else {
            self.marker_launched.saturating_sub(self.marker_collected) as usize
        }
    }

    /// Snapshot every base-native child observed during this turn.
    ///
    /// Structured ids win when present. Marker-only launches have no id, so the
    /// unmatched remainder is represented as an anonymous count. This preserves
    /// honest cardinality without inventing vendor identities.
    #[must_use]
    pub fn observation(&self) -> BaseAgentObservation {
        let seen_count = u32::try_from(self.seen.len()).unwrap_or(u32::MAX);
        let anonymous_count = self.marker_launched.saturating_sub(seen_count);
        BaseAgentObservation {
            agent_ids: self.seen.clone(),
            anonymous_count,
        }
    }

    /// Whether the settle should be converted into ONE more "wait for your
    /// agents" re-drive: outstanding work exists AND the per-turn bound
    /// ([`MAX_BG_REDRIVES`]) is not exhausted. Consumes one re-drive credit
    /// when it returns `true` — call it at most once per settle attempt.
    #[must_use]
    pub fn begin_redrive(&mut self) -> bool {
        if self.outstanding() == 0 || self.redrives >= MAX_BG_REDRIVES {
            return false;
        }
        self.redrives = self.redrives.saturating_add(1);
        true
    }

    /// Re-drives spent so far (for the user-facing "attempt i/N" note).
    #[must_use]
    pub fn redrives(&self) -> u8 {
        self.redrives
    }

    /// The bounded corrective directive for a base that ended its turn with
    /// `n` background sub-agents still running. Imperative and diagnosed (the
    /// director's rework style): wait, collect, fold in, THEN report.
    #[must_use]
    pub fn wait_directive(&self) -> String {
        let n = self.outstanding();
        format!(
            "REALITY CHECK: you ended your turn while {n} of your own background \
             sub-agent(s) are still running — their results were never collected, so \
             any \"final report\" you wrote is premature and the work is NOT done. Do \
             this now, in this turn: (1) use this base's native blocking wait/inspect \
             mechanism for each outstanding background agent and WAIT for it to \
             finish; (2) collect every \
             result and fold it into the actual work products on disk; (3) only after \
             ALL background agents are resolved, write the real final report. If an \
             agent is stuck or no longer needed, say so explicitly and stop it. Never \
             conclude or report completion while your own background agents are \
             outstanding."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(id: &str) -> SessionEvent {
        SessionEvent::BackgroundTask(BackgroundTaskSignal::Started { id: id.to_string() })
    }

    fn finished(id: &str) -> SessionEvent {
        SessionEvent::BackgroundTask(BackgroundTaskSignal::Finished { id: id.to_string() })
    }

    fn live(ids: &[&str]) -> SessionEvent {
        SessionEvent::BackgroundTask(BackgroundTaskSignal::Live {
            agent_ids: ids.iter().map(ToString::to_string).collect(),
        })
    }

    fn tool_result(ok: bool, summary: &str) -> SessionEvent {
        SessionEvent::ToolResult {
            ok,
            summary: summary.to_string(),
        }
    }

    #[test]
    fn frame_channel_tracks_edges_and_level_replaces() {
        let mut t = BgAgentTracker::new();
        assert_eq!(t.outstanding(), 0, "fresh tracker is zero (fail-open)");
        t.observe(&started("a1"));
        t.observe(&started("a2"));
        assert_eq!(t.outstanding(), 2);
        t.observe(&finished("a1"));
        assert_eq!(t.outstanding(), 1);
        // The level signal REPLACES the set — a missed Finished edge for `a2`
        // cannot wedge a stale count once the level says only `a9` is live.
        t.observe(&live(&["a9"]));
        assert_eq!(t.outstanding(), 1);
        t.observe(&live(&[]));
        assert_eq!(t.outstanding(), 0);
        // Duplicate edges are idempotent (a Started after Live re-adds is fine).
        t.observe(&started("a9"));
        t.observe(&started("a9"));
        assert_eq!(t.outstanding(), 1);
    }

    #[test]
    fn marker_channel_counts_launches_minus_collections_until_frames_arrive() {
        let mut t = BgAgentTracker::new();
        // Two background launches observed only via the placeholder text.
        t.observe(&tool_result(
            true,
            "Async agent launched successfully. (This tool result is internal metadata …)",
        ));
        t.observe(&tool_result(
            true,
            "↳ 子代理 · Async agent launched successfully. …",
        ));
        assert_eq!(t.outstanding(), 2);
        // One terminal collection via TaskOutput.
        t.observe(&tool_result(
            true,
            "<retrieval_status>success</retrieval_status><task_id>a1</task_id>",
        ));
        assert_eq!(t.outstanding(), 1);
        // A failed tool_result never counts (fail-open).
        t.observe(&tool_result(false, "Async agent launched successfully."));
        assert_eq!(t.outstanding(), 1);
        // Once ANY frame arrives, the frame channel is authoritative — no
        // double counting between channels.
        t.observe(&live(&[]));
        assert_eq!(t.outstanding(), 0);
    }

    #[test]
    fn redrive_is_bounded_and_only_fires_with_outstanding_work() {
        let mut t = BgAgentTracker::new();
        // Nothing outstanding → never a re-drive.
        assert!(!t.begin_redrive());
        t.observe(&started("a1"));
        assert!(t.begin_redrive(), "first re-drive");
        assert_eq!(t.redrives(), 1);
        assert!(t.begin_redrive(), "second re-drive");
        assert!(
            !t.begin_redrive(),
            "the bound: at most {MAX_BG_REDRIVES} re-drives per turn"
        );
        assert_eq!(t.redrives(), MAX_BG_REDRIVES);
        // Once the agent resolves, outstanding drops to zero → no re-drive
        // even with credits left.
        let mut t2 = BgAgentTracker::new();
        t2.observe(&started("a1"));
        t2.observe(&finished("a1"));
        assert!(!t2.begin_redrive());
    }

    #[test]
    fn wait_directive_names_the_count_and_the_discipline() {
        let mut t = BgAgentTracker::new();
        t.observe(&started("a1"));
        t.observe(&started("a2"));
        t.observe(&started("a3"));
        let d = t.wait_directive();
        assert!(d.contains('3'), "names the outstanding count: {d}");
        assert!(
            d.contains("native blocking wait/inspect mechanism"),
            "requires the base-native blocking collection mechanism: {d}"
        );
        assert!(
            d.contains("Never conclude or report completion"),
            "carries the discipline: {d}"
        );
    }

    #[test]
    fn observation_keeps_finished_ids_without_persisting_only_the_live_set() {
        let mut tracker = BgAgentTracker::new();
        tracker.observe(&started("child-account-shaped-id"));
        tracker.observe(&finished("child-account-shaped-id"));

        assert_eq!(tracker.outstanding(), 0);
        let observed = tracker.observation();
        assert_eq!(
            observed.agent_ids().collect::<Vec<_>>(),
            ["child-account-shaped-id"]
        );
        assert_eq!(observed.anonymous_count(), 0);
    }

    #[test]
    fn observation_preserves_marker_only_cardinality_and_merges_turns() {
        let mut first = BgAgentTracker::new();
        first.observe(&tool_result(true, ASYNC_LAUNCH_MARKER));
        first.observe(&tool_result(true, ASYNC_LAUNCH_MARKER));
        let mut observed = first.observation();

        let mut second = BgAgentTracker::new();
        second.observe(&started("structured-child"));
        second.observe(&finished("structured-child"));
        observed.merge(second.observation());

        assert_eq!(observed.len(), 3);
        assert_eq!(observed.anonymous_count(), 2);
        assert_eq!(
            observed.agent_ids().collect::<Vec<_>>(),
            ["structured-child"]
        );
    }
}
