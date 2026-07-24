//! Resident-TUI adapter for the canonical host-owned Git transaction.

use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;

use futures::FutureExt;
use umadev_agent::ChannelSink;

use crate::interaction_bridge::{
    await_user_approval, interactive_user_present, ApprovalHolder, ApprovalReply, HostInputHolder,
    PendingAskHolder,
};
use crate::session_slot::SessionHolder;
use crate::{App, CancelDrainOutcome, ChatSessionHolder, LiveInputHub, RouteDecision};

pub(super) fn replay_note(requirement: &str) -> Option<String> {
    umadev_agent::request_has_git_commit_operation(requirement)
        .then(|| umadev_i18n::tl("intent.git_commit_host_boundary").to_string())
}

/// Consume one Git operation before any resident base/session transport.
///
/// The caller has already identified a Git operation. This adapter validates
/// its shape, enforces the current-turn Guarded approval, then delegates the
/// actual mutation to the single canonical host transaction.
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    request: &str,
    mode: umadev_agent::TrustMode,
    interactive: bool,
    project_root: &Path,
    approval_holder: &ApprovalHolder,
    sink: &Arc<ChannelSink>,
    route_tx: &tokio::sync::mpsc::UnboundedSender<RouteDecision>,
) {
    let Some(host_request) = umadev_agent::parse_host_git_commit_request(request) else {
        settle(
            Err(umadev_i18n::tl("intent.git_commit_host_boundary").to_string()),
            route_tx,
        );
        return;
    };
    if mode == umadev_agent::TrustMode::Plan {
        settle(
            Err(umadev_i18n::tl("continuous.plan_mode_skip").to_string()),
            route_tx,
        );
        return;
    }
    if host_request.verifier.is_some() {
        settle(
            Err(umadev_i18n::tl("intent.git_commit_host_boundary").to_string()),
            route_tx,
        );
        return;
    }
    let commit_text = host_request.commit_text;
    let confirmed = umadev_agent::request_explicitly_confirms_git_commit(&commit_text);
    if mode == umadev_agent::TrustMode::Guarded && !confirmed {
        let action = "git commit";
        let target = project_root.display().to_string();
        let approved = interactive
            && matches!(
                await_user_approval(approval_holder, sink, action, &target).await,
                ApprovalReply::Allow
            );
        if !approved {
            settle(
                Err(umadev_i18n::tlf("trust.pause.denied", &[action, &target])),
                route_tx,
            );
            return;
        }
    }
    settle(
        crate::host_git::execute_host_git_commit(project_root, &commit_text).await,
        route_tx,
    );
}

/// Spawn one isolated host transaction without opening or consuming any
/// resident model/director session.
pub(super) fn spawn(
    app: &mut App,
    approval_holder: &ApprovalHolder,
    sink: &Arc<ChannelSink>,
    route_tx: &tokio::sync::mpsc::UnboundedSender<RouteDecision>,
    request: String,
) -> tokio::task::JoinHandle<()> {
    let mode = app.effective_trust_mode();
    let project_root = app.project_root.clone();
    let approval_holder = approval_holder.clone();
    let sink = sink.clone();
    let route_tx = route_tx.clone();
    let _ = app.take_route_input(&request);
    app.thinking = true;
    app.thinking_started = Some(std::time::Instant::now());
    app.last_output_at = None;
    app.tool_in_progress = false;
    app.agentic_in_flight = true;
    app.host_git_in_flight = true;
    tokio::spawn(async move {
        let terminal_tx = route_tx.clone();
        let outcome = AssertUnwindSafe(execute(
            &request,
            mode,
            interactive_user_present(),
            &project_root,
            &approval_holder,
            &sink,
            &route_tx,
        ))
        .catch_unwind()
        .await;
        if outcome.is_err() {
            let _ = terminal_tx.send(RouteDecision::HostGitDone {
                result: Err(umadev_i18n::tl("intent.git_commit_worker_failed").to_string()),
            });
        }
    })
}

/// At a Director pause, release only a front-most host commit from UmaDev's
/// local FIFO. Ordinary deferred chat remains parked and a later commit never
/// overtakes it.
pub(super) fn drain_front(
    app: &mut App,
    approval_holder: &ApprovalHolder,
    sink: &Arc<ChannelSink>,
    route_tx: &tokio::sync::mpsc::UnboundedSender<RouteDecision>,
) -> Option<tokio::task::JoinHandle<()>> {
    let text = app.take_front_host_git_commit()?;
    Some(spawn(app, approval_holder, sink, route_tx, text))
}

