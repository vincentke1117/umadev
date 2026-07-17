//! Persisted director-loop resume state and document-artifact staleness.
//!
//! A fresh base session can reattach to an interrupted run through its persisted
//! plan.  This module owns the small, read-only decision surface for determining
//! whether such a plan is resumable and for reopening steps whose upstream
//! artifacts changed after the plan was saved.

use std::path::Path;

use umadev_spec::Phase;

use crate::plan_state::{self, Plan, StepStatus};

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

/// Load a persisted plan only when it still contains resumable work.
pub(super) fn load_resumable_plan(root: &Path) -> Option<Plan> {
    let mut plan = plan_state::load(root)?;
    invalidate_stale_steps(root, &mut plan);
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

/// Whether `root` contains any run state that a fresh session can resume.
#[must_use]
pub fn has_resumable_run(root: &Path) -> bool {
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
