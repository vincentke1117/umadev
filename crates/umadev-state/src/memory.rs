use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const POLICY_VERSION: u32 = 1;
const MAX_POLICY_BYTES: u64 = 64 * 1024;
const POLICY_LOCK_DIR: &str = ".policy.lock";
const POLICY_LOCK_OWNER: &str = "owner";
const POLICY_LOCK_ATTEMPTS: usize = 500;
const POLICY_LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(2);
const POLICY_LOCK_STALE_AFTER_MS: u64 = 5 * 60 * 1_000;
static POLICY_LOCK_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryStore {
    QualityFailures,
    GateRevisions,
    ValidatedPatterns,
    TechDebt,
    Pitfalls,
    Beliefs,
    PitfallReflections,
    GateAdrs,
    OpenDecisions,
    Facts,
    RunNotes,
    Recipes,
    LearnedSkills,
    KnowledgeReceipts,
    KnowledgeUtility,
    CustomKnowledge,
    SkillPackages,
    ChatSessions,
    InputHistory,
    LessonSediment,
    LearnedSkillMirrors,
    GlobalLessonProjection,
    GlobalLessonsManual,
    KnowledgeIndex,
    RepoMap,
    BundledKnowledge,
    EmbeddingModel,
    Tombstones,
    DeletionAudit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionEnforcement {
    Fixed,
    /// A user-configurable age policy with an explicit executor.
    PolicyOnly,
    Unsupported,
}

impl MemoryStore {
    pub const ALL: [Self; 29] = [
        Self::QualityFailures,
        Self::GateRevisions,
        Self::ValidatedPatterns,
        Self::TechDebt,
        Self::Pitfalls,
        Self::Beliefs,
        Self::PitfallReflections,
        Self::GateAdrs,
        Self::OpenDecisions,
        Self::Facts,
        Self::RunNotes,
        Self::Recipes,
        Self::LearnedSkills,
        Self::KnowledgeReceipts,
        Self::KnowledgeUtility,
        Self::CustomKnowledge,
        Self::SkillPackages,
        Self::ChatSessions,
        Self::InputHistory,
        Self::LessonSediment,
        Self::LearnedSkillMirrors,
        Self::GlobalLessonProjection,
        Self::GlobalLessonsManual,
        Self::KnowledgeIndex,
        Self::RepoMap,
        Self::BundledKnowledge,
        Self::EmbeddingModel,
        Self::Tombstones,
        Self::DeletionAudit,
    ];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::QualityFailures => "quality-failures",
            Self::GateRevisions => "gate-revisions",
            Self::ValidatedPatterns => "validated-patterns",
            Self::TechDebt => "tech-debt",
            Self::Pitfalls => "pitfalls",
            Self::Beliefs => "beliefs",
            Self::PitfallReflections => "pitfall-reflections",
            Self::GateAdrs => "gate-adrs",
            Self::OpenDecisions => "open-decisions",
            Self::Facts => "facts",
            Self::RunNotes => "run-notes",
            Self::Recipes => "recipes",
            Self::LearnedSkills => "learned-skills",
            Self::KnowledgeReceipts => "knowledge-receipts",
            Self::KnowledgeUtility => "knowledge-utility",
            Self::CustomKnowledge => "custom-knowledge",
            Self::SkillPackages => "skill-packages",
            Self::ChatSessions => "chat-sessions",
            Self::InputHistory => "input-history",
            Self::LessonSediment => "lesson-sediment",
            Self::LearnedSkillMirrors => "learned-skill-mirrors",
            Self::GlobalLessonProjection => "global-lesson-projection",
            Self::GlobalLessonsManual => "global-lessons-manual",
            Self::KnowledgeIndex => "knowledge-index",
            Self::RepoMap => "repomap",
            Self::BundledKnowledge => "bundled-knowledge",
            Self::EmbeddingModel => "embedding-model",
            Self::Tombstones => "tombstones",
            Self::DeletionAudit => "deletion-audit",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
        Self::ALL.into_iter().find(|store| store.id() == normalized)
    }

    #[must_use]
    pub const fn capture_controllable(self) -> bool {
        matches!(
            self,
            Self::QualityFailures
                | Self::GateRevisions
                | Self::ValidatedPatterns
                | Self::TechDebt
                | Self::Pitfalls
                | Self::Beliefs
                | Self::PitfallReflections
                | Self::GateAdrs
                | Self::Facts
                | Self::RunNotes
                | Self::Recipes
                | Self::LearnedSkills
                | Self::KnowledgeReceipts
                | Self::KnowledgeUtility
                | Self::ChatSessions
                | Self::InputHistory
                | Self::LessonSediment
                | Self::LearnedSkillMirrors
                | Self::GlobalLessonProjection
        )
    }

    #[must_use]
    pub const fn recall_controllable(self) -> bool {
        matches!(
            self,
            Self::QualityFailures
                | Self::GateRevisions
                | Self::ValidatedPatterns
                | Self::TechDebt
                | Self::Pitfalls
                | Self::Beliefs
                | Self::PitfallReflections
                | Self::OpenDecisions
                | Self::Facts
                | Self::RunNotes
                | Self::Recipes
                | Self::LearnedSkills
                | Self::KnowledgeUtility
                | Self::CustomKnowledge
                | Self::SkillPackages
                | Self::ChatSessions
                | Self::InputHistory
                | Self::LessonSediment
                | Self::LearnedSkillMirrors
                | Self::GlobalLessonProjection
                | Self::GlobalLessonsManual
                | Self::BundledKnowledge
        )
    }

    #[must_use]
    pub const fn default_capture(self) -> bool {
        self.capture_controllable() && !matches!(self, Self::KnowledgeUtility)
    }

    #[must_use]
    pub const fn default_recall(self) -> bool {
        self.recall_controllable()
    }

    #[must_use]
    pub const fn supports_project_scope(self) -> bool {
        !matches!(
            self,
            Self::KnowledgeUtility
                | Self::GlobalLessonProjection
                | Self::GlobalLessonsManual
                | Self::BundledKnowledge
                | Self::EmbeddingModel
        )
    }

    #[must_use]
    pub const fn supports_global_scope(self) -> bool {
        matches!(
            self,
            Self::KnowledgeUtility
                | Self::GlobalLessonProjection
                | Self::GlobalLessonsManual
                | Self::BundledKnowledge
                | Self::EmbeddingModel
                | Self::Tombstones
                | Self::DeletionAudit
        )
    }

    #[must_use]
    pub const fn derived(self) -> bool {
        matches!(
            self,
            Self::LessonSediment | Self::LearnedSkillMirrors | Self::KnowledgeIndex | Self::RepoMap
        )
    }

    #[must_use]
    pub const fn clearable_cache(self) -> bool {
        matches!(self, Self::KnowledgeIndex | Self::RepoMap)
    }

    #[must_use]
    pub const fn retention_enforcement(self) -> RetentionEnforcement {
        match self {
            Self::Pitfalls
            | Self::Beliefs
            | Self::PitfallReflections
            | Self::Facts
            | Self::RunNotes
            | Self::Recipes
            | Self::LearnedSkills
            | Self::InputHistory => RetentionEnforcement::Fixed,
            Self::QualityFailures
            | Self::GateRevisions
            | Self::ValidatedPatterns
            | Self::TechDebt
            | Self::KnowledgeReceipts
            | Self::KnowledgeUtility
            | Self::ChatSessions
            | Self::GlobalLessonProjection
            | Self::GlobalLessonsManual => RetentionEnforcement::PolicyOnly,
            _ => RetentionEnforcement::Unsupported,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorePolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recall: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPolicy {
    #[serde(default = "policy_version")]
    pub version: u32,
    #[serde(default = "enabled")]
    pub capture: bool,
    #[serde(default = "enabled")]
    pub recall: bool,
    #[serde(default)]
    pub stores: BTreeMap<String, StorePolicy>,
}

const fn policy_version() -> u32 {
    POLICY_VERSION
}

const fn enabled() -> bool {
    true
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            version: POLICY_VERSION,
            capture: true,
            recall: true,
            stores: BTreeMap::new(),
        }
    }
}

impl MemoryPolicy {
    #[must_use]
    pub fn capture_enabled(&self, store: MemoryStore) -> bool {
        store.capture_controllable()
            && self.capture
            && self
                .stores
                .get(store.id())
                .and_then(|policy| policy.capture)
                .unwrap_or_else(|| store.default_capture())
    }

    #[must_use]
    pub fn recall_enabled(&self, store: MemoryStore) -> bool {
        store.recall_controllable()
            && self.recall
            && self
                .stores
                .get(store.id())
                .and_then(|policy| policy.recall)
                .unwrap_or_else(|| store.default_recall())
    }

    #[must_use]
    pub fn retention_days(&self, store: MemoryStore) -> Option<u32> {
        self.stores
            .get(store.id())
            .and_then(|policy| policy.retention_days)
    }

    pub fn set_capture(&mut self, store: Option<MemoryStore>, enabled: bool) {
        if let Some(store) = store {
            self.stores
                .entry(store.id().to_string())
                .or_default()
                .capture = Some(enabled);
        } else {
            self.capture = enabled;
        }
    }

    pub fn set_recall(&mut self, store: Option<MemoryStore>, enabled: bool) {
        if let Some(store) = store {
            self.stores
                .entry(store.id().to_string())
                .or_default()
                .recall = Some(enabled);
        } else {
            self.recall = enabled;
        }
    }

    pub fn set_retention_days(&mut self, store: MemoryStore, days: Option<u32>) {
        let entry = self.stores.entry(store.id().to_string()).or_default();
        entry.retention_days = days.filter(|days| *days > 0);
        if entry == &StorePolicy::default() {
            self.stores.remove(store.id());
        }
    }

    fn validate(&self) -> std::io::Result<()> {
        if self.version != POLICY_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported memory policy version {}", self.version),
            ));
        }
        if let Some(unknown) = self
            .stores
            .keys()
            .find(|name| MemoryStore::parse(name).is_none())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown memory store `{unknown}`"),
            ));
        }
        Ok(())
    }
}

