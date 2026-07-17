//! Transaction markers for privacy-sensitive memory lifecycle operations.
//!
//! This module deliberately owns only the operation boundary and metadata. The
//! agent layer knows which files belong to each logical store and moves them
//! into the transaction's payload directory. A tombstone is published only
//! after every payload move succeeds; the deletion audit never stores source
//! paths or memory content.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::memory::MemoryStore;

const LOCK_DIR: &str = ".lifecycle.lock";
const LOCK_OWNER: &str = "owner";
const LOCK_ATTEMPTS: usize = 500;
const LOCK_WAIT: Duration = Duration::from_millis(2);
const LOCK_STALE_AFTER_MS: u64 = 5 * 60 * 1_000;
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const MAX_ACTION_NODES: usize = 40_000;
const MAX_ACTION_DEPTH: usize = 16;
const RECORD_VERSION: u32 = 1;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Explicit ownership boundary recorded for a lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleScope {
    /// One canonical project root.
    Project,
    /// The current user's canonical home directory.
    Global,
}

/// Why active memory was moved out of a store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleOperation {
    /// An explicit user forget request.
    Forget,
    /// Enforcement of an explicitly configured age policy.
    Retention,
}

/// Follow-up action applied to one committed, recoverable tombstone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TombstoneAction {
    /// Return the payload to its original active namespace.
    Restore,
    /// Unlink the payload from the filesystem namespace. This is not a claim
    /// that storage media was physically erased.
    LogicalPurge,
}

/// Commit state of one deletion-audit record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditState {
    /// Payload is staged but the tombstone directory has not been published.
    Prepared,
    /// The tombstone directory was atomically published.
    Committed,
}

/// Content-free marker for memory that remains recoverable under `payload/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TombstoneRecord {
    /// On-disk record schema.
    pub version: u32,
    /// Opaque operation identifier; contains no project or memory content.
    pub id: String,
    /// Explicit boundary selected by the caller.
    pub scope: LifecycleScope,
    /// Logical stores represented by the payload.
    pub stores: Vec<String>,
    /// Operation that created the marker.
    pub operation: LifecycleOperation,
    /// UNIX epoch timestamp in milliseconds.
    pub created_at_ms: u64,
    /// Number of regular files moved out of active storage.
    pub files: usize,
    /// Aggregate byte count reported by the preflight.
    pub bytes: u64,
    /// Whether content was physically destroyed. Soft deletion always writes
    /// `false`; this explicit field prevents audit consumers from guessing.
    pub physically_deleted: bool,
}

/// Content-free audit event for a lifecycle transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeletionAuditRecord {
    /// On-disk record schema.
    pub version: u32,
    /// Opaque event identifier.
    pub id: String,
    /// Matching tombstone identifier.
    pub tombstone_id: String,
    /// Explicit boundary selected by the caller.
    pub scope: LifecycleScope,
    /// Logical store identifiers only; no source paths are recorded.
    pub stores: Vec<String>,
    /// Operation that created the event.
    pub operation: LifecycleOperation,
    /// Transaction state, conservatively `prepared` until publication succeeds.
    pub state: AuditState,
    /// UNIX epoch timestamp in milliseconds.
    pub created_at_ms: u64,
    /// Number of regular files affected.
    pub files: usize,
    /// Aggregate byte count; never any content or content-derived preview.
    pub bytes: u64,
    /// Whether content was physically destroyed.
    pub physically_deleted: bool,
}

