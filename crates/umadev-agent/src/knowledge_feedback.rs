//! Turn-scoped retrieval feedback.
//!
//! Candidate retrieval is pure. Only after the final directive has been accepted
//! by the host does the caller commit an immutable receipt containing the exact
//! content-bound memory IDs and a hash of the sent prompt. A deterministic check
//! then publishes exactly one PASS / FAIL / UNKNOWN intent for that receipt.
//! PASS/FAIL becomes one immutable cross-project usefulness outcome; UNKNOWN is
//! consumed without reward or penalty.
//!
//! Both receipt and outcome intent use temp-file + atomic create-new hard-link
//! publication. There is no overwrite-most-recent state and no shared
//! read-modify-write, so forked turns and separate processes cannot clobber one
//! another. A crash after intent but before usefulness publication is recoverable:
//! replay sees the same immutable receipt ID and the global publisher is
//! idempotent. Every I/O failure stays fail-open and never changes a build verdict.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::lessons::RAW_DIR;
use crate::memory_control::{capture_enabled, MemoryScope, MemoryStore};

fn project_receipt_capture_enabled(project_root: &Path) -> bool {
    capture_enabled(
        project_root,
        MemoryScope::Project,
        MemoryStore::KnowledgeReceipts,
    )
}

/// Project-local directory containing immutable sent receipts and outcome
/// intents.
pub const RECEIPTS_DIR: &str = "knowledge-receipts";

/// Receipt schema version.
const RECEIPT_VERSION: u8 = 1;

static RECEIPT_ID_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static RECEIPT_TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Compatibility snapshot for explicit experiments/tests. Production prompt
/// assembly does not write it.
pub const SURFACED_CHUNKS_FILE: &str = "surfaced-chunks.json";

/// Hard cap on how many chunk keys one step's snapshot retains — the step only
/// injects a handful of chunks, and this bounds the outcome record either way.
pub const MAX_TRACKED_CHUNKS: usize = 12;

/// Stable, standalone prompt marker proving that one exact content-bound memory
/// survived final prompt assembly. The marker is deliberately independent of a
/// chunk's human-readable path/section: those strings may coincidentally appear
/// elsewhere in a directive and are not causal evidence that the retrieved
/// content was sent.
const SENT_MEMORY_MARKER_PREFIX: &str = "<!-- umadev-memory:";

/// Render the exact standalone line that prompt assembly places immediately
/// before one retrieved memory. Receipt commit accepts only this full line.
#[must_use]
pub fn sent_memory_marker(memory_id: &str) -> String {
    format!("{SENT_MEMORY_MARKER_PREFIX}{memory_id} -->")
}

fn directive_has_memory_marker(sent_directive: &str, memory_id: &str) -> bool {
    let marker = sent_memory_marker(memory_id);
    sent_directive.lines().any(|line| line.trim() == marker)
}

/// Mechanical outcome for a sent-memory turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// Deterministic positive evidence exists for the turn.
    Pass,
    /// Deterministic failure evidence exists for the turn.
    Fail,
    /// The check was unavailable, skipped, aborted, or otherwise inconclusive.
    /// This consumes the receipt without changing usefulness.
    Unknown,
}

/// Result of settling one sent-memory receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptSettlement {
    /// This call durably applied (or consumed) the outcome.
    Settled,
    /// The same receipt outcome was already durably applied.
    AlreadySettled,
    /// The outcome intent is durable but the global usefulness store is
    /// temporarily unavailable; replay can finish it later.
    Deferred,
    /// The local outcome is durably consumed, but cross-project usefulness
    /// publication was not authorised by the user's global capture policy.
    SuppressedByPolicy,
    /// No valid receipt exists for this token.
    NotFound,
    /// Another outcome already won for this receipt. First writer is retained.
    Conflict,
}

/// Scope guard for production turn orchestration. If control exits before a
/// deterministic verdict is recorded (early return, cancellation, or panic),
/// dropping the guard consumes the receipt as [`TurnOutcome::Unknown`]. Explicit
/// settlement disarms the fallback once it reaches a terminal state. A deferred
/// settlement is retried on drop with the same PASS/FAIL outcome, never replaced
/// by UNKNOWN.
#[derive(Debug)]
#[must_use = "keep the guard alive until the turn's deterministic outcome is known"]
pub struct SentReceiptGuard {
    project_root: PathBuf,
    home: Option<PathBuf>,
    receipt_id: String,
    settled: bool,
    drop_outcome: TurnOutcome,
}