/// Gate-query terminal seam: an invalid/stale epoch drains nothing; a valid
/// terminal releases only a front-most host transaction.
pub(super) fn drain_after_gate_query(
    app: &mut App,
    accepted: bool,
    approval_holder: &ApprovalHolder,
    sink: &Arc<ChannelSink>,
    route_tx: &tokio::sync::mpsc::UnboundedSender<RouteDecision>,
) -> Option<tokio::task::JoinHandle<()>> {
    accepted
        .then(|| drain_front(app, approval_holder, sink, route_tx))
        .flatten()
}

/// Resume UmaDev's FIFO after a host transaction settles. A parked run releases
/// only another front-most host transaction; ordinary chat remains parked.
#[allow(clippy::too_many_arguments)]
pub(super) fn drain_after_settle(
    app: &mut App,
    chat_session: &ChatSessionHolder,
    director_session_holder: &SessionHolder,
    pending_ask: &PendingAskHolder,
    approval_holder: &ApprovalHolder,
    host_input_holder: &HostInputHolder,
    steer_holder: &umadev_agent::SteerIntake,
    live_input_hub: &LiveInputHub,
    sink: &Arc<ChannelSink>,
    route_tx: &tokio::sync::mpsc::UnboundedSender<RouteDecision>,
) -> Option<tokio::task::JoinHandle<()>> {
    if app.active_gate.is_some()
        || app.director_gate_paused
        || app.budget_paused
        || app.operational_pause_reason.is_some()
    {
        return drain_front(app, approval_holder, sink, route_tx);
    }
    super::drain_next_queued_chat(
        app,
        chat_session,
        director_session_holder,
        pending_ask,
        approval_holder,
        host_input_holder,
        steer_holder,
        live_input_hub,
        sink,
        route_tx,
    )
}

/// Apply the linearized result of an Esc/Ctrl-C drain for a host transaction.
///
/// A published result always wins over cancellation. A clean task completion
/// without an observed result keeps the UI waiting for its already-sent
/// `HostGitDone`; cancellation and panic settle locally through the same
/// non-destructive host recorder.
pub(super) fn settle_cancel(
    app: &mut App,
    outcome: CancelDrainOutcome,
    published: Option<Result<String, String>>,
) -> bool {
    if let Some(result) = published {
        app.record_host_git_done(result);
        return true;
    }
    match outcome {
        CancelDrainOutcome::Cancelled => app.record_host_git_cancelled(),
        CancelDrainOutcome::Panicked => app.record_host_git_done(Err(umadev_i18n::t(
            app.lang,
            "intent.git_commit_worker_failed",
        )
        .to_string())),
        CancelDrainOutcome::Finished => {
            app.cancelling = false;
            return false;
        }
        CancelDrainOutcome::TimedOut => return false,
    }
    true
}

/// Settle a host-only cancel drain and, when terminal, resume the permitted
/// portion of UmaDev's FIFO.
#[allow(clippy::too_many_arguments)]
pub(super) fn settle_cancel_and_drain(
    app: &mut App,
    outcome: CancelDrainOutcome,
    published: Option<Result<String, String>>,
    chat_session: &ChatSessionHolder,
    director_session_holder: &SessionHolder,
    pending_ask: &PendingAskHolder,
    approval_holder: &ApprovalHolder,
    host_input_holder: &HostInputHolder,
    steer_holder: &umadev_agent::SteerIntake,
    live_input_hub: &LiveInputHub,
    sink: &Arc<ChannelSink>,
    route_tx: &tokio::sync::mpsc::UnboundedSender<RouteDecision>,
) -> Option<tokio::task::JoinHandle<()>> {
    settle_cancel(app, outcome, published).then(|| {
        drain_after_settle(
            app,
            chat_session,
            director_session_holder,
            pending_ask,
            approval_holder,
            host_input_holder,
            steer_holder,
            live_input_hub,
            sink,
            route_tx,
        )
    })?
}

fn settle(
    result: Result<String, String>,
    route_tx: &tokio::sync::mpsc::UnboundedSender<RouteDecision>,
) {
    let _ = route_tx.send(RouteDecision::HostGitDone { result });
}