/// Content-free disposition and audit record for a tombstone follow-up.
///
/// The same schema is written inside the tombstone as its terminal
/// disposition and under `audit/lifecycle-actions/` as the public audit. It
/// intentionally contains neither payload paths nor memory-derived values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TombstoneActionRecord {
    /// On-disk record schema.
    pub version: u32,
    /// Opaque action event identifier.
    pub id: String,
    /// Opaque identifier of the affected tombstone.
    pub tombstone_id: String,
    /// Explicit ownership boundary selected by the caller.
    pub scope: LifecycleScope,
    /// Logical store identifiers copied from the tombstone marker.
    pub stores: Vec<String>,
    /// Follow-up action applied to the payload.
    pub action: TombstoneAction,
    /// Durable transaction state.
    pub state: AuditState,
    /// UNIX epoch timestamp in milliseconds.
    pub created_at_ms: u64,
    /// Number of regular payload files affected.
    pub files: usize,
    /// Aggregate payload byte count.
    pub bytes: u64,
    /// `true` only for a committed logical purge. Prepared attempts remain
    /// `false`; this describes namespace unlinking, not media sanitisation.
    pub logically_unlinked: bool,
    /// Always `false`: portable filesystem unlink cannot prove physical media
    /// erasure, including on copy-on-write filesystems and SSDs.
    pub physically_deleted: bool,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn operation_id() -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("mlc-{nanos:x}-{:x}-{sequence:x}", std::process::id())
}

fn ensure_memory_dir(boundary: &Path) -> std::io::Result<PathBuf> {
    let root = std::fs::canonicalize(boundary)?;
    if !crate::fs::real_dir(&root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "memory lifecycle boundary is not a real directory",
        ));
    }
    let umadev = crate::fs::ensure_real_child_dir(&root, ".umadev")?;
    crate::fs::ensure_real_child_dir(&umadev, "memory")
}

fn parse_lock_owner(bytes: &[u8]) -> Option<(u64, &str)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (stamp, nonce) = text.split_once('\n')?;
    let stamp = stamp.parse().ok()?;
    (!nonce.is_empty() && !nonce.contains('\n')).then_some((stamp, nonce))
}

#[derive(Debug)]
struct LifecycleLock {
    path: PathBuf,
    nonce: String,
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        let owner = self.path.join(LOCK_OWNER);
        let Ok(bytes) = crate::fs::read_bounded(&owner, 4_096) else {
            return;
        };
        if parse_lock_owner(&bytes).is_none_or(|(_, nonce)| nonce != self.nonce) {
            return;
        }
        let _ = crate::fs::remove_regular_file(&owner);
        let _ = std::fs::remove_dir(&self.path);
    }
}

fn reclaim_stale_lock(lock: &Path) {
    if !crate::fs::real_dir(lock) {
        return;
    }
    let owner = lock.join(LOCK_OWNER);
    let Ok(bytes) = crate::fs::read_bounded(&owner, 4_096) else {
        return;
    };
    let Some((created_at, _)) = parse_lock_owner(&bytes) else {
        return;
    };
    if now_ms().saturating_sub(created_at) <= LOCK_STALE_AFTER_MS {
        return;
    }
    let Some(parent) = lock.parent() else {
        return;
    };
    let stale = parent.join(format!(".lifecycle.lock.stale.{}", operation_id()));
    if std::fs::rename(lock, &stale).is_ok() {
        let _ = crate::fs::remove_regular_file(&stale.join(LOCK_OWNER));
        let _ = std::fs::remove_dir(stale);
    }
}

fn acquire_lock(boundary: &Path) -> std::io::Result<LifecycleLock> {
    let memory = ensure_memory_dir(boundary)?;
    let lock = memory.join(LOCK_DIR);
    for _ in 0..LOCK_ATTEMPTS {
        match std::fs::create_dir(&lock) {
            Ok(()) => {
                let nonce = operation_id();
                let owner = format!("{}\n{nonce}", now_ms());
                if let Err(error) =
                    crate::fs::atomic_write(&lock.join(LOCK_OWNER), owner.as_bytes())
                {
                    let _ = std::fs::remove_dir(&lock);
                    return Err(error);
                }
                return Ok(LifecycleLock { path: lock, nonce });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                reclaim_stale_lock(&lock);
                std::thread::sleep(LOCK_WAIT);
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "memory lifecycle is busy in another UmaDev process",
    ))
}

fn validate_stores(stores: &[MemoryStore]) -> std::io::Result<Vec<String>> {
    let mut ids: Vec<String> = stores.iter().map(|store| store.id().to_string()).collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "memory lifecycle operation requires at least one store",
        ));
    }
    Ok(ids)
}