impl SentReceiptGuard {
    /// Arm an Unknown-on-drop guard for one committed receipt token.
    pub fn new(project_root: &Path, receipt_id: impl Into<String>) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            home: None,
            receipt_id: receipt_id.into(),
            settled: false,
            drop_outcome: TurnOutcome::Unknown,
        }
    }

    /// Explicit-home variant for embedders and tests that must not consult the
    /// process user's home directory.
    pub fn new_in(project_root: &Path, home: &Path, receipt_id: impl Into<String>) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            home: Some(home.to_path_buf()),
            receipt_id: receipt_id.into(),
            settled: false,
            drop_outcome: TurnOutcome::Unknown,
        }
    }

    /// The opaque receipt token carried by this guard.
    #[must_use]
    pub fn receipt_id(&self) -> &str {
        &self.receipt_id
    }

    /// Consume the guard with one explicit mechanical outcome.
    #[must_use]
    pub fn settle(mut self, outcome: TurnOutcome) -> ReceiptSettlement {
        self.drop_outcome = outcome;
        let settlement = match self.home.as_deref() {
            Some(home) => settle_receipt_in(&self.project_root, home, &self.receipt_id, outcome),
            None => settle_receipt(&self.project_root, &self.receipt_id, outcome),
        };
        // Deferred may mean either the local intent or its global publication
        // was unavailable. Preserve the exact mechanical outcome for Drop's
        // best-effort retry; never replace it with Unknown.
        self.settled = settlement != ReceiptSettlement::Deferred;
        settlement
    }
}

