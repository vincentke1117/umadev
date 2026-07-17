//! Typed diagnosis and bounded stuck detection for Director blockers.

use crate::error_kb::{classify_error, ErrorInsight};
use std::collections::{BTreeMap, BTreeSet};

/// Product-facing class of one verification finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingClass {
    /// Compilation, dependency, configuration, or build-tool failure.
    Build,
    /// API/schema/contract drift.
    Contract,
    /// Missing or insufficient test coverage.
    Coverage,
    /// Runtime or assertion behavior differs from the requirement.
    Behavior,
    /// Architecture, accessibility, duplication, or maintainability floor.
    Craft,
}

impl FindingClass {
    /// Stable prompt/report label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Contract => "contract",
            Self::Coverage => "coverage",
            Self::Behavior => "behavior",
            Self::Craft => "craft",
        }
    }
}

/// What the Director should do with the diagnosed finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockerDisposition {
    /// Gather a different piece of evidence before editing again.
    Investigate,
    /// Apply the adjacent, classifier-backed repair and verify it.
    FixAdjacent,
    /// Keep an advisory finding visible without blocking acceptance.
    NoteAndContinue,
    /// Stop repeating the same ineffective repair and settle with evidence.
    Escalate,
}

impl BlockerDisposition {
    /// Stable prompt/report label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Investigate => "investigate",
            Self::FixAdjacent => "fix_adjacent",
            Self::NoteAndContinue => "note_and_continue",
            Self::Escalate => "escalate",
        }
    }
}

/// Classifier-backed description of one current blocker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerDiagnosis {
    /// Product-facing finding class.
    pub class: FindingClass,
    /// Stable, volatility-stripped error fingerprint.
    pub fingerprint: String,
    /// Whether a specific error family, rather than the generic fallback, matched.
    pub recognized: bool,
    /// Classifier-owned root-cause guidance.
    pub root_cause: String,
    /// Classifier-owned repair playbook.
    pub playbook: String,
}

/// One diagnosis combined with the bounded retry decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerAssessment {
    /// Structured diagnosis.
    pub diagnosis: BlockerDiagnosis,
    /// Chosen next action.
    pub disposition: BlockerDisposition,
    /// Consecutive identical no-progress observations.
    pub repeat_count: usize,
}

impl BlockerAssessment {
    /// Render only classifier-owned guidance and stable labels for the next turn.
    #[must_use]
    pub fn prompt_block(&self) -> String {
        let strategy = match self.disposition {
            BlockerDisposition::Investigate if self.repeat_count > 1 => {
                "The previous repair left both the source snapshot and failure fingerprint unchanged. Do not repeat the same command or edit; challenge the prior assumption and collect new evidence first."
            }
            BlockerDisposition::Investigate => {
                "Locate the first causal failure and prove it with a minimal targeted check before editing."
            }
            BlockerDisposition::FixAdjacent => {
                "Apply the bounded adjacent repair below, then rerun the exact failing verifier."
            }
            BlockerDisposition::NoteAndContinue => {
                "Keep this advisory visible; do not widen the current task to fix it."
            }
            BlockerDisposition::Escalate => {
                "Stop repeating this repair. Preserve the evidence and settle the step as blocked."
            }
        };
        format!(
            "## Diagnosed blocker\n- class: {}\n- fingerprint: {}\n- disposition: {}\n- unchanged repeats: {}\n- root cause: {}\n- playbook: {}\n- strategy: {strategy}",
            self.diagnosis.class.as_str(),
            self.diagnosis.fingerprint,
            self.disposition.as_str(),
            self.repeat_count,
            self.diagnosis.root_cause,
            self.diagnosis.playbook,
        )
    }
}

/// Run-local detector for repeated, unchanged failure fingerprints.
#[derive(Debug, Default)]
pub struct StuckDetector {
    last_fingerprint: Option<String>,
    unchanged_repeats: usize,
}

