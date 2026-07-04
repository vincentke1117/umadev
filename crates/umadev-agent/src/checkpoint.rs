//! File-level checkpoint / rewind over the project workspace.
//!
//! Distinct from [`crate::state`] snapshots (which capture only the workflow
//! *phase*): a checkpoint is a full snapshot of the FILES the base wrote, so the
//! user can rewind a whole phase's worth of work — the feature every hot AI
//! coding tool ships (Cline shadow-git, Claude Code `/rewind`, Aider `/undo`).
//!
//! Implemented as a SHADOW git repo at `.umadev/checkpoints.git` (a separate
//! `--git-dir`, work-tree = project root) so it never touches the user's own
//! `.git` history. Every function is FAIL-OPEN: if `git` is missing or any step
//! fails it returns `None` / `Err` quietly and the pipeline is never blocked.
//!
//! This is the director's checkpoint — a phase-level safety net that composes
//! with (does not replace) a base CLI's own per-edit checkpointing.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Heavy paths excluded from checkpoints even without a project `.gitignore`, so
/// a checkpoint never tries to snapshot `node_modules`, build output, or the
/// user's real `.git`. Written to the shadow repo's `info/exclude` on init.
const SHADOW_EXCLUDES: &[&str] = &[
    "/.git/",
    "node_modules/",
    "target/",
    "dist/",
    "build/",
    ".next/",
    ".nuxt/",
    ".output/",
    ".venv/",
    "__pycache__/",
    ".umadev/",
    "*.log",
];

/// The same heavy/dangerous paths as [`SHADOW_EXCLUDES`], expressed as git
/// **exclude pathspecs** for the staging `git add`.
///
/// TRADE-OFF (P1/P2): the shadow `add` is now `-A --force` so it captures files
/// the project `.gitignore` hides — an AI-written `.env.local`, ignored generated
/// config, ignored snapshots — otherwise a "roll the whole run back" would
/// silently leave those behind (the base wrote them, but a plain `git add -A`
/// skips ignored paths, so they were never in a checkpoint to restore). `--force`
/// overrides BOTH the project `.gitignore` and the shadow `info/exclude`, so to
/// keep the shadow commit BOUNDED we re-assert the heavy set here as pathspecs:
/// the real `.git`, dependency trees (`node_modules`, `.venv`), Python caches,
/// build output (`target`/`dist`/`build`/`.next`/`.nuxt`/`.output`), log spew,
/// and UmaDev's own `.umadev/` (which holds THIS shadow repo — force-adding it
/// would be recursive). A user with some OTHER giant ignored artifact (a
/// multi-GB dataset) is out of scope — the common heavy offenders are covered.
const SHADOW_ADD_EXCLUDE_PATHSPECS: &[&str] = &[
    ":(exclude,glob).git/**",
    ":(exclude,glob)**/.git/**",
    ":(exclude,glob)node_modules/**",
    ":(exclude,glob)**/node_modules/**",
    ":(exclude,glob)target/**",
    ":(exclude,glob)**/target/**",
    ":(exclude,glob)dist/**",
    ":(exclude,glob)**/dist/**",
    ":(exclude,glob)build/**",
    ":(exclude,glob)**/build/**",
    ":(exclude,glob).next/**",
    ":(exclude,glob).nuxt/**",
    ":(exclude,glob).output/**",
    ":(exclude,glob).venv/**",
    ":(exclude,glob)**/__pycache__/**",
    ":(exclude,glob).umadev/**",
    ":(exclude,glob)**/.umadev/**",
    ":(exclude,glob)*.log",
];

