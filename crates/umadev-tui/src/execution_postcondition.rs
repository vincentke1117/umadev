//! Resident-turn execution-contract settlement.
//!
//! This module keeps filesystem truth separate from the TUI event pump. A content
//! baseline is captured after intent routing but before the resident writer starts;
//! the final diff is validated only after the base turn and any UmaDev-owned QC
//! turns have settled. Consequently, the terminal success path cannot depend on
//! which tool/event stream happened to perform a write.

use std::path::Path;

use umadev_agent::{ExecutionContract, RoutePlan, WorkspaceBaseline, WorkspaceSnapshotError};

const MAX_FACT_PATHS: usize = 20;

/// A content baseline paired with the routed turn's executable scope contract.
#[derive(Debug)]
pub(super) struct ResidentExecutionPostcondition {
    baseline: WorkspaceBaseline,
    contract: ExecutionContract,
}

/// A blocking inability to prove the resident turn satisfied its contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResidentExecutionBlocked {
    note: String,
}

impl ResidentExecutionPostcondition {
    /// Capture the pre-writer tree and freeze the route-derived contract.
    pub(super) fn capture(
        root: &Path,
        route: &RoutePlan,
        objective: &str,
    ) -> Result<Self, ResidentExecutionBlocked> {
        let baseline = WorkspaceBaseline::capture(root).map_err(snapshot_blocked)?;
        Ok(Self {
            baseline,
            contract: ExecutionContract::from_route(route, objective),
        })
    }

    /// Current changed paths since the pre-writer baseline.
    ///
    /// This is a non-terminal observation only; callers that can run another base
    /// or UmaDev-owned tool afterward must use [`Self::validate_final`] at settlement.
    pub(super) fn changed_paths(
        &self,
        root: &Path,
    ) -> Result<Vec<String>, ResidentExecutionBlocked> {
        self.baseline.changed_paths(root).map_err(snapshot_blocked)
    }

    /// Validate the final tree after every base and UmaDev-owned execution turn.
    pub(super) fn validate_final(
        &self,
        root: &Path,
    ) -> Result<Vec<String>, ResidentExecutionBlocked> {
        let changed = self.changed_paths(root)?;
        let violations = self
            .contract
            .validate_changed_paths(changed.iter().map(String::as_str));
        if violations.is_empty() {
            return Ok(changed);
        }
        let details = violations
            .iter()
            .map(|violation| format!("- [{}] {}", violation.code, violation.message))
            .collect::<Vec<_>>()
            .join("\n");
        Err(ResidentExecutionBlocked {
            note: format!(
                "[blocked] 执行契约未通过,本轮不能标记成功 / execution contract failed; \
                 this turn cannot be marked successful:\n{details}"
            ),
        })
    }
}

impl ResidentExecutionBlocked {
    /// User-visible terminal failure note.
    pub(super) fn into_note(self) -> String {
        self.note
    }
}

fn snapshot_blocked(error: WorkspaceSnapshotError) -> ResidentExecutionBlocked {
    ResidentExecutionBlocked {
        note: format!(
            "[blocked] 无法完整核对本轮工作区内容指纹,因此不能标记成功 / unable to \
             verify the complete workspace content fingerprint; this turn cannot be marked \
             successful: {error}"
        ),
    }
}

/// Snapshot the working tree as `git status --porcelain` for legacy reality
/// prompt/fact rendering. Execution-contract enforcement uses the stronger
/// content-fingerprint baseline above.
pub(super) fn git_status_porcelain(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

pub(super) fn porcelain_path(line: &str) -> Option<String> {
    let trimmed = line.strip_prefix('\u{feff}').unwrap_or(line);
    if trimmed.trim().is_empty() {
        return None;
    }
    let rest = trimmed.get(3..).unwrap_or("").trim();
    if rest.is_empty() {
        return None;
    }
    let path = rest
        .rsplit(" -> ")
        .next()
        .unwrap_or(rest)
        .trim()
        .trim_matches('"');
    (!path.is_empty()).then(|| path.to_string())
}

/// Diff two legacy porcelain snapshots for transcript fact rendering.
pub(super) fn changed_files_between(before: &str, after: &str) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};

    let parse = |snapshot: &str| -> BTreeMap<String, String> {
        snapshot
            .lines()
            .filter_map(|line| porcelain_path(line).map(|path| (path, line.trim_end().to_string())))
            .collect()
    };
    let before = parse(before);
    let after = parse(after);
    let mut changed = BTreeSet::new();
    for (path, line) in &after {
        if before.get(path).map(String::as_str) != Some(line.as_str()) {
            changed.insert(path.clone());
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            changed.insert(path.clone());
        }
    }
    changed.into_iter().collect()
}

