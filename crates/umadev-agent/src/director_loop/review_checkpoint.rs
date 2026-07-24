use super::{
    plan_state, Arc, EngineEvent, EventSink, Plan, RoutePlan, StepStatus,
    FINAL_REVIEW_RETRY_STEP_ID,
};

pub(super) fn final_review_qc_receipt_matches(
    project_root: &std::path::Path,
    recorded: Option<&str>,
) -> bool {
    let Some(recorded) = recorded.filter(|value| !value.trim().is_empty()) else {
        return false;
    };
    crate::freshness::workspace_qc_fingerprint(project_root).as_deref() == Some(recorded)
}

/// Ensure a final-gate outage has a concrete resumable plan boundary.
///
/// Most deliberate plans already contain a review step. A fail-open single-turn
/// build may not have a plan at all (for example, plan synthesis itself could not
/// fork). In that case create one host-owned review checkpoint rather than turning
/// a temporary reviewer outage into a terminal failure. The step is only a resume
/// cursor: the typed [`OperationalReviewCheckpoint::FinalGateReview`] makes resume
/// jump directly to the final gate, so it is never reviewed once as a step and then
/// reviewed a second time immediately afterwards.
pub(super) fn ensure_final_review_retry_step(
    plan: &mut Option<Plan>,
    route: Option<&RoutePlan>,
    events: &Arc<dyn EventSink>,
) {
    if plan.is_none() {
        *plan = Some(Plan {
            steps: Vec::new(),
            risks: Vec::new(),
            open_questions: Vec::new(),
        });
    }
    if let Some(plan) = plan.as_mut() {
        ensure_final_review_retry_step_in_plan(plan, route, events);
    }
}

pub(super) fn ensure_final_review_retry_step_in_plan(
    plan: &mut Plan,
    route: Option<&RoutePlan>,
    events: &Arc<dyn EventSink>,
) {
    let dependencies = plan
        .steps
        .iter()
        .filter(|step| step.kind != plan_state::StepKind::Review)
        .map(|step| step.id.clone())
        .collect::<Vec<_>>();
    if let Some(step) = plan
        .steps
        .iter_mut()
        .find(|step| step.id == FINAL_REVIEW_RETRY_STEP_ID)
    {
        step.depends_on = dependencies;
        if step.status != StepStatus::Pending {
            step.status = StepStatus::Pending;
            events.emit(EngineEvent::plan_step_status(
                step.id.clone(),
                step.title.clone(),
                StepStatus::Pending,
            ));
        }
        return;
    }

    let seat = route
        .and_then(|route| route.team.first().copied())
        .unwrap_or(crate::critics::Seat::QaEngineer);
    let step = plan_state::PlanStep {
        id: FINAL_REVIEW_RETRY_STEP_ID.to_string(),
        title: "Retry final whole-build review".to_string(),
        seat,
        kind: plan_state::StepKind::Review,
        // If a later workspace/doc edit reopens real work, this cursor is not
        // schedulable as an ordinary review until that work has settled.
        depends_on: dependencies,
        acceptance: plan_state::AcceptanceSpec::ReviewClean,
        evidence: Vec::new(),
        files: plan_state::StepFiles::default(),
        status: StepStatus::Pending,
    };
    events.emit(EngineEvent::plan_step_status(
        step.id.clone(),
        step.title.clone(),
        StepStatus::Pending,
    ));
    plan.steps.push(step);
}