/// Marker-last lifecycle transaction. Dropping an uncommitted transaction
/// never removes its payload; callers must either roll moved files back or
/// leave the `.pending-*` directory for manual recovery.
#[derive(Debug)]
pub struct LifecycleTransaction {
    _lock: LifecycleLock,
    id: String,
    scope: LifecycleScope,
    stores: Vec<String>,
    operation: LifecycleOperation,
    created_at_ms: u64,
    pending_dir: PathBuf,
    final_dir: PathBuf,
    payload_dir: PathBuf,
    audit_path: PathBuf,
    committed: bool,
}

/// Cross-process-serialized follow-up transaction for one tombstone.
///
/// The agent layer owns payload movement/unlinking because it owns the
/// filesystem classifiers. This state-layer guard validates the tombstone,
/// keeps the lifecycle lock for the entire action, and publishes only
/// content-free transaction metadata.
#[derive(Debug)]
pub struct TombstoneActionTransaction {
    _lock: LifecycleLock,
    tombstone: TombstoneRecord,
    action: TombstoneAction,
    action_id: String,
    created_at_ms: u64,
    payload_dir: PathBuf,
    disposition_path: PathBuf,
    audit_path: PathBuf,
    prepared: bool,
    committed: bool,
}

impl LifecycleTransaction {
    /// Opaque identifier shared by the tombstone and deletion audit.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Root under which callers preserve files using boundary-relative paths.
    #[must_use]
    pub fn payload_dir(&self) -> &Path {
        &self.payload_dir
    }