impl Drop for SentReceiptGuard {
    fn drop(&mut self) {
        if !self.settled {
            let _ = match self.home.as_deref() {
                Some(home) => settle_receipt_in(
                    &self.project_root,
                    home,
                    &self.receipt_id,
                    self.drop_outcome,
                ),
                None => settle_receipt(&self.project_root, &self.receipt_id, self.drop_outcome),
            };
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SentMemoryReceipt {
    version: u8,
    receipt_id: String,
    sent_prompt_sha256: String,
    sent_at: String,
    memories: Vec<umadev_knowledge::MemoryRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OutcomeIntent {
    version: u8,
    receipt_id: String,
    outcome: TurnOutcome,
    /// Captured at the first settlement attempt. `false` is permanent: turning
    /// global capture on later must never retroactively publish an outcome that
    /// was settled while capture was off. Legacy intents default to false.
    #[serde(default)]
    publish_utility: bool,
}

/// Full path to the surfaced-chunks snapshot for a project.
fn snapshot_path(project_root: &Path) -> std::path::PathBuf {
    project_root.join(RAW_DIR).join(SURFACED_CHUNKS_FILE)
}

fn receipts_dir(project_root: &Path) -> PathBuf {
    project_root.join(RAW_DIR).join(RECEIPTS_DIR)
}

fn receipt_path(project_root: &Path, receipt_id: &str) -> PathBuf {
    receipts_dir(project_root).join(format!("{receipt_id}.receipt.json"))
}

fn intent_path(project_root: &Path, receipt_id: &str) -> PathBuf {
    receipts_dir(project_root).join(format!("{receipt_id}.outcome.json"))
}

fn valid_receipt_id(receipt_id: &str) -> bool {
    receipt_id.strip_prefix("kr1-").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn next_receipt_id(prompt_hash: &str) -> String {
    let sequence = RECEIPT_ID_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let digest = sha256_hex(&format!(
        "knowledge-receipt-v1\0{}\0{}\0{sequence}\0{prompt_hash}",
        std::process::id(),
        stamp
    ));
    format!("kr1-{digest}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishResult {
    Created,
    AlreadyExists,
    Unavailable,
}

/// Publish bytes at `path` without ever replacing an existing writer. The temp
/// is fully written and synced before the atomic hard-link create-new step.
fn publish_create_new(path: &Path, body: &[u8]) -> PublishResult {
    let Some(parent) = path.parent() else {
        return PublishResult::Unavailable;
    };
    if std::fs::create_dir_all(parent).is_err()
        || !std::fs::symlink_metadata(parent).is_ok_and(|meta| meta.file_type().is_dir())
    {
        return PublishResult::Unavailable;
    }
    if path.exists() {
        return PublishResult::AlreadyExists;
    }
    let sequence = RECEIPT_TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("knowledge-receipt");
    let temp_path = parent.join(format!(
        ".{name}.{}.{}.{}.tmp",
        std::process::id(),
        stamp,
        sequence
    ));
    let Ok(mut temp) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    else {
        return PublishResult::Unavailable;
    };
    if temp.write_all(body).is_err() || temp.sync_all().is_err() {
        let _ = std::fs::remove_file(&temp_path);
        return PublishResult::Unavailable;
    }
    drop(temp);
    let published = std::fs::hard_link(&temp_path, path);
    let _ = std::fs::remove_file(&temp_path);
    match published {
        Ok(()) => PublishResult::Created,
        Err(_) if path.exists() => PublishResult::AlreadyExists,
        Err(_) => PublishResult::Unavailable,
    }
}

/// Commit the exact memories present in a final directive after the host has
/// accepted that directive. Candidate retrieval must never call this function.
///
/// The receipt includes a SHA-256 of the complete sent directive. Memories whose
/// exact stable marker line is absent from that final payload are defensively
/// dropped, so a downstream budgeter cannot receive credit for content it
/// removed. Merely mentioning the same path/section is insufficient. Empty or
/// unwritable receipts fail open to `None`.
#[must_use]
pub fn commit_sent_memories(
    project_root: &Path,
    sent_directive: &str,
    memories: &[umadev_knowledge::MemoryRef],
) -> Option<String> {
    if !project_receipt_capture_enabled(project_root) {
        return None;
    }
    if sent_directive.is_empty() || memories.is_empty() {
        return None;
    }
    let mut sent = memories
        .iter()
        .filter(|memory| directive_has_memory_marker(sent_directive, &memory.id))
        .take(MAX_TRACKED_CHUNKS)
        .cloned()
        .collect::<Vec<_>>();
    sent.sort_by(|left, right| left.id.cmp(&right.id));
    sent.dedup_by(|left, right| left.id == right.id);
    if sent.is_empty() {
        return None;
    }
    let prompt_hash = sha256_hex(sent_directive);
    let receipt_id = next_receipt_id(&prompt_hash);
    let receipt = SentMemoryReceipt {
        version: RECEIPT_VERSION,
        receipt_id: receipt_id.clone(),
        sent_prompt_sha256: prompt_hash,
        sent_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        memories: sent,
    };
    let body = serde_json::to_vec(&receipt).ok()?;
    match publish_create_new(&receipt_path(project_root, &receipt_id), &body) {
        PublishResult::Created => Some(receipt_id),
        PublishResult::AlreadyExists => {
            std::fs::read_to_string(receipt_path(project_root, &receipt_id))
                .ok()
                .and_then(|existing| serde_json::from_str::<SentMemoryReceipt>(&existing).ok())
                .filter(|existing| existing == &receipt)
                .map(|_| receipt_id)
        }
        PublishResult::Unavailable => None,
    }
}

/// Settle one committed receipt against the normal cross-project user home.
/// UNKNOWN is durable but intentionally records no usefulness update.
#[must_use]
pub fn settle_receipt(
    project_root: &Path,
    receipt_id: &str,
    outcome: TurnOutcome,
) -> ReceiptSettlement {
    let settlement = settle_receipt_with_home(project_root, None, receipt_id, outcome);
    let _ = recover_recorded_outcomes_with_home(project_root, None);
    settlement
}

/// Explicit-home variant of [`settle_receipt`], useful for deterministic tests
/// and embedders that keep UmaDev state outside the process environment.
#[must_use]
pub fn settle_receipt_in(
    project_root: &Path,
    home: &Path,
    receipt_id: &str,
    outcome: TurnOutcome,
) -> ReceiptSettlement {
    let settlement = settle_receipt_with_home(project_root, Some(home), receipt_id, outcome);
    let _ = recover_recorded_outcomes_with_home(project_root, Some(home));
    settlement
}

fn settle_receipt_with_home(
    project_root: &Path,
    home: Option<&Path>,
    receipt_id: &str,
    outcome: TurnOutcome,
) -> ReceiptSettlement {
    if !valid_receipt_id(receipt_id) {
        return ReceiptSettlement::NotFound;
    }
    let Some(receipt) = std::fs::read_to_string(receipt_path(project_root, receipt_id))
        .ok()
        .and_then(|body| serde_json::from_str::<SentMemoryReceipt>(&body).ok())
        .filter(|receipt| receipt.version == RECEIPT_VERSION && receipt.receipt_id == receipt_id)
    else {
        return ReceiptSettlement::NotFound;
    };
    // Issuing a receipt is automatic capture and is gated at commit. Settling an
    // already-issued receipt remains allowed after project capture is disabled:
    // it closes causal state rather than opening a new attribution attempt.
    let publish_utility = outcome != TurnOutcome::Unknown
        && match home {
            Some(home) => umadev_knowledge::usefulness::knowledge_utility_capture_enabled_in(home),
            None => umadev_knowledge::usefulness::knowledge_utility_capture_enabled(),
        };
    let proposed_intent = OutcomeIntent {
        version: RECEIPT_VERSION,
        receipt_id: receipt_id.to_string(),
        outcome,
        publish_utility,
    };
    let Some(body) = serde_json::to_vec(&proposed_intent).ok() else {
        return ReceiptSettlement::Deferred;
    };
    let (newly_recorded, intent) =
        match publish_create_new(&intent_path(project_root, receipt_id), &body) {
            PublishResult::Created => (true, proposed_intent),
            PublishResult::AlreadyExists => {
                let existing = std::fs::read_to_string(intent_path(project_root, receipt_id))
                    .ok()
                    .and_then(|existing| serde_json::from_str::<OutcomeIntent>(&existing).ok());
                let Some(existing) = existing.filter(|existing| {
                    existing.version == RECEIPT_VERSION
                        && existing.receipt_id == receipt_id
                        && existing.outcome == outcome
                }) else {
                    return ReceiptSettlement::Conflict;
                };
                (false, existing)
            }
            PublishResult::Unavailable => return ReceiptSettlement::Deferred,
        };
    if outcome == TurnOutcome::Unknown {
        return if newly_recorded {
            ReceiptSettlement::Settled
        } else {
            ReceiptSettlement::AlreadySettled
        };
    }
    if !intent.publish_utility {
        return ReceiptSettlement::SuppressedByPolicy;
    }
    let write = match home {
        Some(home) => umadev_knowledge::record_receipt_outcome_in(
            home,
            project_root,
            receipt_id,
            &receipt.memories,
            outcome == TurnOutcome::Pass,
        ),
        None => umadev_knowledge::record_receipt_outcome(
            project_root,
            receipt_id,
            &receipt.memories,
            outcome == TurnOutcome::Pass,
        ),
    };
    match write {
        umadev_knowledge::ReceiptOutcomeWrite::Recorded => ReceiptSettlement::Settled,
        umadev_knowledge::ReceiptOutcomeWrite::AlreadyRecorded => ReceiptSettlement::AlreadySettled,
        umadev_knowledge::ReceiptOutcomeWrite::SuppressedByPolicy => {
            ReceiptSettlement::SuppressedByPolicy
        }
        umadev_knowledge::ReceiptOutcomeWrite::Unavailable => ReceiptSettlement::Deferred,
        umadev_knowledge::ReceiptOutcomeWrite::Conflict => ReceiptSettlement::Conflict,
    }
}

/// Replay durable outcome intents left between local settlement and global
/// usefulness publication. Returns how many intents are now settled. A corrupt
/// file or unavailable home is ignored; it never affects the host turn.
pub fn recover_recorded_outcomes(project_root: &Path) -> usize {
    recover_recorded_outcomes_with_home(project_root, None)
}

/// Explicit-home variant of [`recover_recorded_outcomes`].
pub fn recover_recorded_outcomes_in(project_root: &Path, home: &Path) -> usize {
    recover_recorded_outcomes_with_home(project_root, Some(home))
}

fn recover_recorded_outcomes_with_home(project_root: &Path, home: Option<&Path>) -> usize {
    let Ok(entries) = std::fs::read_dir(receipts_dir(project_root)) else {
        return 0;
    };
    let mut intents = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".outcome.json"))
        })
        .collect::<Vec<_>>();
    intents.sort();
    intents
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|body| serde_json::from_str::<OutcomeIntent>(&body).ok())
        .filter(|intent| intent.version == RECEIPT_VERSION)
        .filter(|intent| {
            matches!(
                settle_receipt_with_home(project_root, home, &intent.receipt_id, intent.outcome,),
                ReceiptSettlement::Settled
                    | ReceiptSettlement::AlreadySettled
                    | ReceiptSettlement::SuppressedByPolicy
            )
        })
        .count()
}

/// Snapshot `(path, section)` keys for an explicit experiment/test. Overwrites
/// the previous value, is bounded to [`MAX_TRACKED_CHUNKS`], and is fail-open.
/// This snapshot alone must not authorize production pass/fail attribution.
pub fn record_surfaced_chunks(project_root: &Path, keys: &[(String, String)]) {
    if !project_receipt_capture_enabled(project_root) {
        return;
    }
    let raw_dir = project_root.join(RAW_DIR);
    let _ = std::fs::create_dir_all(&raw_dir);
    let bounded: Vec<&(String, String)> = keys.iter().take(MAX_TRACKED_CHUNKS).collect();
    if let Ok(json) = serde_json::to_string(&bounded) {
        let _ = std::fs::write(raw_dir.join(SURFACED_CHUNKS_FILE), json);
    }
}

/// Read the most recently surfaced chunk keys (written by
/// [`record_surfaced_chunks`]). Fail-open: a missing/corrupt snapshot yields an
/// empty vec (no feedback), never an error.
#[must_use]
pub fn read_surfaced_chunks(project_root: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(snapshot_path(project_root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Explicit compatibility helper used by tests. It folds the current snapshot
/// into the usefulness prior, but is intentionally not wired to production
/// verdicts because the snapshot has no exact sent-prompt/turn token.
fn apply(project_root: &Path, home: Option<&Path>, helpful: bool) {
    let keys = read_surfaced_chunks(project_root);
    if keys.is_empty() {
        return; // nothing surfaced → nothing to attribute (never touches the store)
    }
    match home {
        Some(h) => umadev_knowledge::usefulness::record_chunk_outcomes_in(h, &keys, helpful),
        None => umadev_knowledge::usefulness::record_chunk_outcomes(&keys, helpful),
    }
}

/// Explicit/manual reward primitive. Not called by production run paths.
pub fn reward_surfaced_chunks(project_root: &Path) {
    apply(project_root, None, true);
}

/// Explicit/manual penalty primitive. Not called by production run paths.
pub fn penalise_surfaced_chunks(project_root: &Path) {
    apply(project_root, None, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use umadev_knowledge::usefulness::{UsefulnessStore, MIN_SAMPLES, NEUTRAL_WEIGHT};

    fn enable_utility_capture(home: &Path) {
        umadev_state::memory::update_policy(home, |policy| {
            policy.set_capture(
                Some(umadev_state::memory::MemoryStore::KnowledgeUtility),
                true,
            );
            Ok(())
        })
        .unwrap();
    }

    fn key(path: &str, section: &str) -> (String, String) {
        (path.to_string(), section.to_string())
    }

    fn memory(path: &str, section: &str, body: &str) -> umadev_knowledge::MemoryRef {
        umadev_knowledge::MemoryRef::from_parts(path, section, body)
    }

    fn commit(project: &Path, memory: &umadev_knowledge::MemoryRef, suffix: &str) -> String {
        commit_sent_memories(
            project,
            &format!(
                "{}\nknowledge: {} — {}\n{suffix}",
                sent_memory_marker(&memory.id),
                memory.path,
                memory.section
            ),
            std::slice::from_ref(memory),
        )
        .expect("receipt")
    }

    #[test]
    fn receipt_contains_only_memories_that_survived_final_prompt_assembly() {
        let project = tempfile::TempDir::new().unwrap();
        let kept = memory("security/login.md", "OAuth", "Use PKCE.");
        let dropped = memory("database/tx.md", "Isolation", "Use serializable.");
        let prompt = format!(
            "{}\nRelevant team experience: security/login.md — OAuth: Use PKCE.",
            sent_memory_marker(&kept.id)
        );
        let token = commit_sent_memories(project.path(), &prompt, &[kept.clone(), dropped])
            .expect("a kept memory creates a receipt");
        let receipt: SentMemoryReceipt = serde_json::from_str(
            &std::fs::read_to_string(receipt_path(project.path(), &token)).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt.sent_prompt_sha256, sha256_hex(&prompt));
        assert_eq!(receipt.memories, vec![kept]);
    }

    #[test]
    fn path_and_section_mentions_without_the_exact_marker_are_not_attributed() {
        let project = tempfile::TempDir::new().unwrap();
        let m = memory("security/login.md", "OAuth", "Use PKCE.");
        let coincidental =
            "Review security/login.md and the OAuth section; this is not retrieved content.";
        assert!(commit_sent_memories(project.path(), coincidental, &[m]).is_none());
    }

    #[test]
    fn unknown_consumes_the_receipt_without_reward_or_penalty() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        let m = memory("security/login.md", "OAuth", "Use PKCE.");
        let token = commit(project.path(), &m, "unknown turn");
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Unknown),
            ReceiptSettlement::Settled
        );
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Unknown),
            ReceiptSettlement::AlreadySettled
        );
        assert!(UsefulnessStore::load_from(home.path()).is_empty());
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Pass),
            ReceiptSettlement::Conflict,
            "the first tri-state outcome wins"
        );
    }

    #[test]
    fn dropping_an_unsettled_guard_records_unknown() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        let m = memory("security/login.md", "OAuth", "Use PKCE.");
        let token = commit(project.path(), &m, "cancelled turn");
        {
            let _guard = SentReceiptGuard::new_in(project.path(), home.path(), token.clone());
        }
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Unknown),
            ReceiptSettlement::AlreadySettled
        );
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Pass),
            ReceiptSettlement::Conflict
        );
        assert!(UsefulnessStore::load_from(home.path()).is_empty());
    }

    #[test]
    fn pass_and_fail_change_only_the_exact_content_identity() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        enable_utility_capture(home.path());
        let good = memory("shared.md", "Same heading", "good body");
        let colliding = memory("shared.md", "Same heading", "different body");
        for index in 0..MIN_SAMPLES {
            let token = commit(project.path(), &good, &format!("pass {index}"));
            assert!(matches!(
                settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Pass),
                ReceiptSettlement::Settled | ReceiptSettlement::AlreadySettled
            ));
        }
        let store = UsefulnessStore::load_from(home.path());
        assert!(store.weight_for_memory(&good) > NEUTRAL_WEIGHT);
        let colliding_weight = store.weight_for_memory(&colliding);
        assert!(
            (colliding_weight - NEUTRAL_WEIGHT).abs() < f32::EPSILON,
            "different content with the same path/section stays neutral: {colliding_weight}"
        );
    }

    #[test]
    fn concurrent_settlement_counts_one_receipt_exactly_once() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        enable_utility_capture(home.path());
        let m = memory("frontend/forms.md", "Validation", "Validate on blur.");
        let token = commit(project.path(), &m, "concurrent");
        let results = std::thread::scope(|scope| {
            let mut joins = Vec::new();
            for _ in 0..16 {
                joins.push(scope.spawn(|| {
                    settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Pass)
                }));
            }
            joins
                .into_iter()
                .map(|join| join.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == ReceiptSettlement::Settled)
                .count(),
            1
        );
        assert!(results.iter().all(|result| matches!(
            result,
            ReceiptSettlement::Settled | ReceiptSettlement::AlreadySettled
        )));
        let weight = UsefulnessStore::load_from(home.path()).weight_for_memory(&m);
        assert!(
            (weight - NEUTRAL_WEIGHT).abs() < f32::EPSILON,
            "sixteen settlement races still produce only one sample: {weight}"
        );
    }

    #[test]
    fn durable_intent_recovers_after_global_store_was_unavailable() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        enable_utility_capture(home.path());
        let blocked_outcomes = home.path().join(".umadev/knowledge-outcomes");
        std::fs::write(&blocked_outcomes, "not a directory").unwrap();
        let m = memory("backend/http.md", "Timeouts", "Bound every request.");
        let token = commit(project.path(), &m, "recover me");
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Fail),
            ReceiptSettlement::Deferred
        );
        std::fs::remove_file(blocked_outcomes).unwrap();
        assert_eq!(recover_recorded_outcomes_in(project.path(), home.path()), 1);
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Fail),
            ReceiptSettlement::AlreadySettled
        );
    }

    #[test]
    fn snapshot_round_trips_and_is_bounded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let keys: Vec<(String, String)> = (0..(MAX_TRACKED_CHUNKS + 5))
            .map(|i| key(&format!("f{i}.md"), "S"))
            .collect();
        record_surfaced_chunks(tmp.path(), &keys);
        let back = read_surfaced_chunks(tmp.path());
        assert_eq!(
            back.len(),
            MAX_TRACKED_CHUNKS,
            "snapshot is bounded to MAX_TRACKED_CHUNKS"
        );
        assert_eq!(back[0], key("f0.md", "S"));
    }

    #[test]
    fn read_is_fail_open_on_a_missing_snapshot() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(read_surfaced_chunks(tmp.path()).is_empty());
    }

    #[test]
    fn a_passing_step_rewards_its_surfaced_chunks() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        enable_utility_capture(home.path());
        record_surfaced_chunks(project.path(), &[key("security/login.md", "OAuth")]);
        // Reward across enough passing steps to cross the sample gate.
        for _ in 0..MIN_SAMPLES {
            apply(project.path(), Some(home.path()), true);
        }
        let store = UsefulnessStore::load_from(home.path());
        assert!(
            store.weight_for("security/login.md", "OAuth") > NEUTRAL_WEIGHT,
            "a chunk surfaced for passing steps gains usefulness"
        );
    }

    #[test]
    fn a_failing_step_penalises_its_surfaced_chunks() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        enable_utility_capture(home.path());
        record_surfaced_chunks(project.path(), &[key("security/login.md", "OAuth")]);
        for _ in 0..MIN_SAMPLES {
            apply(project.path(), Some(home.path()), false);
        }
        let store = UsefulnessStore::load_from(home.path());
        assert!(
            store.weight_for("security/login.md", "OAuth") < NEUTRAL_WEIGHT,
            "a chunk surfaced for failing steps loses usefulness"
        );
    }

    #[test]
    fn no_surfaced_chunks_is_a_no_op_on_the_store() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        // No snapshot written → apply must not create or touch the home store.
        apply(project.path(), Some(home.path()), true);
        assert!(
            UsefulnessStore::load_from(home.path()).is_empty(),
            "an empty snapshot records nothing (fail-open no-op)"
        );
    }

    #[test]
    fn project_receipt_capture_off_creates_no_receipt_but_does_not_block_the_turn() {
        let project = tempfile::TempDir::new().unwrap();
        crate::memory_control::update_capture(
            project.path(),
            MemoryScope::Project,
            Some(MemoryStore::KnowledgeReceipts),
            false,
        )
        .unwrap();
        let m = memory("security/login.md", "OAuth", "Use PKCE.");
        let prompt = format!("{}\nUse the memory.", sent_memory_marker(&m.id));
        assert!(commit_sent_memories(project.path(), &prompt, &[m]).is_none());
        assert!(!receipts_dir(project.path()).exists());

        record_surfaced_chunks(project.path(), &[key("security/login.md", "OAuth")]);
        assert!(read_surfaced_chunks(project.path()).is_empty());
    }

    #[test]
    fn existing_receipt_settles_after_project_capture_is_disabled() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        let m = memory("security/login.md", "OAuth", "Use PKCE.");
        let token = commit(project.path(), &m, "already sent");
        crate::memory_control::update_capture(
            project.path(),
            MemoryScope::Project,
            Some(MemoryStore::KnowledgeReceipts),
            false,
        )
        .unwrap();
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Unknown),
            ReceiptSettlement::Settled,
            "capture-off closes an already-issued receipt instead of leaving it pending"
        );
    }

    #[test]
    fn utility_capture_off_is_terminal_and_never_retroactively_publishes() {
        let project = tempfile::TempDir::new().unwrap();
        let home = tempfile::TempDir::new().unwrap();
        let m = memory("security/login.md", "OAuth", "Use PKCE.");
        let token = commit(project.path(), &m, "private outcome");
        assert_eq!(
            settle_receipt_in(project.path(), home.path(), &token, TurnOutcome::Pass),
            ReceiptSettlement::SuppressedByPolicy
        );
        enable_utility_capture(home.path());
        assert_eq!(recover_recorded_outcomes_in(project.path(), home.path()), 1);
        assert!(
            UsefulnessStore::load_from(home.path()).is_empty(),
            "later opt-in must not publish an outcome settled while capture was off"
        );
    }
}