pub fn policy_path(boundary: &Path) -> std::io::Result<PathBuf> {
    let root = std::fs::canonicalize(boundary)?;
    if !crate::fs::real_dir(&root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "memory policy boundary is not a real directory",
        ));
    }
    Ok(root.join(".umadev").join("memory").join("policy.toml"))
}

fn ensure_policy_path(boundary: &Path) -> std::io::Result<PathBuf> {
    Ok(ensure_memory_dir(boundary)?.join("policy.toml"))
}

fn ensure_memory_dir(boundary: &Path) -> std::io::Result<PathBuf> {
    let root = std::fs::canonicalize(boundary)?;
    if !crate::fs::real_dir(&root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "memory policy boundary is not a real directory",
        ));
    }
    let umadev = crate::fs::ensure_real_child_dir(&root, ".umadev")?;
    crate::fs::ensure_real_child_dir(&umadev, "memory")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn lock_nonce() -> String {
    let sequence = POLICY_LOCK_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{}-{sequence}", std::process::id(), now_ms())
}

fn parse_lock_owner(bytes: &[u8]) -> Option<(u64, &str)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (stamp, nonce) = text.split_once('\n')?;
    let stamp = stamp.parse().ok()?;
    (!nonce.is_empty() && !nonce.contains('\n')).then_some((stamp, nonce))
}