    /// Atomically publishes the tombstone after the caller staged all payload.
    ///
    /// The audit is written as `prepared` before publication and upgraded to
    /// `committed` afterward. If that final best-effort upgrade fails, the
    /// conservative prepared audit remains rather than losing the event.
    pub fn commit(&mut self, files: usize, bytes: u64) -> std::io::Result<TombstoneRecord> {
        if self.committed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "memory lifecycle transaction is already committed",
            ));
        }
        let tombstone = TombstoneRecord {
            version: RECORD_VERSION,
            id: self.id.clone(),
            scope: self.scope,
            stores: self.stores.clone(),
            operation: self.operation,
            created_at_ms: self.created_at_ms,
            files,
            bytes,
            physically_deleted: false,
        };
        let mut audit = DeletionAuditRecord {
            version: RECORD_VERSION,
            id: self.id.clone(),
            tombstone_id: self.id.clone(),
            scope: self.scope,
            stores: self.stores.clone(),
            operation: self.operation,
            state: AuditState::Prepared,
            created_at_ms: self.created_at_ms,
            files,
            bytes,
            physically_deleted: false,
        };
        let tombstone_bytes = serde_json::to_vec_pretty(&tombstone).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?;
        let audit_bytes = serde_json::to_vec_pretty(&audit).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?;
        crate::fs::atomic_write(&self.pending_dir.join("tombstone.json"), &tombstone_bytes)?;
        crate::fs::atomic_write(&self.audit_path, &audit_bytes)?;
        std::fs::rename(&self.pending_dir, &self.final_dir)?;
        self.committed = true;

        audit.state = AuditState::Committed;
        if let Ok(bytes) = serde_json::to_vec_pretty(&audit) {
            let _ = crate::fs::atomic_write(&self.audit_path, &bytes);
        }
        Ok(tombstone)
    }

    /// Removes metadata for a transaction whose payload has already been fully
    /// rolled back by the caller. Non-empty directories are intentionally left
    /// untouched so this method can never destroy recoverable memory.
    pub fn abort(&mut self) -> std::io::Result<()> {
        if self.committed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a committed lifecycle transaction cannot be aborted",
            ));
        }
        let _ = crate::fs::remove_regular_file(&self.pending_dir.join("tombstone.json"))?;
        let _ = crate::fs::remove_regular_file(&self.audit_path)?;
        remove_empty_dirs(&self.payload_dir)?;
        match std::fs::remove_dir(&self.pending_dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl TombstoneActionTransaction {
    /// Validated marker for the tombstone being acted upon.
    #[must_use]
    pub fn tombstone(&self) -> &TombstoneRecord {
        &self.tombstone
    }

    /// Validated real payload directory. Callers must retain this transaction
    /// while inspecting or changing files under it.
    #[must_use]
    pub fn payload_dir(&self) -> &Path {
        &self.payload_dir
    }

    fn record(&self, state: AuditState, files: usize, bytes: u64) -> TombstoneActionRecord {
        TombstoneActionRecord {
            version: RECORD_VERSION,
            id: self.action_id.clone(),
            tombstone_id: self.tombstone.id.clone(),
            scope: self.tombstone.scope,
            stores: self.tombstone.stores.clone(),
            action: self.action,
            state,
            created_at_ms: self.created_at_ms,
            files,
            bytes,
            logically_unlinked: state == AuditState::Committed
                && self.action == TombstoneAction::LogicalPurge,
            physically_deleted: false,
        }
    }

    /// Durably records a prepared action before any active namespace changes
    /// or irreversible unlinks occur.
    ///
    /// Counts must still match the committed tombstone. A mismatch means the
    /// recovery payload changed after publication and must not be trusted.
    pub fn prepare(&mut self, files: usize, bytes: u64) -> std::io::Result<()> {
        if self.prepared || self.committed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "tombstone action is already prepared",
            ));
        }
        if files != self.tombstone.files || bytes != self.tombstone.bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "tombstone payload no longer matches its content-free marker",
            ));
        }
        let record = self.record(AuditState::Prepared, files, bytes);
        let bytes = serde_json::to_vec_pretty(&record).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?;
        crate::fs::write_new_private(&self.audit_path, &bytes)?;
        self.prepared = true;
        Ok(())
    }

    /// Publishes the terminal disposition after the agent layer has completed
    /// and, for restore, can still roll back every payload move if this call
    /// fails.
    pub fn commit(&mut self) -> std::io::Result<TombstoneActionRecord> {
        if !self.prepared || self.committed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tombstone action must be prepared exactly once before commit",
            ));
        }
        let mut visited = 0usize;
        validate_payload_unlinked(&self.payload_dir, 0, &mut visited)?;
        let record = self.record(
            AuditState::Committed,
            self.tombstone.files,
            self.tombstone.bytes,
        );
        let bytes = serde_json::to_vec_pretty(&record).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?;
        // Creation, rather than replacement, makes a concurrently published
        // disposition a hard conflict even outside the lifecycle lock.
        crate::fs::write_new_private(&self.disposition_path, &bytes)?;
        self.committed = true;
        // The committed disposition is authoritative. Preserve a conservative
        // prepared audit if this best-effort state upgrade cannot be written.
        let _ = crate::fs::atomic_write(&self.audit_path, &bytes);
        Ok(record)
    }
}

fn validate_payload_unlinked(
    path: &Path,
    depth: usize,
    visited: &mut usize,
) -> std::io::Result<()> {
    if depth > MAX_ACTION_DEPTH || *visited >= MAX_ACTION_NODES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "tombstone payload exceeds the action validation bound",
        ));
    }
    *visited += 1;
    let metadata = std::fs::symlink_metadata(path)?;
    if !crate::fs::metadata_is_real_dir(&metadata) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "tombstone payload contains a linked or special directory",
        ));
    }
    let mut entries = std::fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let child = entry.path();
        let metadata = std::fs::symlink_metadata(&child)?;
        if crate::fs::metadata_is_real_dir(&metadata) {
            validate_payload_unlinked(&child, depth + 1, visited)?;
        } else if crate::fs::metadata_is_real_file(&metadata) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::DirectoryNotEmpty,
                "tombstone payload remains recoverable and cannot be committed",
            ));
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "tombstone payload contains a linked or special entry",
            ));
        }
    }
    Ok(())
}