/// Stage the whole work-tree into the shadow index — INCLUDING files the project
/// `.gitignore` hides (`--force`), so a rollback can restore an AI-written
/// `.env.local` / ignored config — while still excluding the heavy/dangerous
/// paths in [`SHADOW_ADD_EXCLUDE_PATHSPECS`] so the checkpoint stays bounded and
/// never pulls in `node_modules` / build output / the real `.git` / this shadow
/// repo. A leading positive `.` guards the "exclude-only pathspec" edge across
/// git versions. Fail-open: `None` when `git` can't spawn (the caller
/// `?`-propagates, exactly as the old `git add -A` did).
fn stage_all_including_ignored(project_root: &Path) -> Option<std::process::Output> {
    let mut args: Vec<&str> = vec!["add", "-A", "--force", "--", "."];
    args.extend_from_slice(SHADOW_ADD_EXCLUDE_PATHSPECS);
    git(project_root, &args)
}

/// One checkpoint entry, newest-first in [`list_checkpoints`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Checkpoint {
    /// Short commit id — the handle passed to [`restore_checkpoint`].
    pub id: String,
    /// Human label (usually the phase or reason).
    pub label: String,
    /// ISO-8601 commit time.
    pub when: String,
}

/// Path to the shadow git directory.
fn git_dir(project_root: &Path) -> PathBuf {
    project_root.join(".umadev").join("checkpoints.git")
}

/// Run a shadow-git command (work-tree = project root). `None` if `git` can't be
/// spawned at all.
fn git(project_root: &Path, args: &[&str]) -> Option<std::process::Output> {
    Command::new("git")
        .arg("--git-dir")
        .arg(git_dir(project_root))
        .arg("--work-tree")
        .arg(project_root)
        .args(args)
        .output()
        .ok()
}

/// `true` when at least one checkpoint exists.
#[must_use]
pub fn has_checkpoints(project_root: &Path) -> bool {
    git_dir(project_root).join("HEAD").exists()
        && git(project_root, &["rev-parse", "--verify", "-q", "HEAD"])
            .is_some_and(|o| o.status.success())
}

/// Initialise the shadow repo on first use. Returns `false` (fail-open) when
/// `git` is missing or init fails.
fn ensure_init(project_root: &Path) -> bool {
    let gd = git_dir(project_root);
    if gd.join("HEAD").exists() {
        return true;
    }
    if std::fs::create_dir_all(&gd).is_err() {
        return false;
    }
    let ok = git(project_root, &["init", "-q"]).is_some_and(|o| o.status.success());
    if ok {
        // The shadow repo's own ignore file — keeps heavy dirs out of every
        // checkpoint regardless of whether the project ships a `.gitignore`.
        let info = gd.join("info");
        let _ = std::fs::create_dir_all(&info);
        let _ = std::fs::write(info.join("exclude"), SHADOW_EXCLUDES.join("\n"));
    }
    ok
}