struct PolicyLock {
    path: PathBuf,
    nonce: String,
}

impl Drop for PolicyLock {
    fn drop(&mut self) {
        let owner = self.path.join(POLICY_LOCK_OWNER);
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

fn reclaim_stale_policy_lock(lock: &Path) {
    if !crate::fs::real_dir(lock) {
        return;
    }
    let owner = lock.join(POLICY_LOCK_OWNER);
    let Ok(bytes) = crate::fs::read_bounded(&owner, 4_096) else {
        return;
    };
    let Some((created_at, _)) = parse_lock_owner(&bytes) else {
        return;
    };
    if now_ms().saturating_sub(created_at) <= POLICY_LOCK_STALE_AFTER_MS {
        return;
    }
    let Some(parent) = lock.parent() else {
        return;
    };
    let tomb = parent.join(format!(".policy.lock.stale.{}", lock_nonce()));
    if std::fs::rename(lock, &tomb).is_ok() {
        let _ = crate::fs::remove_regular_file(&tomb.join(POLICY_LOCK_OWNER));
        let _ = std::fs::remove_dir(tomb);
    }
}

fn acquire_policy_lock(boundary: &Path) -> std::io::Result<PolicyLock> {
    let memory = ensure_memory_dir(boundary)?;
    let lock = memory.join(POLICY_LOCK_DIR);
    for _ in 0..POLICY_LOCK_ATTEMPTS {
        match std::fs::create_dir(&lock) {
            Ok(()) => {
                let nonce = lock_nonce();
                let owner = format!("{}\n{nonce}", now_ms());
                if let Err(error) =
                    crate::fs::atomic_write(&lock.join(POLICY_LOCK_OWNER), owner.as_bytes())
                {
                    let _ = std::fs::remove_dir(&lock);
                    return Err(error);
                }
                return Ok(PolicyLock { path: lock, nonce });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                reclaim_stale_policy_lock(&lock);
                std::thread::sleep(POLICY_LOCK_WAIT);
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "memory policy is busy in another UmaDev process",
    ))
}

pub fn load_policy(boundary: &Path) -> std::io::Result<MemoryPolicy> {
    let path = policy_path(boundary)?;
    match crate::fs::read_bounded(&path, MAX_POLICY_BYTES) {
        Ok(bytes) => {
            let text = std::str::from_utf8(&bytes).map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
            })?;
            let policy: MemoryPolicy = toml::from_str(text).map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
            })?;
            policy.validate()?;
            Ok(policy)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(MemoryPolicy::default()),
        Err(error) => Err(error),
    }
}

