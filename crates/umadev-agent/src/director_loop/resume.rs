//! Persisted director-loop resume state and document-artifact staleness.
//!
//! A fresh base session can reattach to an interrupted run through its persisted
//! plan.  This module owns the small, read-only decision surface for determining
//! whether such a plan is resumable and for reopening steps whose upstream
//! artifacts changed after the plan was saved.

use std::path::Path;

use serde::{Deserialize, Serialize};
use umadev_spec::Phase;

use crate::plan_state::{self, Plan, StepStatus};

const OPERATIONAL_REVIEW_CHECKPOINT_FILE: &str = "director-operational-review.json";
const MAX_OPERATIONAL_REVIEW_CHECKPOINT_BYTES: u64 = 16 * 1024;

/// The exact review boundary a recoverable host outage parked.
///
/// A step review and the final whole-build review are deliberately distinct:
/// retrying the latter by reopening an ordinary review step would review once in
/// the scheduler and then immediately review a second time in the final gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(super) enum OperationalReviewCheckpoint {
    /// Retry the named plan review step through the normal scheduler.
    StepReview {
        /// Stable persisted plan-step id.
        step_id: String,
        /// QC-input identity for which the build step's deterministic
        /// acceptance already passed before its reviewer became unavailable.
        #[serde(default)]
        qc_source_fingerprint: Option<String>,
        /// Exact reviewer roster for this boundary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        required_seats: Option<Vec<crate::critics::Seat>>,
        /// A Build step already ran its doer and reached only its
        /// `ReviewClean` acceptance boundary. When the fingerprint still
        /// matches, resume must retry that review rather than re-run the doer.
        #[serde(default)]
        review_only: bool,
    },
    /// Skip already-settled plan steps and retry the final whole-build gate.
    FinalGateReview {
        /// QC-input identity (source/tests plus manifests, lockfiles, build
        /// config, and CI) for which every deterministic floor already passed.
        /// When it still matches, resume retries only the unavailable reviewer;
        /// a missing/mismatched fingerprint conservatively re-runs QC.
        #[serde(default)]
        qc_source_fingerprint: Option<String>,
        /// Exact required reviewer roster that was convened at the parked
        /// boundary. A continuation must not silently resize the gate by
        /// re-routing the requirement.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        required_seats: Option<Vec<crate::critics::Seat>>,
        /// Resident task-ledger run that owns this paused boundary. Older
        /// checkpoints omit it and resume conservatively without ledger reuse.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry_task_run_id: Option<String>,
    },
}

/// In-band plan marker for a final-review retry.
///
/// The checkpoint and plan are separate atomically-written files, so a process can
/// die between their renames. Keeping a distinctive cursor in the plan lets the
/// loader reconstruct a missing checkpoint without scheduling an ordinary review
/// and then immediately reviewing again in the final gate.
pub(super) const FINAL_REVIEW_RETRY_STEP_ID: &str = "umadev-final-review-retry";

fn operational_review_checkpoint_path(root: &Path) -> std::path::PathBuf {
    root.join(".umadev")
        .join(OPERATIONAL_REVIEW_CHECKPOINT_FILE)
}

pub(super) fn save_operational_review_checkpoint(
    root: &Path,
    checkpoint: &OperationalReviewCheckpoint,
) -> std::io::Result<()> {
    let dir = root.join(".umadev");
    std::fs::create_dir_all(&dir)?;
    let body = serde_json::to_vec_pretty(checkpoint)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    umadev_state::fs::atomic_write(&operational_review_checkpoint_path(root), body.as_slice())
}

pub(super) fn load_operational_review_checkpoint(
    root: &Path,
) -> Option<OperationalReviewCheckpoint> {
    let body = umadev_state::fs::read_bounded(
        &operational_review_checkpoint_path(root),
        MAX_OPERATIONAL_REVIEW_CHECKPOINT_BYTES,
    )
    .ok()?;
    serde_json::from_slice(&body).ok()
}

pub(super) fn clear_operational_review_checkpoint(root: &Path) {
    let _ = std::fs::remove_file(operational_review_checkpoint_path(root));
}

/// Whether `plan` still has work left to drive — at least one non-terminal step.
fn plan_has_incomplete_step(plan: &Plan) -> bool {
    plan.steps
        .iter()
        .any(|step| matches!(step.status, StepStatus::Pending | StepStatus::Active))
}