/// Run-local detector for a whole set of independently actionable findings.
///
/// Unlike [`StuckDetector`], this keeps recurrence counts per stable fingerprint,
/// so one changing review item cannot hide another item that has remained stuck.
#[derive(Debug, Default)]
pub struct BlockerSetTracker {
    repeats: BTreeMap<String, usize>,
    previous: BTreeSet<String>,
}

impl BlockerSetTracker {
    /// Diagnose every current finding and choose an action for each one.
    ///
    /// Duplicate fingerprints are collapsed. Findings that disappeared are
    /// forgotten; if they return later they begin a fresh bounded attempt.
    #[must_use]
    pub fn assess_all(
        &mut self,
        findings: &[String],
        criterion: &str,
        blocking: bool,
        workspace_progress: bool,
    ) -> Vec<BlockerAssessment> {
        let mut by_fingerprint = BTreeMap::new();
        for finding in findings {
            let diagnosis = diagnose(finding, criterion);
            by_fingerprint
                .entry(diagnosis.fingerprint.clone())
                .or_insert(diagnosis);
        }

        let current: BTreeSet<String> = by_fingerprint.keys().cloned().collect();
        let mut next_repeats = BTreeMap::new();
        let mut assessments = Vec::with_capacity(by_fingerprint.len());
        for (fingerprint, diagnosis) in by_fingerprint {
            let repeat_count = if workspace_progress || !self.previous.contains(&fingerprint) {
                1
            } else {
                self.repeats
                    .get(&fingerprint)
                    .copied()
                    .unwrap_or(1)
                    .saturating_add(1)
            };
            next_repeats.insert(fingerprint, repeat_count);
            assessments.push(BlockerAssessment {
                disposition: disposition_for(&diagnosis, blocking, repeat_count),
                diagnosis,
                repeat_count,
            });
        }
        self.previous = current;
        self.repeats = next_repeats;
        assessments
    }
}

impl StuckDetector {
    /// Diagnose one finding and choose the next bounded action.
    ///
    /// `workspace_progress` resets the repeat counter even when the verifier still
    /// reports the same family, preventing a productive repair from being stopped.
    #[must_use]
    pub fn assess(
        &mut self,
        evidence: &str,
        criterion: &str,
        blocking: bool,
        workspace_progress: bool,
    ) -> BlockerAssessment {
        let diagnosis = diagnose(evidence, criterion);
        if workspace_progress || self.last_fingerprint.as_deref() != Some(&diagnosis.fingerprint) {
            self.unchanged_repeats = 1;
        } else {
            self.unchanged_repeats = self.unchanged_repeats.saturating_add(1);
        }
        self.last_fingerprint = Some(diagnosis.fingerprint.clone());

        let disposition = disposition_for(&diagnosis, blocking, self.unchanged_repeats);
        BlockerAssessment {
            diagnosis,
            disposition,
            repeat_count: self.unchanged_repeats,
        }
    }
}

fn disposition_for(
    diagnosis: &BlockerDiagnosis,
    blocking: bool,
    repeat_count: usize,
) -> BlockerDisposition {
    if !blocking {
        BlockerDisposition::NoteAndContinue
    } else if repeat_count >= 3 {
        BlockerDisposition::Escalate
    } else if repeat_count == 2 || !diagnosis.recognized {
        BlockerDisposition::Investigate
    } else {
        BlockerDisposition::FixAdjacent
    }
}

fn diagnose(evidence: &str, criterion: &str) -> BlockerDiagnosis {
    let insight = classify_error(evidence);
    BlockerDiagnosis {
        class: finding_class(evidence, criterion, &insight),
        fingerprint: insight.signature,
        recognized: insight.recognized,
        root_cause: insight.root_cause,
        playbook: insight.fix,
    }
}

