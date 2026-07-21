use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    ensure_raw_dir, existing_raw_dir, OutcomeIntent, SentMemoryReceipt, RECEIPTS_DIR,
    RECEIPT_VERSION,
};

const MAX_RECEIPT_FILE_BYTES: u64 = 512 * 1024;
const MAX_RECEIPTS: usize = 4096;
const RECEIPT_PRUNE_TARGET: usize = 3584;
static RECEIPT_TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn ensure_receipts_dir(project_root: &Path) -> Option<PathBuf> {
    let raw = ensure_raw_dir(project_root)?;
    umadev_state::fs::ensure_real_child_dir(&raw, RECEIPTS_DIR).ok()
}

pub(super) fn existing_receipts_dir(project_root: &Path) -> Option<PathBuf> {
    let dir = existing_raw_dir(project_root)?.join(RECEIPTS_DIR);
    umadev_state::fs::real_dir(&dir).then_some(dir)
}

pub(super) fn receipt_path(dir: &Path, receipt_id: &str) -> PathBuf {
    dir.join(format!("{receipt_id}.receipt.json"))
}

pub(super) fn settled_receipt_path(dir: &Path, receipt_id: &str) -> PathBuf {
    dir.join(format!("{receipt_id}.settled-receipt.json"))
}

pub(super) fn intent_path(dir: &Path, receipt_id: &str) -> PathBuf {
    dir.join(format!("{receipt_id}.outcome.json"))
}

pub(super) fn settled_intent_path(dir: &Path, receipt_id: &str) -> PathBuf {
    dir.join(format!("{receipt_id}.settled-outcome.json"))
}

pub(super) fn read_managed_text(path: &Path) -> Option<String> {
    String::from_utf8(umadev_state::fs::read_bounded(path, MAX_RECEIPT_FILE_BYTES).ok()?).ok()
}

pub(super) fn read_receipt(dir: &Path, receipt_id: &str) -> Option<SentMemoryReceipt> {
    [
        receipt_path(dir, receipt_id),
        settled_receipt_path(dir, receipt_id),
    ]
    .into_iter()
    .find_map(|path| {
        read_managed_text(&path)
            .and_then(|body| serde_json::from_str::<SentMemoryReceipt>(&body).ok())
            .filter(|receipt| {
                receipt.version == RECEIPT_VERSION && receipt.receipt_id == receipt_id
            })
    })
}

pub(super) fn valid_receipt_id(receipt_id: &str) -> bool {
    receipt_id.strip_prefix("kr1-").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PublishResult {
    Created,
    AlreadyExists,
    Unavailable,
}

/// Publish bytes at `path` without ever replacing an existing writer. The temp
/// is fully written and synced before the atomic hard-link create-new step.
pub(super) fn publish_create_new(path: &Path, body: &[u8]) -> PublishResult {
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

fn publish_matches<T>(path: &Path, value: &T) -> bool
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq,
{
    let Some(body) = serde_json::to_vec(value).ok() else {
        return false;
    };
    match publish_create_new(path, &body) {
        PublishResult::Created => true,
        PublishResult::AlreadyExists => read_managed_text(path)
            .and_then(|existing| serde_json::from_str::<T>(&existing).ok())
            .is_some_and(|existing| existing == *value),
        PublishResult::Unavailable => false,
    }
}

pub(super) fn receipt_artifact_name(name: &str) -> bool {
    name.strip_suffix(".receipt.json")
        .or_else(|| name.strip_suffix(".settled-receipt.json"))
        .is_some_and(valid_receipt_id)
}

pub(super) fn prune_settled_receipts_to(dir: &Path, maximum: usize, target: usize) {
    if maximum == 0 || target >= maximum {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut total = 0_usize;
    let mut active = 0_usize;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !receipt_artifact_name(name) {
            continue;
        }
        total = total.saturating_add(1);
        if name.strip_suffix(".settled-receipt.json").is_none() {
            active = active.saturating_add(1);
        }
    }
    if total < maximum {
        return;
    }

    let keep_capacity = target.saturating_sub(active);
    let mut newest_settled = BinaryHeap::with_capacity(keep_capacity);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let Some(receipt_id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".settled-receipt.json"))
                .filter(|receipt_id| valid_receipt_id(receipt_id))
            else {
                continue;
            };
            if keep_capacity == 0 {
                continue;
            }
            let candidate = (
                entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(std::time::UNIX_EPOCH),
                receipt_id.to_string(),
                path,
            );
            if newest_settled.len() < keep_capacity {
                newest_settled.push(Reverse(candidate));
            } else if newest_settled
                .peek()
                .is_some_and(|Reverse(oldest)| &candidate > oldest)
            {
                newest_settled.pop();
                newest_settled.push(Reverse(candidate));
            }
        }
    }
    let retained = newest_settled
        .into_iter()
        .map(|Reverse((_, _, path))| path)
        .collect::<HashSet<_>>();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let Some(receipt_id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".settled-receipt.json"))
                .filter(|receipt_id| valid_receipt_id(receipt_id))
            else {
                continue;
            };
            if !retained.contains(&path)
                && umadev_state::fs::remove_regular_file(&path).ok() == Some(true)
            {
                let _ =
                    umadev_state::fs::remove_regular_file(&settled_intent_path(dir, receipt_id));
            }
        }
    }
}

fn prune_settled_receipts(dir: &Path) {
    prune_settled_receipts_to(dir, MAX_RECEIPTS, RECEIPT_PRUNE_TARGET);
}

pub(super) fn receipt_capacity_available(dir: &Path) -> bool {
    prune_settled_receipts(dir);
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(receipt_artifact_name)
            })
            .take(MAX_RECEIPTS)
            .count()
            < MAX_RECEIPTS
    })
}

pub(super) fn finalize_local_settlement(
    dir: &Path,
    receipt: &SentMemoryReceipt,
    intent: &OutcomeIntent,
) -> bool {
    if !publish_matches(&settled_intent_path(dir, &intent.receipt_id), intent) {
        return false;
    }
    if !publish_matches(&settled_receipt_path(dir, &receipt.receipt_id), receipt) {
        return false;
    }
    let _ = umadev_state::fs::remove_regular_file(&receipt_path(dir, &receipt.receipt_id));
    let _ = umadev_state::fs::remove_regular_file(&intent_path(dir, &intent.receipt_id));
    prune_settled_receipts(dir);
    true
}