/// Reset interrupted work so a fresh session can schedule it again.
fn reset_active_to_pending(plan: &mut Plan) {
    for step in &mut plan.steps {
        if step.status == StepStatus::Active {
            step.status = StepStatus::Pending;
        }
    }
}

fn final_review_retry_step(depends_on: Vec<String>) -> plan_state::PlanStep {
    plan_state::PlanStep {
        id: FINAL_REVIEW_RETRY_STEP_ID.to_string(),
        title: "Retry final whole-build review".to_string(),
        seat: crate::critics::Seat::QaEngineer,
        kind: plan_state::StepKind::Review,
        depends_on,
        acceptance: plan_state::AcceptanceSpec::ReviewClean,
        evidence: Vec::new(),
        files: plan_state::StepFiles::default(),
        status: StepStatus::Pending,
    }
}

fn reconcile_plan_with_operational_checkpoint(
    plan: &mut Plan,
    checkpoint: &OperationalReviewCheckpoint,
) {
    match checkpoint {
        OperationalReviewCheckpoint::StepReview { step_id, .. } => {
            if let Some(step) = plan.steps.iter_mut().find(|step| step.id == *step_id) {
                if matches!(step.status, StepStatus::Done | StepStatus::Blocked) {
                    step.status = StepStatus::Pending;
                }
            } else {
                let mut step = final_review_retry_step(Vec::new());
                step.id.clone_from(step_id);
                step.title = format!("Retry interrupted review `{step_id}`");
                plan.steps.push(step);
            }
        }
        OperationalReviewCheckpoint::FinalGateReview { .. } => {
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
                step.status = StepStatus::Pending;
                step.depends_on = dependencies;
            } else {
                plan.steps.push(final_review_retry_step(dependencies));
            }
        }
    }
}

/// Resolve the logical review checkpoint from either transaction half.
///
/// This is read-only: an in-band final cursor is sufficient to recover a lost
/// checkpoint file, so status/resume probes never rewrite project state.
pub(super) fn operational_review_checkpoint_for_plan(
    root: &Path,
    plan: &Plan,
) -> Option<OperationalReviewCheckpoint> {
    load_operational_review_checkpoint(root).or_else(|| {
        plan.steps
            .iter()
            .any(|step| {
                step.id == FINAL_REVIEW_RETRY_STEP_ID
                    && matches!(step.status, StepStatus::Pending | StepStatus::Active)
            })
            .then_some(OperationalReviewCheckpoint::FinalGateReview {
                // The crash lost the verified-tree receipt. Reconstruct only the
                // boundary; the resume conservatively re-runs deterministic QC.
                qc_source_fingerprint: None,
                required_seats: None,
                entry_task_run_id: None,
            })
    })
}

/// Load a persisted plan only when it still contains resumable work.
///
/// The plan and operational-review checkpoint are reconciled as one logical
/// transaction. Either file may be the one whose atomic rename landed before a
/// crash: a checkpoint can recreate its missing plan cursor, while the distinctive
/// final-review cursor can recreate a missing checkpoint.
pub(super) fn load_resumable_plan(root: &Path) -> Option<Plan> {
    let mut checkpoint = load_operational_review_checkpoint(root);
    let mut plan = match plan_state::load(root) {
        Some(plan) => plan,
        None if checkpoint.is_some() => Plan {
            steps: Vec::new(),
            risks: Vec::new(),
            open_questions: Vec::new(),
        },
        None => return None,
    };
    invalidate_stale_steps(root, &mut plan);
    checkpoint = operational_review_checkpoint_for_plan(root, &plan);
    if let Some(checkpoint) = checkpoint.as_ref() {
        reconcile_plan_with_operational_checkpoint(&mut plan, checkpoint);
    }
    if !plan_has_incomplete_step(&plan) {
        return None;
    }
    reset_active_to_pending(&mut plan);
    Some(plan)
}

/// Canonical artifact name for an `output/<slug>-<kind>.md` file.
fn artifact_name_from_filename(filename: &str) -> Option<&'static str> {
    let stem = filename.strip_suffix(".md")?;
    if stem.ends_with("-architecture") {
        Some("architecture")
    } else if stem.ends_with("-prd") {
        Some("prd")
    } else if stem.ends_with("-uiux") {
        Some("uiux")
    } else {
        None
    }
}