fn finding_class(evidence: &str, criterion: &str, insight: &ErrorInsight) -> FindingClass {
    let haystack = format!("{criterion}\n{evidence}").to_ascii_lowercase();
    if contains_any(&haystack, &["contract", "openapi", "schema", "api drift"]) {
        FindingClass::Contract
    } else if contains_any(
        &haystack,
        &[
            "coverage",
            "untested",
            "missing test",
            "no test",
            "named test",
        ],
    ) {
        FindingClass::Coverage
    } else if contains_any(
        &haystack,
        &[
            "god-file",
            "architecture",
            "duplicate",
            "comment narration",
            "accessibility",
            "craft",
        ],
    ) {
        FindingClass::Craft
    } else if contains_any(
        &haystack,
        &["assert", "expected", "runtime", "behavior", "http", "cors"],
    ) || matches!(
        insight.category.as_str(),
        "runtime" | "api" | "network" | "test"
    ) {
        FindingClass::Behavior
    } else {
        FindingClass::Build
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_error_uses_playbook_then_changes_strategy_then_escalates() {
        let error = "error[E0308]: mismatched types at src/main.rs:42";
        let mut detector = StuckDetector::default();
        let first = detector.assess(error, "build passes", true, false);
        assert_eq!(first.diagnosis.class, FindingClass::Build);
        assert_eq!(first.disposition, BlockerDisposition::FixAdjacent);
        assert!(first.diagnosis.recognized);
        assert!(!first.diagnosis.playbook.is_empty());

        let second = detector.assess(error, "build passes", true, false);
        assert_eq!(second.disposition, BlockerDisposition::Investigate);
        assert!(second.prompt_block().contains("Do not repeat"));

        let third = detector.assess(error, "build passes", true, false);
        assert_eq!(third.disposition, BlockerDisposition::Escalate);
    }

    #[test]
    fn source_progress_or_a_new_fingerprint_resets_stuck_count() {
        let mut detector = StuckDetector::default();
        let error = "Error: Cannot find module 'react-router-dom'";
        let _ = detector.assess(error, "build", true, false);
        let progressing = detector.assess(error, "build", true, true);
        assert_eq!(progressing.repeat_count, 1);
        assert_eq!(progressing.disposition, BlockerDisposition::FixAdjacent);

        let changed = detector.assess("AssertionError: expected 2 got 3", "test", true, false);
        assert_eq!(changed.repeat_count, 1);
        assert_eq!(changed.diagnosis.class, FindingClass::Behavior);
    }

    #[test]
    fn contract_coverage_craft_and_advisory_are_typed() {
        let mut detector = StuckDetector::default();
        assert_eq!(
            detector
                .assess("OpenAPI schema drift", "contract", true, false)
                .diagnosis
                .class,
            FindingClass::Contract
        );
        assert_eq!(
            detector
                .assess("coverage below threshold", "coverage", true, false)
                .diagnosis
                .class,
            FindingClass::Coverage
        );
        let advisory = detector.assess("god-file advisory", "craft", false, false);
        assert_eq!(advisory.diagnosis.class, FindingClass::Craft);
        assert_eq!(advisory.disposition, BlockerDisposition::NoteAndContinue);
    }

    #[test]
    fn set_tracker_counts_each_fingerprint_without_cross_talk() {
        let mut tracker = BlockerSetTracker::default();
        let first = vec![
            "Error: Cannot find module 'react'".to_string(),
            "AssertionError: expected 2 got 3".to_string(),
        ];
        assert!(tracker
            .assess_all(&first, "objective QC", true, false)
            .iter()
            .all(|item| item.repeat_count == 1));

        let only_assertion = vec!["AssertionError: expected 2 got 3".to_string()];
        let second = tracker.assess_all(&only_assertion, "objective QC", true, false);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].repeat_count, 2);
        assert_eq!(second[0].disposition, BlockerDisposition::Investigate);

        let third = tracker.assess_all(&only_assertion, "objective QC", true, false);
        assert_eq!(third[0].disposition, BlockerDisposition::Escalate);

        let after_progress = tracker.assess_all(&only_assertion, "objective QC", true, true);
        assert_eq!(after_progress[0].repeat_count, 1);
    }
}