fn remove_empty_dirs(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !crate::fs::metadata_is_real_dir(&metadata) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to clean a linked lifecycle directory",
        ));
    }
    for entry in std::fs::read_dir(path)? {
        let child = entry?.path();
        let child_metadata = std::fs::symlink_metadata(&child)?;
        if !crate::fs::metadata_is_real_dir(&child_metadata) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::DirectoryNotEmpty,
                "lifecycle payload still contains recoverable data",
            ));
        }
        remove_empty_dirs(&child)?;
    }
    std::fs::remove_dir(path)
}

/// Starts a cross-process serialized soft-deletion transaction.
pub fn begin_transaction(
    boundary: &Path,
    scope: LifecycleScope,
    stores: &[MemoryStore],
    operation: LifecycleOperation,
) -> std::io::Result<LifecycleTransaction> {
    let stores = validate_stores(stores)?;
    let lock = acquire_lock(boundary)?;
    let memory = ensure_memory_dir(boundary)?;
    let tombstones = crate::fs::ensure_real_child_dir(&memory, "tombstones")?;
    let audit = crate::fs::ensure_real_child_dir(&memory, "audit")?;
    let deletions = crate::fs::ensure_real_child_dir(&audit, "deletions")?;
    let id = operation_id();
    let pending_name = format!(".pending-{id}");
    let pending_dir = tombstones.join(&pending_name);
    std::fs::create_dir(&pending_dir)?;
    if !crate::fs::real_dir(&pending_dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "lifecycle staging directory is not a real directory",
        ));
    }
    let payload_dir = crate::fs::ensure_real_child_dir(&pending_dir, "payload")?;
    Ok(LifecycleTransaction {
        _lock: lock,
        id: id.clone(),
        scope,
        stores,
        operation,
        created_at_ms: now_ms(),
        pending_dir,
        final_dir: tombstones.join(&id),
        payload_dir,
        audit_path: deletions.join(format!("{id}.json")),
        committed: false,
    })
}

fn validate_operation_id(id: &str) -> std::io::Result<()> {
    if id.len() < 8
        || id.len() > 128
        || !id.starts_with("mlc-")
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid opaque memory lifecycle identifier",
        ));
    }
    Ok(())
}

/// Starts a serialized restore or logical-purge transaction for a committed
/// tombstone.
///
/// The marker, scope, payload directory, and terminal-disposition absence are
/// all validated without following links. The returned guard retains the
/// lifecycle lock until it is dropped.
pub fn begin_tombstone_action(
    boundary: &Path,
    scope: LifecycleScope,
    tombstone_id: &str,
    action: TombstoneAction,
) -> std::io::Result<TombstoneActionTransaction> {
    validate_operation_id(tombstone_id)?;
    let lock = acquire_lock(boundary)?;
    let memory = ensure_memory_dir(boundary)?;
    let tombstones = crate::fs::ensure_real_child_dir(&memory, "tombstones")?;
    let tombstone_dir = tombstones.join(tombstone_id);
    if !crate::fs::real_dir(&tombstone_dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "committed memory tombstone is unavailable or unsafe",
        ));
    }
    let tombstone = read_tombstone(&tombstone_dir.join("tombstone.json"))?;
    if tombstone.id != tombstone_id || tombstone.scope != scope {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "tombstone identity or scope does not match the requested boundary",
        ));
    }
    let payload_dir = tombstone_dir.join("payload");
    if !crate::fs::real_dir(&payload_dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "tombstone payload is missing, linked, or special",
        ));
    }
    let disposition_path = tombstone_dir.join("disposition.json");
    match std::fs::symlink_metadata(&disposition_path) {
        Ok(metadata) if crate::fs::metadata_is_real_file(&metadata) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "tombstone already has a terminal disposition",
            ));
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "unsafe tombstone disposition path",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let audit = crate::fs::ensure_real_child_dir(&memory, "audit")?;
    let actions = crate::fs::ensure_real_child_dir(&audit, "lifecycle-actions")?;
    let action_id = operation_id();
    Ok(TombstoneActionTransaction {
        _lock: lock,
        tombstone,
        action,
        audit_path: actions.join(format!("{action_id}.json")),
        action_id,
        created_at_ms: now_ms(),
        payload_dir,
        disposition_path,
        prepared: false,
        committed: false,
    })
}