fn artifact_kind_from_name(name: &str) -> Option<crate::critics::ArtifactKind> {
    use crate::critics::ArtifactKind as A;
    match name {
        "architecture" => Some(A::Architecture),
        "prd" => Some(A::Prd),
        "uiux" => Some(A::Uiux),
        _ => None,
    }
}

/// Read current document-artifact content versions. Unreadable inputs are skipped.
fn current_artifact_versions(root: &Path) -> Vec<(String, String)> {
    let mut versions = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join("output")) else {
        return versions;
    };
    for entry in entries.flatten() {
        let filename = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = artifact_name_from_filename(&filename) else {
            continue;
        };
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            versions.push((name.to_string(), crate::critics::artifact_version(&content)));
        }
    }
    versions
}

/// Record the artifact versions the persisted plan was built against.
pub(super) fn record_artifact_versions(root: &Path) {
    let current = current_artifact_versions(root);
    if current.is_empty() {
        return;
    }
    crate::critics::write_artifact_versions(root, &current.into_iter().collect());
}

/// Re-open steps whose upstream document artifacts changed since the last save.
pub(super) fn invalidate_stale_steps(root: &Path, plan: &mut Plan) {
    let current = current_artifact_versions(root);
    if current.is_empty() {
        return;
    }
    let stale = crate::critics::stale_artifacts(root, &current);
    if stale.is_empty() {
        return;
    }

    use crate::critics::ArtifactKind as A;
    let mut kinds = Vec::new();
    for name in &stale {
        let Some(kind) = artifact_kind_from_name(name) else {
            continue;
        };
        kinds.push(kind);
        // Typed contracts are derived from these source documents, so they become
        // stale with their source and must reopen their dependent plan steps too.
        match kind {
            A::Architecture => {
                kinds.push(A::ApiContract);
                kinds.push(A::DataModel);
            }
            A::Uiux => kinds.push(A::DesignTokens),
            A::Prd => kinds.push(A::Acceptance),
            _ => {}
        }
    }
    plan.invalidate_stale(&kinds);
}

/// Whether `root` contains an incomplete persisted director-loop plan.
#[must_use]
pub fn has_resumable_director_plan(root: &Path) -> bool {
    load_resumable_plan(root).is_some()
}

/// Whether `reason` is a RUN-TIME-BUDGET-exhaustion reason — the terminal string a
/// budget-stopped build carries ("run time budget exhausted …", from
/// `plan_incomplete_reason` / the single-turn twin, both wrapped by
/// `qc_incomplete_reason`). This is the string-matching fallback for the surfaces
/// that see only a reason string (not the typed
/// [`crate::director_loop::DirectorLoopOutcome::PausedAtBudget`]); prefer keying off
/// the typed outcome wherever the flow has it. A budget reason is DISTINCT from a
/// transient (429 / network), auth, or generic failure — none of those contain this
/// marker — so it never mis-classifies a real failure as a resumable budget pause.
/// Pure.
#[must_use]
pub fn is_budget_pause_reason(reason: &str) -> bool {
    reason.contains("run time budget exhausted")
}

/// A ONE-LINE localized discoverability hint to emit when a director run stops with a
/// still-resumable plan on disk AND the stop was either a **transient** base failure
/// (a rate limit / an overloaded base / a network blip — [`crate::base_error::is_transient`])
/// OR a **run-time-budget** exhaustion ([`is_budget_pause_reason`]): the plan was
/// saved and `/continue` picks up the unfinished steps.
///
/// Without it a rate-limited or budget-stopped run reads as "it just stopped": the
/// saved plan is invisible unless the user happens to know `/continue` exists.
/// Returns `Some` only when a resumable plan exists AND the reason is transient or a
/// budget pause; a hard failure (auth / context / a non-zero exit) or a run with
/// nothing left to resume yields `None` so no misleading "you can continue" line is
/// shown. A budget pause fills the hint with the plan's `done/total` step counts so
/// the user sees exactly where the run parked.
///
/// Fail-open by construction: classification is a pure scan of `reason` and the
/// resumable check is best-effort file IO — an unclassifiable reason or an
/// unreadable plan simply yields `None` (the stop is never blocked). Pure aside from
/// the read-only plan probe.
#[must_use]
pub fn transient_resume_hint(reason: &str, root: &Path) -> Option<String> {
    // The plan must still be resumable for EITHER hint — probe once and read its
    // progress for the budget-pause variant (done/total).
    let plan = load_resumable_plan(root)?;
    let failure = crate::base_error::classify(None, None, Some(reason.trim()));
    if crate::base_error::is_transient(&failure) {
        return Some(umadev_i18n::tl("run.transient_resume_hint").to_string());
    }
    if is_budget_pause_reason(reason) {
        let (done, total) = plan.progress();
        return Some(umadev_i18n::tlf(
            "run.budget_pause_resume_hint",
            &[&done.to_string(), &total.to_string()],
        ));
    }
    None
}