/// Snapshot the whole workspace as a checkpoint labelled `label`. Returns the
/// short commit id, or `None` (fail-open) if `git` is unavailable. An empty
/// snapshot (nothing changed) still produces a checkpoint via `--allow-empty`.
#[must_use]
pub fn create_checkpoint(project_root: &Path, label: &str) -> Option<String> {
    if !ensure_init(project_root) {
        return None;
    }
    stage_all_including_ignored(project_root)?;
    git(
        project_root,
        &[
            "-c",
            "user.email=umadev@local",
            "-c",
            "user.name=UmaDev",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            label,
        ],
    )?;
    let head = git(project_root, &["rev-parse", "--short", "HEAD"])?;
    head.status
        .success()
        .then(|| String::from_utf8_lossy(&head.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Snapshot the workspace at a PHASE boundary, but ONLY if the working tree
/// changed since the last checkpoint — an empty phase boundary produces no new
/// checkpoint. This is what the runner calls on every phase start so the
/// headless `run` / `continue` paths get the same per-phase rewind points the
/// TUI builds from its `PhaseStarted` handler, WITHOUT cluttering the rewind
/// list with a run of identical empty commits when several phases start
/// back-to-back before any file is written (e.g. the docs→spec gap).
///
/// Returns `Some(id)` when a NEW checkpoint was written, `None` when nothing
/// changed (or `git` is unavailable — fail-open, never blocks the pipeline).
/// The very first checkpoint of a fresh shadow repo is always written (so a
/// run always has a baseline to rewind to), even if the tree looks "clean"
/// relative to an empty HEAD.
#[must_use]
pub fn create_phase_checkpoint(project_root: &Path, label: &str) -> Option<String> {
    if !ensure_init(project_root) {
        return None;
    }
    // Stage everything (including .gitignore'd product files, minus the heavy
    // set — see `stage_all_including_ignored`), then ask whether the index
    // differs from HEAD. `git diff --cached --quiet` exits 0 = no staged changes,
    // 1 = changes. When there is no HEAD yet (fresh repo) the diff reports changes
    // (or errors), so the baseline checkpoint is taken.
    stage_all_including_ignored(project_root)?;
    let has_head = has_checkpoints(project_root);
    if has_head {
        let clean =
            git(project_root, &["diff", "--cached", "--quiet"]).is_some_and(|o| o.status.success());
        if clean {
            return None; // nothing changed since the last checkpoint
        }
    }
    git(
        project_root,
        &[
            "-c",
            "user.email=umadev@local",
            "-c",
            "user.name=UmaDev",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            label,
        ],
    )?;
    let head = git(project_root, &["rev-parse", "--short", "HEAD"])?;
    head.status
        .success()
        .then(|| String::from_utf8_lossy(&head.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// List checkpoints, newest first (capped at 50). Empty when none / `git`
/// missing.
#[must_use]
pub fn list_checkpoints(project_root: &Path) -> Vec<Checkpoint> {
    // `--all` so checkpoints that a rewind moved off the linear HEAD history
    // (the pre-rewind snapshot + any forward checkpoints, preserved under a
    // `umadev-saved-*` ref by restore_checkpoint) are still listed.
    let Some(out) = git(
        project_root,
        &["log", "--all", "--pretty=%h%x1f%cI%x1f%s", "-n", "50"],
    ) else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\u{1f}');
            Some(Checkpoint {
                id: parts.next()?.to_string(),
                when: parts.next()?.to_string(),
                label: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// `true` when `id` names one of the checkpoints in `list` — the ONLY commits
/// `restore_checkpoint` may reset to. Accepts an exact match or an unambiguous
/// abbreviation in EITHER direction (the caller's `id` is a prefix of a listed
/// short id, OR a listed short id is a prefix of a fuller `id` the caller pasted)
/// so a user can pass the short handle we printed or a longer SHA. Rejects
/// everything else — `HEAD~999`, arbitrary refs/branches, an unrelated SHA — so a
/// stray handle can never `reset --hard` the work-tree to an unintended commit and
/// silently discard files. An empty `id` matches nothing.
fn id_is_known_checkpoint(id: &str, list: &[Checkpoint]) -> bool {
    let id = id.trim();
    if id.is_empty() {
        return false;
    }
    list.iter().any(|c| {
        let cid = c.id.as_str();
        cid == id || cid.starts_with(id) || id.starts_with(cid)
    })
}

/// Rewind the workspace files to checkpoint `id` (`git reset --hard`). The
/// CURRENT state is auto-checkpointed first, so a rewind is itself undoable.
/// Untracked / excluded paths (`node_modules`, …) are left untouched.
///
/// `id` MUST name a checkpoint returned by [`list_checkpoints`]; it is validated
/// against that set BEFORE any reset (P1-3). An arbitrary git ref (`HEAD~999`, a
/// branch, an unrelated SHA) is refused rather than reset-to, so a malformed
/// handle can never `reset --hard` the work-tree to an unintended commit and
/// destroy uncommitted work.
///
/// # Errors
/// Returns `Err` with a human message when no checkpoints exist, `git` is
/// unavailable, or the id is not a known checkpoint.
pub fn restore_checkpoint(project_root: &Path, id: &str) -> Result<(), String> {
    if !has_checkpoints(project_root) {
        return Err("还没有任何检查点可回滚".to_string());
    }
    // P1-3: validate `id` against the known-checkpoint set BEFORE doing anything
    // destructive. `git reset --hard <id>` accepts ANY revision (HEAD~999, a
    // branch, a random SHA), so without this gate a bad handle would silently
    // rewind the work-tree to an unintended commit and drop files. We only allow
    // ids that `list_checkpoints` actually surfaced.
    let known = list_checkpoints(project_root);
    if !id_is_known_checkpoint(id, &known) {
        return Err(format!(
            "未知的检查点 id「{id}」—— 它不在可回滚的检查点列表中"
        ));
    }
    // Make the rewind reversible AND keep forward checkpoints reachable: snapshot
    // the present, then anchor that snapshot (whose history includes every
    // checkpoint newer than `id`) under a `umadev-saved-<sha>` ref BEFORE the
    // reset. Otherwise `reset --hard` would orphan them and `list_checkpoints`
    // (which uses `git log --all`) couldn't show them.
    let _ = create_checkpoint(project_root, &format!("auto: 回滚到 {id} 前的现场"));
    if let Some(sha) = git(project_root, &["rev-parse", "--short", "HEAD"])
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
    {
        let branch = format!("umadev-saved-{sha}");
        let _ = git(project_root, &["branch", "-f", &branch, "HEAD"]);
    }
    let out = git(project_root, &["reset", "--hard", id]).ok_or("git 不可用")?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

// =====================================================================
// Run-level rollback — "undo the whole run" over the SHADOW repo
// (Wave 6 deliverable 2: real-git rollback, decoupled from the user's .git)
// =====================================================================

/// The label prefix that marks a **run baseline** checkpoint — the snapshot of
/// the workspace taken the instant a workspace-mutating run begins, BEFORE the
/// base writes anything. `rollback` rewinds to the newest of these, so the user
/// can undo an entire run's worth of edits in one move.
///
/// Kept distinct from ordinary per-phase checkpoints (which use a `phase: …` /
/// freeform label) so [`run_baseline`] can find it deterministically.
pub const RUN_BASELINE_PREFIX: &str = "run-baseline: ";

/// Take a **run-baseline** checkpoint — the pre-run snapshot `rollback` rewinds
/// to. Call this once at the very start of a workspace-mutating run, before any
/// phase writes. It is a normal shadow-repo checkpoint with a recognisable
/// label, so it composes with the existing per-phase rewind points and never
/// touches the user's own `.git`. Fail-open: `None` when `git` is unavailable.
///
/// `slug` only flavours the label; the baseline is identified by its prefix.
#[must_use]
pub fn create_run_baseline(project_root: &Path, slug: &str) -> Option<String> {
    let label = format!("{RUN_BASELINE_PREFIX}{slug}");
    create_checkpoint(project_root, &label)
}

/// The newest run-baseline checkpoint, if one exists — the target `rollback`
/// rewinds to. Newest-first scan of [`list_checkpoints`] for the baseline
/// prefix. `None` when no run has recorded a baseline yet.
#[must_use]
pub fn run_baseline(project_root: &Path) -> Option<Checkpoint> {
    list_checkpoints(project_root)
        .into_iter()
        .find(|c| c.label.starts_with(RUN_BASELINE_PREFIX))
}

/// Ensure THIS run has its own fresh run-baseline — the pre-run snapshot
/// `rollback` rewinds to — so a rollback only ever reverts the CURRENT run's
/// changes, never a prior run's (P1/P2).
///
/// The old logic took a baseline only when `run_baseline().is_none()`, i.e. once
/// per WORKSPACE: a second run in the same workspace reused the first run's
/// baseline, so rolling back the second run over-reverted everything since the
/// first run started. Here we instead take a **fresh** baseline at each new run's
/// start, while staying idempotent WITHIN a single run:
///
/// - If the newest run-baseline is ALSO the shadow repo's current HEAD, we are
///   still sitting at this run's start — no phase checkpoint has been committed
///   since the baseline was taken (e.g. the legacy `run_clarify` → `run_initial_block`
///   hand-off calls this twice at the Research boundary). Re-use it; do NOT stack
///   a second baseline that would move `rollback`'s target to mid-run state.
/// - Otherwise (no baseline yet, OR HEAD has advanced past the last baseline
///   because a prior run committed phase checkpoints) this is a NEW run's start:
///   snapshot fresh so `rollback` targets only this run.
///
/// Fail-open: `git` unavailable → `None`, never blocks the run. `slug` only
/// flavours the label; the baseline is identified by [`RUN_BASELINE_PREFIX`].
#[must_use]
pub fn ensure_run_baseline(project_root: &Path, slug: &str) -> Option<String> {
    if let Some(existing) = run_baseline(project_root) {
        if baseline_is_current_head(project_root, &existing.id) {
            // Still at this run's start (nothing checkpointed since) → keep it.
            return Some(existing.id);
        }
    }
    create_run_baseline(project_root, slug)
}

/// `true` when `baseline_id` names the shadow repo's current HEAD commit — i.e.
/// no checkpoint has been committed since that baseline was taken, so we are
/// still at the run's start rather than mid-run. Prefix-tolerant in either
/// direction (short-id abbreviations, like [`id_is_known_checkpoint`]).
///
/// Fail-open: an unreadable HEAD reads as "not current" so [`ensure_run_baseline`]
/// takes a fresh baseline — the safe direction (never silently reuse a stale one).
fn baseline_is_current_head(project_root: &Path, baseline_id: &str) -> bool {
    let Some(head) = git(project_root, &["rev-parse", "--short", "HEAD"])
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return false;
    };
    let id = baseline_id.trim();
    !id.is_empty() && (id == head || id.starts_with(&head) || head.starts_with(id))
}

/// Roll the workspace back to the most recent **run baseline** — a true,
/// reversible "undo this whole run". This is the user-facing `rollback`:
///
/// - Rewinds tracked files to the pre-run snapshot via the SHADOW repo (so the
///   user's own `.git` history is never rewritten — only the working-tree files
///   the run wrote are reverted).
/// - Is itself undoable: [`restore_checkpoint`] auto-snapshots the present and
///   anchors forward checkpoints under a `umadev-saved-*` ref first, so a
///   rollback can be rolled forward again.
/// - Leaves untracked/excluded paths (`node_modules`, the user's `.git`, the
///   isolation branch pointer) untouched.
///
/// Fail-open by contract: when no run baseline exists (no run has started, or
/// `git` is unavailable) it returns an actionable `Err` string and changes
/// nothing — it NEVER deletes a remote, force-pushes, or rewrites user commits.
///
/// # Errors
/// Returns `Err` with a human message when there is no recorded baseline to
/// roll back to, or the underlying restore fails.
pub fn rollback_run(project_root: &Path) -> Result<Checkpoint, String> {
    let baseline = run_baseline(project_root)
        .ok_or_else(|| "还没有可回滚的运行基线(本工作区尚未开始过会改动文件的运行)".to_string())?;
    restore_checkpoint(project_root, &baseline.id)?;
    Ok(baseline)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn create_list_restore_roundtrip() {
        if !git_available() {
            return; // fail-open environment without git — nothing to assert
        }
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), "v1").unwrap();
        let c1 = create_checkpoint(root, "phase: frontend").expect("checkpoint 1");

        // Mutate + add a file, then checkpoint again.
        std::fs::write(root.join("a.txt"), "v2").unwrap();
        std::fs::write(root.join("b.txt"), "new").unwrap();
        let _c2 = create_checkpoint(root, "phase: backend").expect("checkpoint 2");

        let list = list_checkpoints(root);
        assert_eq!(list.len(), 2, "two checkpoints, newest first");
        assert_eq!(list[0].label, "phase: backend");

        // Rewind to c1: a.txt back to v1, b.txt removed.
        restore_checkpoint(root, &c1).expect("restore");
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "v1");
        assert!(!root.join("b.txt").exists(), "file added after c1 is gone");

        // The rewind must NOT lose history: the forward checkpoint ("phase:
        // backend") and the auto pre-rewind snapshot are still listed (preserved
        // under a umadev-saved-* ref), so the rewind itself stays undoable.
        let after = list_checkpoints(root);
        assert!(
            after.iter().any(|c| c.label == "phase: backend"),
            "forward checkpoint survives the rewind"
        );
        assert!(
            after.iter().any(|c| c.label.contains("回滚到")),
            "pre-rewind snapshot is reachable"
        );
    }

    #[test]
    fn phase_checkpoint_baselines_then_skips_unchanged() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), "v1").unwrap();
        // First phase boundary → baseline checkpoint is taken even though there's
        // no prior HEAD.
        let c1 = create_phase_checkpoint(root, "phase: research");
        assert!(c1.is_some(), "first phase checkpoint is the baseline");
        // A second phase boundary with NO file change → no new checkpoint (the
        // headless run starts several phases back-to-back before any file lands;
        // we must not clutter the rewind list with identical empty commits).
        let c2 = create_phase_checkpoint(root, "phase: docs");
        assert!(c2.is_none(), "unchanged tree yields no new checkpoint");
        assert_eq!(list_checkpoints(root).len(), 1, "still one checkpoint");
        // Now the base writes a file → the next phase boundary DOES checkpoint.
        std::fs::write(root.join("b.txt"), "new").unwrap();
        let c3 = create_phase_checkpoint(root, "phase: frontend");
        assert!(c3.is_some(), "a changed tree produces a new checkpoint");
        assert_eq!(list_checkpoints(root).len(), 2);
    }

    #[test]
    fn restore_without_checkpoints_errors() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        assert!(restore_checkpoint(tmp.path(), "deadbeef").is_err());
    }

    #[test]
    fn has_checkpoints_false_on_empty() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        assert!(!has_checkpoints(tmp.path()));
    }

    #[test]
    fn restore_rejects_unknown_id_without_resetting() {
        // P1-3: an id that is NOT a known checkpoint must be refused BEFORE any
        // `git reset --hard`, so a stray ref (HEAD~999, a random SHA) can never
        // rewind the work-tree to an unintended commit and discard files.
        if !git_available() {
            return;
        }
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), "v1").unwrap();
        let _c1 = create_checkpoint(root, "phase: frontend").expect("checkpoint");
        // Mutate the live tree AFTER the checkpoint — this is the work an out-of-
        // range reset would have destroyed.
        std::fs::write(root.join("a.txt"), "v2-live-work").unwrap();

        // An arbitrary ref that git WOULD accept for `reset --hard` but is not a
        // known checkpoint → refused.
        for bad in ["HEAD~999", "deadbeef", "main", "refs/heads/whatever", ""] {
            let r = restore_checkpoint(root, bad);
            assert!(r.is_err(), "unknown id {bad:?} must be refused");
        }
        // The live work is intact — no reset happened.
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "v2-live-work",
            "a refused restore must not touch the work-tree"
        );
    }

    #[test]
    fn rollback_run_rewinds_whole_run_to_baseline() {
        // Wave 6: `rollback` = real-git undo of the entire run, over the shadow
        // repo (the user's own `.git` is never touched).
        if !git_available() {
            return;
        }
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        // Pre-run state.
        std::fs::write(root.join("src.rs"), "original").unwrap();
        // Run start → baseline snapshot.
        let baseline = create_run_baseline(root, "demo").expect("baseline");
        assert!(run_baseline(root).is_some(), "baseline is findable");
        // The run writes a couple of phases' worth of edits.
        std::fs::write(root.join("src.rs"), "rewritten by the run").unwrap();
        std::fs::write(root.join("new_file.rs"), "added by the run").unwrap();
        let _ = create_phase_checkpoint(root, "phase: frontend");
        std::fs::write(root.join("another.rs"), "more run output").unwrap();
        let _ = create_phase_checkpoint(root, "phase: backend");
        // Roll the WHOLE run back.
        let rolled = rollback_run(root).expect("rollback to baseline");
        assert!(
            rolled.id == baseline
                || baseline.starts_with(&rolled.id)
                || rolled.id.starts_with(&baseline)
        );
        // Pre-run file restored, run-added files gone.
        assert_eq!(
            std::fs::read_to_string(root.join("src.rs")).unwrap(),
            "original"
        );
        assert!(!root.join("new_file.rs").exists(), "run-added file removed");
        assert!(!root.join("another.rs").exists(), "run-added file removed");
        // The rollback is itself undoable: forward checkpoints survive under a
        // saved ref (so list still shows the run's work).
        let after = list_checkpoints(root);
        assert!(
            after.iter().any(|c| c.label == "phase: backend"),
            "forward run checkpoints survive the rollback (it stays reversible)"
        );
    }

    #[test]
    fn rollback_run_without_baseline_errors_and_changes_nothing() {
        // Fail-open: no baseline → an actionable error, never a destructive op.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("a.txt"), "untouched").unwrap();
        let r = rollback_run(root);
        assert!(r.is_err(), "no baseline → Err, not a silent reset");
        // The working tree was not touched.
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "untouched"
        );
    }

    #[test]
    fn run_baseline_picks_newest_and_ignores_phase_checkpoints() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("f.txt"), "1").unwrap();
        let _ = create_run_baseline(root, "first-run");
        std::fs::write(root.join("f.txt"), "2").unwrap();
        let _ = create_phase_checkpoint(root, "phase: spec");
        std::fs::write(root.join("f.txt"), "3").unwrap();
        let second = create_run_baseline(root, "second-run").expect("second baseline");
        // The newest baseline is the rollback target, not the phase checkpoint.
        let target = run_baseline(root).expect("a baseline exists");
        assert!(target.label.starts_with(RUN_BASELINE_PREFIX));
        assert!(target.label.contains("second-run"));
        assert!(
            second.starts_with(&target.id) || target.id.starts_with(&second) || second == target.id
        );
    }

    #[test]
    fn ensure_run_baseline_is_fresh_per_run_and_deduped_within_a_run() {
        // P1/P2: a 2nd run must take its OWN baseline (not reuse the 1st run's), so
        // `rollback` reverts ONLY the current run's changes — while staying
        // idempotent within one run (a re-entry block at the same start does not
        // stack a second baseline).
        if !git_available() {
            return;
        }
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        std::fs::write(root.join("src.rs"), "pre-run-1").unwrap();

        // Run 1 start → the first baseline.
        let b1 = ensure_run_baseline(root, "run1").expect("run1 baseline");
        // A SECOND setup call at the same run's start (no phase checkpoint yet) must
        // NOT stack a new baseline — it reuses b1 (still sitting at the run start).
        let b1_again = ensure_run_baseline(root, "run1").expect("dedup within run");
        assert!(
            b1_again == b1 || b1.starts_with(&b1_again) || b1_again.starts_with(&b1),
            "within one run the baseline is re-used, not re-taken"
        );

        // Run 1 writes + checkpoints a phase → shadow HEAD moves past the baseline.
        std::fs::write(root.join("src.rs"), "run-1-output").unwrap();
        std::fs::write(root.join("run1_file.rs"), "added by run 1").unwrap();
        let _ = create_phase_checkpoint(root, "phase: frontend");

        // Run 2 start → a FRESH baseline (not run 1's), capturing run 1's output.
        let b2 = ensure_run_baseline(root, "run2").expect("run2 baseline");
        assert!(
            !(b2 == b1 || b1.starts_with(&b2) || b2.starts_with(&b1)),
            "a 2nd run takes its OWN baseline, not run 1's"
        );
        assert!(
            run_baseline(root).unwrap().label.contains("run2"),
            "the newest run baseline is run 2's"
        );

        // Run 2 writes.
        std::fs::write(root.join("src.rs"), "run-2-output").unwrap();
        std::fs::write(root.join("run2_file.rs"), "added by run 2").unwrap();
        let _ = create_phase_checkpoint(root, "phase: backend");

        // Rollback reverts ONLY run 2 (back to run 1's output), never run 1.
        let _ = rollback_run(root).expect("rollback run 2");
        assert_eq!(
            std::fs::read_to_string(root.join("src.rs")).unwrap(),
            "run-1-output",
            "rollback reverts run 2 to run 1's output, not to before run 1"
        );
        assert!(
            !root.join("run2_file.rs").exists(),
            "run 2's added file is removed by the rollback"
        );
        assert!(
            root.join("run1_file.rs").exists(),
            "run 1's file SURVIVES — only the current run was reverted"
        );
    }

    #[test]
    fn checkpoint_captures_and_restores_gitignored_files() {
        // P1/P2: the shadow checkpoint force-adds the project's .gitignore'd product
        // files (an AI-written `.env.local`) so a rollback RESTORES them — but still
        // excludes heavy dirs so the shadow commit can't balloon.
        if !git_available() {
            return;
        }
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        // A project .gitignore that hides .env.local AND node_modules.
        std::fs::write(root.join(".gitignore"), "node_modules/\n.env.local\n").unwrap();
        std::fs::write(root.join("app.js"), "v1").unwrap();
        std::fs::write(root.join(".env.local"), "SECRET=original").unwrap();
        std::fs::create_dir_all(root.join("node_modules").join("pkg")).unwrap();
        std::fs::write(root.join("node_modules").join("pkg").join("i.js"), "heavy").unwrap();

        let c1 = create_checkpoint(root, "baseline").expect("checkpoint");
        // The ignored .env.local WAS captured; the heavy node_modules was NOT.
        let tracked = git(root, &["ls-files"])
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        assert!(
            tracked.lines().any(|l| l == ".env.local"),
            "the .gitignore'd .env.local is captured in the checkpoint"
        );
        assert!(
            !tracked.lines().any(|l| l.contains("node_modules")),
            "the heavy node_modules dir stays out of the checkpoint"
        );

        // The base overwrites the ignored file, then we roll back to c1.
        std::fs::write(root.join(".env.local"), "SECRET=rewritten-by-ai").unwrap();
        let _c2 = create_checkpoint(root, "after edit");
        restore_checkpoint(root, &c1).expect("restore");
        assert_eq!(
            std::fs::read_to_string(root.join(".env.local")).unwrap(),
            "SECRET=original",
            "rollback restored the .gitignore'd file the base had overwritten"
        );
    }

    #[test]
    fn id_is_known_checkpoint_matches_prefixes_both_directions() {
        let list = vec![Checkpoint {
            id: "abc1234".to_string(),
            label: "x".to_string(),
            when: "t".to_string(),
        }];
        // Exact, listed-id-as-prefix-of-input (full SHA), input-as-prefix-of-id.
        assert!(id_is_known_checkpoint("abc1234", &list));
        assert!(id_is_known_checkpoint("abc1234def567", &list)); // fuller SHA
        assert!(id_is_known_checkpoint("abc", &list)); // abbreviation
                                                       // Unrelated / empty → rejected.
        assert!(!id_is_known_checkpoint("zzz9999", &list));
        assert!(!id_is_known_checkpoint("", &list));
        assert!(!id_is_known_checkpoint("HEAD~5", &list));
    }
}