/// Reads one tombstone marker without following links or accepting oversized
/// lifecycle metadata.
pub fn read_tombstone(path: &Path) -> std::io::Result<TombstoneRecord> {
    let bytes = crate::fs::read_bounded(path, MAX_RECORD_BYTES)?;
    let record: TombstoneRecord = serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    if record.version != RECORD_VERSION || record.physically_deleted {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported or unsafe memory tombstone record",
        ));
    }
    validate_operation_id(&record.id)?;
    if record.stores.is_empty()
        || record
            .stores
            .iter()
            .any(|store| MemoryStore::parse(store).is_none())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "tombstone contains an invalid logical store set",
        ));
    }
    Ok(record)
}

/// Reads one content-free deletion audit without following links.
pub fn read_deletion_audit(path: &Path) -> std::io::Result<DeletionAuditRecord> {
    let bytes = crate::fs::read_bounded(path, MAX_RECORD_BYTES)?;
    let record: DeletionAuditRecord = serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    if record.version != RECORD_VERSION || record.physically_deleted {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported or unsafe memory deletion audit record",
        ));
    }
    Ok(record)
}

/// Reads one content-free tombstone action record without following links.
pub fn read_tombstone_action(path: &Path) -> std::io::Result<TombstoneActionRecord> {
    let bytes = crate::fs::read_bounded(path, MAX_RECORD_BYTES)?;
    let record: TombstoneActionRecord = serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    let expected_unlinked =
        record.state == AuditState::Committed && record.action == TombstoneAction::LogicalPurge;
    if record.version != RECORD_VERSION
        || record.physically_deleted
        || record.logically_unlinked != expected_unlinked
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported or unsafe tombstone action record",
        ));
    }
    validate_operation_id(&record.id)?;
    validate_operation_id(&record.tombstone_id)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_published_last_and_audit_has_no_content_or_paths() {
        let temp = tempfile::tempdir().unwrap();
        let mut transaction = begin_transaction(
            temp.path(),
            LifecycleScope::Project,
            &[MemoryStore::Facts],
            LifecycleOperation::Forget,
        )
        .unwrap();
        let secret = "customer-secret-do-not-audit";
        crate::fs::atomic_write(
            &transaction.payload_dir().join("payload.bin"),
            secret.as_bytes(),
        )
        .unwrap();
        let id = transaction.id().to_string();
        let tombstone = transaction.commit(1, secret.len() as u64).unwrap();
        assert_eq!(tombstone.id, id);

        let tombstone_path = temp
            .path()
            .join(".umadev/memory/tombstones")
            .join(&id)
            .join("tombstone.json");
        assert_eq!(read_tombstone(&tombstone_path).unwrap(), tombstone);
        let audit_path = temp
            .path()
            .join(".umadev/memory/audit/deletions")
            .join(format!("{id}.json"));
        let audit = read_deletion_audit(&audit_path).unwrap();
        assert_eq!(audit.state, AuditState::Committed);
        let audit_text = std::fs::read_to_string(audit_path).unwrap();
        assert!(!audit_text.contains(secret));
        assert!(!audit_text.contains("payload.bin"));
        assert!(!audit.physically_deleted);
    }

    #[test]
    fn uncommitted_transaction_never_claims_a_tombstone() {
        let temp = tempfile::tempdir().unwrap();
        let transaction = begin_transaction(
            temp.path(),
            LifecycleScope::Project,
            &[MemoryStore::Facts],
            LifecycleOperation::Forget,
        )
        .unwrap();
        let id = transaction.id().to_string();
        drop(transaction);
        assert!(!temp
            .path()
            .join(".umadev/memory/tombstones")
            .join(id)
            .exists());
    }

    #[test]
    fn tombstone_action_is_prepared_then_published_without_paths_or_content() {
        let temp = tempfile::tempdir().unwrap();
        let secret = "private-memory-value";
        let mut deletion = begin_transaction(
            temp.path(),
            LifecycleScope::Project,
            &[MemoryStore::Facts],
            LifecycleOperation::Forget,
        )
        .unwrap();
        crate::fs::atomic_write(
            &deletion.payload_dir().join("opaque.bin"),
            secret.as_bytes(),
        )
        .unwrap();
        let id = deletion.id().to_string();
        deletion.commit(1, secret.len() as u64).unwrap();
        drop(deletion);

        let mut action = begin_tombstone_action(
            temp.path(),
            LifecycleScope::Project,
            &id,
            TombstoneAction::LogicalPurge,
        )
        .unwrap();
        assert_eq!(
            action.prepare(2, secret.len() as u64).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        action.prepare(1, secret.len() as u64).unwrap();
        let prepared = read_tombstone_action(&action.audit_path).unwrap();
        assert_eq!(prepared.state, AuditState::Prepared);
        assert!(!prepared.logically_unlinked);
        assert!(!prepared.physically_deleted);
        assert_eq!(
            action.commit().unwrap_err().kind(),
            std::io::ErrorKind::DirectoryNotEmpty
        );
        crate::fs::remove_regular_file(&action.payload_dir().join("opaque.bin")).unwrap();
        let disposition = action.commit().unwrap();
        assert_eq!(disposition.state, AuditState::Committed);
        assert!(disposition.logically_unlinked);
        assert!(!disposition.physically_deleted);

        let disposition_path = temp
            .path()
            .join(".umadev/memory/tombstones")
            .join(&id)
            .join("disposition.json");
        assert_eq!(
            read_tombstone_action(&disposition_path).unwrap(),
            disposition
        );
        let audit_path = temp
            .path()
            .join(".umadev/memory/audit/lifecycle-actions")
            .join(format!("{}.json", disposition.id));
        assert_eq!(read_tombstone_action(&audit_path).unwrap(), disposition);
        let public_audit = std::fs::read_to_string(audit_path).unwrap();
        assert!(!public_audit.contains(secret));
        assert!(!public_audit.contains("opaque.bin"));
        assert!(!public_audit.contains("payload"));

        drop(action);
        assert_eq!(
            begin_tombstone_action(
                temp.path(),
                LifecycleScope::Project,
                &id,
                TombstoneAction::Restore,
            )
            .unwrap_err()
            .kind(),
            std::io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn tombstone_identity_tampering_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let mut deletion = begin_transaction(
            temp.path(),
            LifecycleScope::Project,
            &[MemoryStore::Facts],
            LifecycleOperation::Forget,
        )
        .unwrap();
        crate::fs::atomic_write(&deletion.payload_dir().join("opaque.bin"), b"x").unwrap();
        let id = deletion.id().to_string();
        deletion.commit(1, 1).unwrap();
        drop(deletion);

        let marker = temp
            .path()
            .join(".umadev/memory/tombstones")
            .join(&id)
            .join("tombstone.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&crate::fs::read_bounded(&marker, MAX_RECORD_BYTES).unwrap())
                .unwrap();
        value["id"] = serde_json::Value::String("mlc-tampered".to_string());
        crate::fs::atomic_write(&marker, &serde_json::to_vec(&value).unwrap()).unwrap();

        assert_eq!(
            begin_tombstone_action(
                temp.path(),
                LifecycleScope::Project,
                &id,
                TombstoneAction::Restore,
            )
            .unwrap_err()
            .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_audit_boundary_is_rejected_before_transaction_starts() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let memory = ensure_memory_dir(temp.path()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), memory.join("audit")).unwrap();
        let error = begin_transaction(
            temp.path(),
            LifecycleScope::Project,
            &[MemoryStore::Facts],
            LifecycleOperation::Forget,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(outside.path().read_dir().unwrap().next().is_none());
    }
}