/// Whether `root` contains any run state that a fresh session can resume.
#[must_use]
pub fn has_resumable_run(root: &Path) -> bool {
    // A clean final workflow state is the canonical terminal receipt. Check it
    // BEFORE the plan: an interrupted write can leave stale Pending/Active rows in
    // `.umadev/plan.json` even though finalization already committed delivery. Letting
    // that stale plan win re-opened the quality/review loop whenever the user merely
    // asked for progress in a later session.
    if let Some(state) = crate::state::read_workflow_state(root) {
        let clean_delivery = state.active_gate.trim().is_empty()
            && state.phase.eq_ignore_ascii_case(Phase::Delivery.id())
            && state.note.trim() == super::DIRECTOR_COMPLETE_NOTE;
        if clean_delivery {
            return false;
        }
    }
    if load_resumable_plan(root).is_some() {
        return true;
    }
    if let Some(state) = crate::state::read_workflow_state(root) {
        if !state.active_gate.trim().is_empty() || state.phase != Phase::Delivery.id() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::critics::Seat;
    use crate::plan_state::{AcceptanceSpec, PlanStep, StepFiles, StepKind};

    /// Build a minimal one-step plan with the given status and persist it under
    /// `root` so the resumable/hint probes have real `.umadev/plan.json` to read.
    fn save_plan(root: &Path, status: StepStatus) {
        let plan = Plan {
            steps: vec![PlanStep {
                id: "s1".to_string(),
                title: "build the thing".to_string(),
                seat: Seat::BackendEngineer,
                kind: StepKind::Build,
                depends_on: Vec::new(),
                acceptance: AcceptanceSpec::SourcePresent,
                evidence: Vec::new(),
                files: StepFiles::default(),
                status,
            }],
            risks: Vec::new(),
            open_questions: Vec::new(),
        };
        plan_state::save(&plan, root).expect("persist test plan");
    }

    #[test]
    fn transient_hint_fires_only_for_transient_reason_with_resumable_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A resumable plan (one still-active step) + a rate-limit reason → hint.
        save_plan(root, StepStatus::Active);
        let hint = transient_resume_hint(
            "API Error: Request rejected (429) · exceeded the 5-hour usage quota",
            root,
        );
        assert_eq!(
            hint.as_deref(),
            Some(umadev_i18n::tl("run.transient_resume_hint")),
            "a rate-limit abort with a resumable plan surfaces the /continue hint"
        );
    }

    #[test]
    fn transient_hint_is_none_for_hard_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        save_plan(root, StepStatus::Active);
        // Auth is a HARD failure — retrying is futile, so no "you can continue" line.
        assert!(
            transient_resume_hint("Error 401 Unauthorized: invalid api key", root).is_none(),
            "a hard auth failure never claims the run is resumable"
        );
    }

    #[test]
    fn transient_hint_is_none_without_a_resumable_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No plan on disk at all → nothing to resume.
        assert!(
            transient_resume_hint("429 too many requests", root).is_none(),
            "a transient reason with no saved plan surfaces no hint"
        );
        // An all-done plan is not resumable either.
        save_plan(root, StepStatus::Done);
        assert!(
            transient_resume_hint("429 too many requests", root).is_none(),
            "a completed plan has nothing left to /continue"
        );
    }

    #[test]
    fn is_budget_pause_reason_matches_only_the_budget_reasons() {
        // The two terminal budget strings (plan-path + single-turn twin) both carry
        // the "run time budget exhausted" marker.
        assert!(is_budget_pause_reason(
            "director build incomplete: run time budget exhausted; 2 plan step(s) unfinished"
        ));
        assert!(is_budget_pause_reason(
            "director build incomplete: run time budget exhausted before auto-QC cleared"
        ));
        // A transient / auth / generic failure is NOT a budget pause.
        assert!(!is_budget_pause_reason(
            "API Error: Request rejected (429) · exceeded the 5-hour usage quota"
        ));
        assert!(!is_budget_pause_reason(
            "Error 401 Unauthorized: invalid api key"
        ));
        assert!(!is_budget_pause_reason(
            "director build incomplete: auto-QC settled without a clean verdict"
        ));
    }

    #[test]
    fn resume_hint_fires_for_a_budget_pause_with_done_total() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A resumable plan (one still-pending step) + a budget reason → the budget
        // resume hint, filled with the plan's done/total (0/1 here).
        save_plan(root, StepStatus::Pending);
        let hint = transient_resume_hint(
            "director build incomplete: run time budget exhausted; 1 plan step(s) unfinished",
            root,
        )
        .expect("a budget pause with a resumable plan surfaces the /continue hint");
        assert_eq!(
            hint,
            umadev_i18n::tlf("run.budget_pause_resume_hint", &["0", "1"]),
            "the budget hint carries done/total from the persisted plan"
        );
    }

    #[test]
    fn budget_resume_hint_is_none_without_a_resumable_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A budget reason but no saved plan → no hint (nothing to resume).
        assert!(
            transient_resume_hint("run time budget exhausted", root).is_none(),
            "a budget reason with no saved plan surfaces no hint"
        );
    }

    #[test]
    fn final_checkpoint_reopens_a_fully_done_plan_after_a_crash_window() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        save_plan(root, StepStatus::Done);
        save_operational_review_checkpoint(
            root,
            &OperationalReviewCheckpoint::FinalGateReview {
                qc_source_fingerprint: Some("tree-a".to_string()),
                required_seats: None,
                entry_task_run_id: None,
            },
        )
        .unwrap();

        let plan = load_resumable_plan(root)
            .expect("the checkpoint half of the transaction recreates a resume cursor");
        assert!(plan.steps.iter().any(|step| {
            step.id == FINAL_REVIEW_RETRY_STEP_ID && step.status == StepStatus::Pending
        }));
    }

    #[test]
    fn in_band_final_cursor_recovers_a_missing_checkpoint_without_rewriting_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let plan = Plan {
            steps: vec![final_review_retry_step(Vec::new())],
            risks: Vec::new(),
            open_questions: Vec::new(),
        };
        plan_state::save(&plan, root).unwrap();

        assert!(load_operational_review_checkpoint(root).is_none());
        let loaded = load_resumable_plan(root).expect("the in-band cursor is resumable");
        assert!(matches!(
            operational_review_checkpoint_for_plan(root, &loaded),
            Some(OperationalReviewCheckpoint::FinalGateReview {
                qc_source_fingerprint: None,
                ..
            })
        ));
        assert!(
            load_operational_review_checkpoint(root).is_none(),
            "a read-only resume/status probe does not rewrite project state"
        );
    }

    #[test]
    fn final_checkpoint_without_a_plan_still_has_a_safe_resume_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        save_operational_review_checkpoint(
            root,
            &OperationalReviewCheckpoint::FinalGateReview {
                qc_source_fingerprint: None,
                required_seats: None,
                entry_task_run_id: None,
            },
        )
        .unwrap();

        let plan = load_resumable_plan(root).expect("a checkpoint-only crash remains resumable");
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].id, FINAL_REVIEW_RETRY_STEP_ID);
        assert_eq!(plan.steps[0].status, StepStatus::Pending);
    }

    #[test]
    fn old_unit_final_checkpoint_deserializes_conservatively() {
        let checkpoint: OperationalReviewCheckpoint =
            serde_json::from_str(r#"{"kind":"final-gate-review"}"#).unwrap();
        assert!(matches!(
            checkpoint,
            OperationalReviewCheckpoint::FinalGateReview {
                qc_source_fingerprint: None,
                ..
            }
        ));
    }
}