/// Build the reality-anchored fact line shown after an agentic turn.
pub(super) fn agentic_fact_line(changed: Option<&[String]>, claimed: bool) -> Option<String> {
    let changed = changed?;
    if changed.is_empty() {
        return Some(if claimed {
            "[note] 本轮无文件变更\n[warn] 底座报告了改动,但工作区没有实际文件变更 —— \
             可能未真正落盘或为复述,请核对 / base reported changes but the working \
             tree is unchanged — verify before trusting"
                .to_string()
        } else {
            "[note] 本轮无文件变更 / no file changes this turn".to_string()
        });
    }
    let mut list = changed
        .iter()
        .take(MAX_FACT_PATHS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if changed.len() > MAX_FACT_PATHS {
        list.push_str(&format!(" ... (+{})", changed.len() - MAX_FACT_PATHS));
    }
    Some(format!("[note] 本轮实际文件变更: {list}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use umadev_agent::{Budget, Depth, RouteClass, TaskKind};

    fn route(class: RouteClass, depth: Depth, scope: &[&str]) -> RoutePlan {
        RoutePlan {
            class,
            kind: TaskKind::Light,
            depth,
            team: Vec::new(),
            scope: scope.iter().map(|path| (*path).to_string()).collect(),
            needs_clarify: None,
            est_budget: Budget::for_route(class, depth),
            confidence: 1.0,
        }
    }

    #[test]
    fn final_diff_contains_base_and_later_umadev_execution_writes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::create_dir_all(root.path().join("tests")).unwrap();
        let postcondition = ResidentExecutionPostcondition::capture(
            root.path(),
            &route(RouteClass::Build, Depth::Fast, &["src/", "tests/"]),
            "implement and verify",
        )
        .unwrap();

        // The selected base writes during the resident turn.
        std::fs::write(root.path().join("src/base.rs"), "base").unwrap();
        assert_eq!(
            postcondition.changed_paths(root.path()).unwrap(),
            ["src/base.rs"]
        );
        // A later UmaDev-owned verifier/QC turn also writes before settlement.
        std::fs::write(root.path().join("tests/qc.rs"), "qc").unwrap();
        assert_eq!(
            postcondition.validate_final(root.path()).unwrap(),
            ["src/base.rs", "tests/qc.rs"]
        );
    }

    #[test]
    fn out_of_scope_final_write_is_blocking_not_success() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        let postcondition = ResidentExecutionPostcondition::capture(
            root.path(),
            &route(RouteClass::QuickEdit, Depth::Fast, &["src/title.rs"]),
            "change the title",
        )
        .unwrap();
        std::fs::write(root.path().join("src/title.rs"), "allowed").unwrap();
        std::fs::write(root.path().join("package.json"), "{}").unwrap();

        let blocked = postcondition
            .validate_final(root.path())
            .expect_err("an out-of-scope/sensitive write cannot settle successfully")
            .into_note();
        assert!(blocked.contains("[blocked]"));
        assert!(blocked.contains("execution-path-out-of-scope"));
        assert!(blocked.contains("package.json"));
        assert!(!blocked.contains("[ok]"));
    }

    #[test]
    fn quick_edit_change_budget_is_enforced_over_actual_content_diff() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        let postcondition = ResidentExecutionPostcondition::capture(
            root.path(),
            &route(RouteClass::QuickEdit, Depth::Fast, &["src/"]),
            "small edit",
        )
        .unwrap();
        for index in 0..5 {
            std::fs::write(root.path().join(format!("src/{index}.rs")), "x").unwrap();
        }
        let note = postcondition
            .validate_final(root.path())
            .unwrap_err()
            .into_note();
        assert!(note.contains("execution-change-budget-exceeded"));
    }

    #[test]
    fn capture_failure_is_explicitly_unverified() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing");
        let note = ResidentExecutionPostcondition::capture(
            &missing,
            &route(RouteClass::Debug, Depth::Fast, &[]),
            "debug",
        )
        .unwrap_err()
        .into_note();
        assert!(note.contains("[blocked]"));
        assert!(note.contains("cannot be marked successful"));
    }

    #[test]
    fn legacy_porcelain_and_fact_helpers_remain_stable() {
        let before = " M a.rs\n?? keep.rs\n";
        let after = " M a.rs\nMM a.rs2\n?? new.rs\n";
        assert_eq!(
            changed_files_between(before, after),
            ["a.rs2", "keep.rs", "new.rs"]
        );
        assert!(agentic_fact_line(Some(&[]), true)
            .unwrap()
            .contains("[warn]"));
    }
}