fn save_policy_unlocked(boundary: &Path, policy: &MemoryPolicy) -> std::io::Result<()> {
    policy.validate()?;
    let path = ensure_policy_path(boundary)?;
    let text = toml::to_string_pretty(policy)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    crate::fs::atomic_write(&path, text.as_bytes())
}

pub fn save_policy(boundary: &Path, policy: &MemoryPolicy) -> std::io::Result<()> {
    let _lock = acquire_policy_lock(boundary)?;
    save_policy_unlocked(boundary, policy)
}

pub fn update_policy(
    boundary: &Path,
    update: impl FnOnce(&mut MemoryPolicy) -> std::io::Result<()>,
) -> std::io::Result<MemoryPolicy> {
    let _lock = acquire_policy_lock(boundary)?;
    let mut policy = load_policy(boundary)?;
    update(&mut policy)?;
    save_policy_unlocked(boundary, &policy)?;
    Ok(policy)
}

#[must_use]
pub fn capture_enabled(boundary: &Path, store: MemoryStore) -> bool {
    store.capture_controllable()
        && load_policy(boundary).is_ok_and(|policy| policy.capture_enabled(store))
}

#[must_use]
pub fn recall_enabled(boundary: &Path, store: MemoryStore) -> bool {
    store.recall_controllable()
        && load_policy(boundary).is_ok_and(|policy| policy.recall_enabled(store))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_policy_enables_capture_and_recall() {
        let temp = tempfile::tempdir().unwrap();
        assert!(capture_enabled(temp.path(), MemoryStore::Facts));
        assert!(recall_enabled(temp.path(), MemoryStore::Facts));
    }

    #[test]
    fn store_override_roundtrips_and_replaces_on_second_save() {
        let temp = tempfile::tempdir().unwrap();
        let mut policy = MemoryPolicy::default();
        policy.set_capture(Some(MemoryStore::Facts), false);
        save_policy(temp.path(), &policy).unwrap();
        assert!(!capture_enabled(temp.path(), MemoryStore::Facts));
        assert!(capture_enabled(temp.path(), MemoryStore::Pitfalls));

        policy.set_recall(Some(MemoryStore::Facts), false);
        save_policy(temp.path(), &policy).unwrap();
        let loaded = load_policy(temp.path()).unwrap();
        assert!(!loaded.capture_enabled(MemoryStore::Facts));
        assert!(!loaded.recall_enabled(MemoryStore::Facts));
    }

    #[test]
    fn malformed_policy_disables_use_instead_of_ignoring_privacy_intent() {
        let temp = tempfile::tempdir().unwrap();
        let path = ensure_policy_path(temp.path()).unwrap();
        std::fs::write(path, "capture = maybe").unwrap();
        assert!(!capture_enabled(temp.path(), MemoryStore::Facts));
        assert!(!recall_enabled(temp.path(), MemoryStore::Facts));
    }

    #[test]
    fn derived_and_user_owned_stores_are_not_toggled_as_learned_memory() {
        let temp = tempfile::tempdir().unwrap();
        for store in [
            MemoryStore::KnowledgeIndex,
            MemoryStore::RepoMap,
            MemoryStore::SkillPackages,
            MemoryStore::DeletionAudit,
        ] {
            assert!(!capture_enabled(temp.path(), store));
        }
        assert!(!recall_enabled(temp.path(), MemoryStore::KnowledgeIndex));
        assert!(capture_enabled(temp.path(), MemoryStore::GateAdrs));
        assert!(!recall_enabled(temp.path(), MemoryStore::GateAdrs));
    }

    #[test]
    fn global_utility_capture_requires_an_explicit_opt_in() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!capture_enabled(temp.path(), MemoryStore::KnowledgeUtility));
        let mut policy = MemoryPolicy::default();
        policy.set_capture(Some(MemoryStore::KnowledgeUtility), true);
        save_policy(temp.path(), &policy).unwrap();
        assert!(capture_enabled(temp.path(), MemoryStore::KnowledgeUtility));
        assert!(recall_enabled(temp.path(), MemoryStore::KnowledgeUtility));
    }

    #[test]
    fn transactional_updates_do_not_lose_independent_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root_a = temp.path().to_path_buf();
        let root_b = root_a.clone();
        let first = std::thread::spawn(move || {
            update_policy(&root_a, |policy| {
                policy.set_capture(Some(MemoryStore::Facts), false);
                std::thread::sleep(std::time::Duration::from_millis(20));
                Ok(())
            })
            .unwrap();
        });
        let second = std::thread::spawn(move || {
            update_policy(&root_b, |policy| {
                policy.set_recall(Some(MemoryStore::Pitfalls), false);
                Ok(())
            })
            .unwrap();
        });
        first.join().unwrap();
        second.join().unwrap();

        let policy = load_policy(temp.path()).unwrap();
        assert!(!policy.capture_enabled(MemoryStore::Facts));
        assert!(!policy.recall_enabled(MemoryStore::Pitfalls));
        assert!(!temp.path().join(".umadev/memory/.policy.lock").exists());
    }

    #[test]
    fn stale_policy_lock_is_reclaimed_without_recursive_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let memory = ensure_memory_dir(temp.path()).unwrap();
        let lock = memory.join(POLICY_LOCK_DIR);
        std::fs::create_dir(&lock).unwrap();
        let owner = format!("{}\nstale-test", now_ms() - POLICY_LOCK_STALE_AFTER_MS - 1);
        crate::fs::atomic_write(&lock.join(POLICY_LOCK_OWNER), owner.as_bytes()).unwrap();

        update_policy(temp.path(), |policy| {
            policy.set_capture(Some(MemoryStore::Facts), false);
            Ok(())
        })
        .unwrap();
        assert!(!capture_enabled(temp.path(), MemoryStore::Facts));
        assert!(!lock.exists());
    }
}
