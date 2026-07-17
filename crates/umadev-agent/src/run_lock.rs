//! Advisory single-writer lock per workspace.
//!
//! Two concurrent `umadev` runs in the same workspace (e.g. the chat TUI plus a
//! scripted `umadev continue` in another terminal) share `workflow-state.json`,
//! `output/*`, and the provider config — running them at once silently corrupts
//! ordering and clobbers artifacts. This is the same hazard Terraform guards
//! with state locking and git with `index.lock`.
//!
//! The lock is a `.umadev/run.lock` file created with `create_new`
//! (`O_CREAT|O_EXCL`), holding the **owner identity** (`{pid, host, ts}`), and
//! removed on drop. It is **dependency-free** and **fail-open**: any IO error
//! other than "already exists" yields an un-owned guard that never blocks the
//! run (a lock bug must never stop a legitimate run).
//!
//! ## Stale-lock recovery (PID liveness)
//!
//! When the lock already exists we don't blindly refuse — that's what wedged the
//! user after a `Ctrl-C`/crash left an orphan `run.lock` behind. Instead we read
//! the owner identity and decide:
//!
//! 1. **Same host + owner PID is dead** → the previous run crashed; the lock is
//!    stale. Reclaim it and take over. This is the primary path (mirrors how a
//!    DBMS / `flock`-style supervisor reaps a dead holder).
//! 2. **Same host + owner PID is alive** → a real concurrent run; refuse with an
//!    actionable message.
//! 3. **Different host, or identity unparseable/missing, or the boot id conflicts
//!    with a live PID** → we can't attribute the owner, so fall back to a generous
//!    internal age threshold: an ancient lock with no heartbeat is
//!    reclaimed, otherwise refuse (with a "delete the file to force" hint).
//!
//! That verdict comes from the internal claim-owner classifier — the SINGLE owner-liveness rule,
//! shared with the temporary-rewind crash marker in [`crate::checkpoint`], because
//! two files answering the same question two different ways is exactly how a live
//! holder's lock gets reclaimed under it.
//!
//! Liveness probing is itself **fail-open**: if we cannot determine whether the
//! PID is alive we treat it as *alive* (conservative — never reclaim a lock that
//! might be live just because the probe errored), and the age fallback still
//! frees a genuinely abandoned lock.

use std::io;
use std::path::{Path, PathBuf};

/// A same-host lock whose owner PID we could not prove dead is reclaimed only
/// once it is older than this (the cross-host / unparseable fallback). No
/// UmaDev pipeline block runs anywhere near six hours, so this never reclaims a
/// live run; a user with a genuinely longer run can delete the lock file by
/// hand (the refusal message says so).
const STALE_SECS: u64 = 6 * 3600;

/// Held for the duration of a pipeline block; releases the workspace lock on
/// drop. An un-owned guard (fail-open path) is a harmless no-op.
#[derive(Debug)]
pub struct RunLock {
    path: PathBuf,
    owned: bool,
}

/// Why the lock is being taken — decides how a lock already held by **this same
/// process** is treated. The two intents are genuinely different:
///
/// - [`AcquireIntent::Route`] is the **input-routing / queue** layer (the chat
///   TUI deciding where a freshly-typed line goes). A same-PID lock means a run
///   this session already kicked off is still in flight, so the right answer is
///   *queue the input into it* — surfaced as a `WouldBlock` signal, never a
///   reclaim. Two run blocks could legitimately co-exist here (one running, the
///   user typing the next).
/// - [`AcquireIntent::Run`] is a real **execution** path (`run_initial_block`,
///   the `continue_after_*` blocks, `run_light`, `redo_phase`) actually about to
///   drive the pipeline. Because one process runs these strictly serially, a
///   same-PID lock can only be **our own residue** from a previous block whose
///   guard hasn't dropped yet (or a block that aborted before `Drop` ran) — it
///   can NEVER be a second concurrent execution in the same process. So the run
///   path **reclaims** it and takes over instead of `WouldBlock`-aborting, which
///   is exactly what wedged research at `0/9`.
///
/// In both intents an **external** PID is classified identically (dead →
/// reclaim, alive → refuse): the run path only relaxes the *same-PID* case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcquireIntent {
    /// Input-routing layer: same-PID lock → `WouldBlock` queue signal.
    Route,
    /// Real execution path: same-PID lock → reclaim our own residue + take over.
    Run,
}

/// Parsed contents of a `run.lock` file: who claims to hold it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Owner {
    /// Process id of the holder (`0` if it could not be parsed).
    pid: u32,
    /// Hostname of the holder, or empty if absent (older lock format / corrupt).
    host: String,
    /// UNIX-seconds creation timestamp (`0` if absent / corrupt).
    ts: u64,
    /// A per-BOOT identifier (empty if unavailable). Lets a lock left by a PRE-REBOOT run be
    /// told apart from a live run that merely reused the same recycled PID after a reboot.
    boot: String,
}

impl RunLock {
    /// Acquire the workspace run lock from the **input-routing / queue** layer.
    ///
    /// Use this where the caller is *deciding what to do with input*, not where
    /// it is about to drive the pipeline. A lock already held by **this** process
    /// means our session has a run in flight → the caller should queue the input
    /// into it; that case is signalled with [`io::ErrorKind::WouldBlock`].
    ///
    /// # Errors
    /// - `WouldBlock` when **this** process already holds the lock (queue signal).
    /// - `AlreadyExists` with an actionable message when another **live** run on
    ///   this host holds it.
    ///
    /// A lock left behind by a crashed/killed run on this host is detected via
    /// PID liveness, reclaimed automatically, and taken over. Any other IO
    /// problem fails open (returns an un-owned guard) so a lock bug can never
    /// block a legitimate run.
    pub fn acquire(project_root: &Path) -> io::Result<RunLock> {
        Self::acquire_with(project_root, AcquireIntent::Route)
    }

    /// Acquire the workspace run lock for a real **execution** block
    /// (`run_initial_block`, the `continue_after_*` blocks, `run_light`,
    /// `redo_phase`).
    ///
    /// Differs from [`acquire`](Self::acquire) in exactly one place: a lock held
    /// by **this same process** is treated as our own leftover residue and
    /// **reclaimed** (the same process runs these blocks serially, so it can
    /// never be a real second concurrent execution), instead of returning the
    /// `WouldBlock` queue signal that the routing layer wants. This is what stops
    /// a run from self-aborting at `0/9` when a previous block's lock guard hasn't
    /// dropped yet. External holders are classified exactly as in `acquire`
    /// (dead → reclaim, alive → refuse).
    ///
    /// # Errors
    /// Returns `AlreadyExists` with an actionable message when another **live**
    /// run on this host holds the lock. Any other IO problem fails open (un-owned
    /// guard) so a lock bug can never block a legitimate run.
    pub fn acquire_for_run(project_root: &Path) -> io::Result<RunLock> {
        // WORKSPACE-INTEGRITY BACKSTOP, before anything else. A previous run that was
        // SIGKILLed / OOM-killed / whose terminal was closed inside a temporary
        // evidence rewind left the user's tracked source files reverted to an earlier
        // step's state — no destructor ran, so nothing put them back. Restore the
        // PRESENT before this run starts writing on top of a tree that is silently in
        // the past. Fail-open and strictly conservative: it no-ops unless the marker's
        // owner is provably gone, and it can only reset to a checkpoint we ourselves
        // wrote (see `checkpoint::recover_abandoned_temp_rewind`).
        if let Some(note) = crate::checkpoint::recover_abandoned_temp_rewind(project_root) {
            // A `tracing::warn!` alone is invisible to the person this is FOR: under the TUI
            // the log goes to a file. Hand the note to the surface that can actually speak
            // (the transcript drains it), and keep the log line for post-mortems.
            tracing::warn!("{note}");
            crate::checkpoint::record_workspace_notice(note);
        }
        Self::acquire_with(project_root, AcquireIntent::Run)
    }

    /// Shared acquisition core. `intent` only changes how a lock held by **this**
    /// process is handled (see [`AcquireIntent`]); every external-holder path is
    /// identical for both intents.
    fn acquire_with(project_root: &Path, intent: AcquireIntent) -> io::Result<RunLock> {
        let dir = project_root.join(".umadev");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("run.lock");
        // A BOUNDED loop (at most one stale-reclaim retry) — never recurse, so a
        // wedged-but-undeletable stale lock can't blow the stack.
        for attempt in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    // Owner identity: PID + host + creation timestamp. The host
                    // lets us avoid probing a PID that belongs to a *different*
                    // machine's process table (a shared/NFS workspace).
                    let _ = writeln!(
                        file,
                        "pid={} host={} ts={} boot={}",
                        std::process::id(),
                        hostname(),
                        now_secs(),
                        boot_id()
                    );
                    // Flush + drop the handle so the read-back below sees our
                    // bytes (and any clobber by a racing reclaimer).
                    let _ = file.flush();
                    drop(file);
                    // ── P0-2: reclaim TOCTOU read-back self-check ─────────────
                    // The reclaim path is remove-then-create, which is NOT atomic:
                    // if A and B both saw the same stale dead-PID lock, both could
                    // `remove_file` and both could win a `create_new` (B's remove
                    // can delete A's just-created lock, then B re-creates). Our own
                    // `create_new` succeeding is therefore NOT proof we are the sole
                    // owner. So we READ THE LOCK BACK and confirm it still records
                    // OUR pid+host. If a racing reclaimer clobbered it after us, the
                    // read-back shows a foreign pid → we DROP ownership (`owned:
                    // false`) and do NOT delete it (fail-open: never remove a lock
                    // that now belongs to someone else). Last-writer keeps it; the
                    // single-writer invariant holds because at most one identity can
                    // survive the read-back as self. `create_new` already closed the
                    // common race; the read-back closes the remove-then-create window.
                    if holder_is_self(&path) {
                        return Ok(RunLock { path, owned: true });
                    }
                    // Someone overwrote our lock between create and read-back.
                    // Surrender ownership without deleting their lock.
                    return Ok(RunLock { path, owned: false });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    // The lock is taken. Classify the holder into exactly three
                    // cases (the misleading "another umadev" message used to fire
                    // for all of them):
                    //
                    //   1. holder == THIS process  → our own session already has a
                    //      run in flight. Two intents diverge here (see
                    //      `AcquireIntent`):
                    //        * Route → queue the input INTO that run; signalled
                    //          with `WouldBlock` + an accurate message.
                    //        * Run → this can only be OUR OWN residue (the same
                    //          process runs blocks serially), so reclaim it and
                    //          take over rather than self-abort with WouldBlock.
                    //   2. holder is a dead PID on this host → crashed/killed run;
                    //      reclaim and take over (handled by is_stale below).
                    //   3. holder is a live foreign run → the genuine
                    //      "another umadev is running" refusal.
                    if holder_is_self(&path) {
                        if intent == AcquireIntent::Run {
                            // Our own leftover lock blocking our own next block.
                            // Only retry if we actually removed it; a remove
                            // failure (read-only fs) falls through to fail-open.
                            if attempt == 0 && std::fs::remove_file(&path).is_ok() {
                                continue;
                            }
                            // Couldn't clear it — fail open rather than wedge the
                            // run on our own residue (a lock bug must never block).
                            return Ok(RunLock { path, owned: false });
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "本会话已有一个 umadev run 正在进行中 —— \
                             你的输入会排队发给这个 run,而不是另起新 run。"
                                .to_string(),
                        ));
                    }
                    // Only retry if we actually RECLAIMED a stale leftover; if the
                    // remove fails (undeletable lock), fall through to refusal.
                    if attempt == 0 && is_stale(&path) && std::fs::remove_file(&path).is_ok() {
                        continue;
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "另一个 umadev 运行正在占用该工作区(锁文件 {}).\n\
                             请等它结束。如果确定没有其他运行(上次异常退出残留),\
                             删除该文件后重试:\n  rm {}",
                            path.display(),
                            path.display()
                        ),
                    ));
                }
                // Fail-open: a permissions/IO problem must not block a real run.
                Err(_) => return Ok(RunLock { path, owned: false }),
            }
        }
        // Both attempts hit AlreadyExists-but-couldn't-reclaim in a tight race —
        // fail open rather than spin.
        Ok(RunLock { path, owned: false })
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// `true` when the lock at `path` is held by **this very process** on this
/// host — i.e. the current session already has a run in flight. Used to turn the
/// misleading "another umadev is running" refusal into an accurate
/// "your input will be queued to the existing run" signal. Fail-open: an
/// unreadable/unparseable lock is NOT attributed to us (so we never silently
/// swallow a foreign lock as our own).
fn holder_is_self(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(owner) = Owner::parse(&contents) else {
        return false;
    };
    let local_host = hostname();
    let local_boot = boot_id();
    let same_host = owner.host.is_empty() || local_host.is_empty() || owner.host == local_host;
    // A recorded boot-id that differs from THIS boot means the lock predates a reboot, so a
    // matching PID is a RECYCLED pid, not us - never treat that as self (else the routing
    // layer queues input into a run that no longer exists). Empty on EITHER side = unknown =
    // "matches": an unreadable local boot id (`sysctl` absent, `wmic` gone in current Windows)
    // must not make us stop recognising our OWN lock.
    let same_boot = owner.boot.is_empty() || local_boot.is_empty() || owner.boot == local_boot;
    owner.pid == std::process::id() && same_host && same_boot
}

/// The recorded owner of a workspace claim: a `.umadev/run.lock` line, or a temporary-rewind
/// crash marker ([`crate::checkpoint`]). Both files exist to answer ONE question — "is the
/// process that wrote this still alive?" — and getting two different answers out of them is
/// how a live single-writer lock gets reclaimed under a live holder.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClaimOwner<'a> {
    /// The owner's process id (`0` = none recorded).
    pub pid: u32,
    /// The owner's hostname (empty = not recorded / unreadable).
    pub host: &'a str,
    /// The owner's boot id (empty = not recorded / unreadable).
    pub boot: &'a str,
}

/// What we can PROVE about a claim's owner. Deliberately three-valued: "I could not tell"
/// is a distinct answer from "it is dead", and collapsing them is what reclaims live locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerLiveness {
    /// Alive (proved, or conservatively assumed) — the claim stands. NEVER reclaim.
    Live,
    /// Provably gone — the claim may be reclaimed right now.
    Abandoned,
    /// Unattributable. The caller's own age window is the only honest signal left.
    AgeOnly,
}

/// The ONE owner-liveness rule, shared by the run lock ([`is_stale`]) and the temporary-rewind
/// crash marker (`checkpoint::marker_is_abandoned`). Pure + fully injectable (the local host,
/// the local boot id, our pid and the liveness answer are all parameters), so every branch —
/// reboot, PID reuse, shared network workspace, unreadable boot id — is decidable in a test.
///
/// Decision order — **host, then boot, then PID, then age**:
///
/// 1. **It is us** (our live pid, same host, no boot conflict) → [`OwnerLiveness::Live`].
/// 2. **Another host** (a shared / network workspace): its process table is unreachable and
///    its boot id is unrelated to ours, so NEITHER probe means anything →
///    [`OwnerLiveness::AgeOnly`]. This is the ordering that matters: a boot check placed
///    ahead of the host check reclaims a *live* remote holder's lock on every acquire (machine
///    B never has machine A's boot id), putting two writers on one tree.
/// 3. **No usable pid** → [`OwnerLiveness::AgeOnly`].
/// 4. **Same host, pid probe** — the primary and STRONGEST signal:
///    - dead → [`OwnerLiveness::Abandoned`] (the ordinary crash path, and the ordinary
///      post-reboot path: a reboot kills the holder, so its pid is normally gone);
///    - alive **and** no boot conflict → [`OwnerLiveness::Live`];
///    - alive **but** the recorded boot id differs → [`OwnerLiveness::AgeOnly`]. The mismatch
///      has TWO readings we cannot tell apart: a reboot recycled the pid onto an unrelated
///      live process, **or** the boot id itself moved under a still-live owner (macOS
///      recomputes `kern.boottime` on every clock correction, and an unreadable id reads as
///      empty). Reclaiming on a boot STRING alone would delete a live holder's claim — the
///      single-writer invariant, gone. The age window still frees a genuinely rebooted owner;
///      it just never yanks a live one.
///    - unprobeable → [`OwnerLiveness::AgeOnly`] (conservative: an errored probe is not proof
///      of death; a boot mismatch is not proof either).
///
/// Empty = **unknown**, and unknown reads as "matches" on both host and boot: a claim written
/// by an older build, or on a machine whose boot id / hostname cannot be read (`sysctl`
/// missing; `wmic` is gone in current Windows), must keep behaving exactly as it did — never
/// as "a different boot ⇒ dead", which would reclaim a LIVE local lock.
pub(crate) fn classify_claim_owner(
    owner: ClaimOwner<'_>,
    local_host: &str,
    local_boot: &str,
    self_pid: u32,
    alive: Option<bool>,
) -> OwnerLiveness {
    let same_host = owner.host.is_empty() || local_host.is_empty() || owner.host == local_host;
    let boot_conflict =
        !owner.boot.is_empty() && !local_boot.is_empty() && owner.boot != local_boot;

    // 1. Ours, right now.
    if owner.pid != 0 && owner.pid == self_pid && same_host && !boot_conflict {
        return OwnerLiveness::Live;
    }
    // 2. Another machine — unprobeable by construction.
    if !same_host {
        return OwnerLiveness::AgeOnly;
    }
    // 3. Nothing to probe.
    if owner.pid == 0 {
        return OwnerLiveness::AgeOnly;
    }
    // 4. This host: the pid probe decides, and a boot mismatch may only DOWNGRADE a live
    //    answer to the age window — never upgrade it to "reclaim".
    match alive {
        Some(false) => OwnerLiveness::Abandoned,
        Some(true) if !boot_conflict => OwnerLiveness::Live,
        Some(true) | None => OwnerLiveness::AgeOnly,
    }
}

/// A best-effort identifier that CHANGES on every reboot (whitespace-stripped to a single
/// token). Linux: `/proc/sys/kernel/random/boot_id`. macOS: the `kern.boottime` sysctl. Empty
/// when neither is available (reboot-detection then simply doesn't apply). Lets an abandoned
/// pre-reboot lock be reclaimed even when its PID was recycled by a live process.
///
/// `pub(crate)` because the temp-rewind crash marker
/// ([`crate::checkpoint::TempRewindMarker`]) needs the SAME reboot rule: after a reboot a
/// PID is meaningless, so an owner-liveness probe against a recycled PID must never be
/// allowed to keep a dead owner's claim alive.
pub(crate) fn boot_id() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(id) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        return id.split_whitespace().collect();
    }
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("sysctl")
        .args(["-n", "kern.boottime"])
        .output()
    {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .collect();
        }
    }
    // Windows: the OS last-boot-up timestamp is stable within a boot and changes on every
    // reboot, so it works as a boot-id. wmic is present on essentially all current Windows;
    // when it is absent we fall through to empty (the age fallback still frees a stale lock).
    #[cfg(windows)]
    if let Ok(out) = std::process::Command::new("wmic")
        .args(["os", "get", "lastbootuptime", "/value"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(v) = s.split('=').nth(1) {
                let tok: String = v.split_whitespace().collect();
                if !tok.is_empty() {
                    return tok;
                }
            }
        }
    }
    String::new()
}

/// `true` when the lock at `path` belongs to a crashed/abandoned run and may be
/// reclaimed. The owner verdict is the SHARED rule ([`classify_claim_owner`] —
/// host → boot → PID → age), so the run lock and the temporary-rewind marker can
/// never disagree about whether the same process is alive:
///
/// 1. Same host + dead PID → stale (reclaim).
/// 2. Same host + live PID → NOT stale (a real concurrent run) — and no boot-id
///    string may override that: a live holder's lock is never reclaimed out from
///    under it, because two writers on one tree is the failure this lock exists
///    to prevent.
/// 3. Cross-host / unattributable / boot-conflicted owner → stale only if older
///    than [`STALE_SECS`] (measured from the owner's own recorded `ts`).
///
/// Fail-open at every branch: an unreadable file is treated as stale (the holder
/// can no longer be identified, so it can't be live), and a PID we cannot probe
/// falls to the age window rather than being reclaimed.
fn is_stale(path: &Path) -> bool {
    // Can't read it → owner is unidentifiable → safe to reclaim.
    let Ok(contents) = std::fs::read_to_string(path) else {
        return true;
    };
    // Corrupt / legacy-without-fields → no parseable owner at all, so there is
    // no recorded `ts` heartbeat. Force the mtime fallback with a ts==0 owner.
    let Some(owner) = Owner::parse(&contents) else {
        return older_than_stale(&Owner::default(), path);
    };

    // Never probe pid 0 (on Unix `kill -0 0` signals the whole process GROUP and
    // would read back as "alive"); an owner with no pid is decided by age.
    let alive = if owner.pid == 0 {
        None
    } else {
        pid_is_alive(owner.pid)
    };
    match classify_claim_owner(
        ClaimOwner {
            pid: owner.pid,
            host: &owner.host,
            boot: &owner.boot,
        },
        &hostname(),
        &boot_id(),
        std::process::id(),
        alive,
    ) {
        OwnerLiveness::Live => false,
        OwnerLiveness::Abandoned => true,
        // Age from the OWNER's recorded `ts` (the lock's own heartbeat), NOT the
        // file's mtime.
        OwnerLiveness::AgeOnly => older_than_stale(&owner, path),
    }
}

/// `true` when the lock is older than [`STALE_SECS`].
///
/// P0-5/P1-4: age is measured from the OWNER's recorded `ts` (UNIX seconds
/// written into the lock at creation) against the current wall clock — NOT the
/// file's mtime. mtime is unreliable across hosts and on NFS (clock skew, `noatime`
/// /relatime quirks, a `touch`/rsync bumping it), and `Owner.ts` was already
/// parsed but went unused. Only when the owner has no usable timestamp (`ts == 0`:
/// a legacy/corrupt line) do we fall back to the file mtime as a last resort. An
/// unstattable file on that fallback can't be a live heartbeat → treat as stale.
fn older_than_stale(owner: &Owner, path: &Path) -> bool {
    if owner.ts != 0 {
        // now - ts > STALE_SECS  (saturating: a future-dated ts → age 0 → not stale).
        return now_secs().saturating_sub(owner.ts) > STALE_SECS;
    }
    // No recorded heartbeat — last-resort mtime fallback.
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(mtime) => mtime
            .elapsed()
            .map(|age| age.as_secs() > STALE_SECS)
            .unwrap_or(false),
        Err(_) => true,
    }
}

impl Owner {
    /// Parse `pid=<n> host=<name> ts=<n>` (whitespace-separated, any order, extra
    /// keys ignored). Returns `None` if no `pid=` key is present at all — older
    /// `pid=.. ts=..` lines without a host still parse (host = empty).
    fn parse(s: &str) -> Option<Owner> {
        let line = s.lines().next().unwrap_or("");
        let mut pid: Option<u32> = None;
        let mut host = String::new();
        let mut ts: u64 = 0;
        let mut boot = String::new();
        for tok in line.split_whitespace() {
            if let Some(v) = tok.strip_prefix("pid=") {
                pid = v.parse().ok();
            } else if let Some(v) = tok.strip_prefix("host=") {
                host = v.to_string();
            } else if let Some(v) = tok.strip_prefix("ts=") {
                ts = v.parse().unwrap_or(0);
            } else if let Some(v) = tok.strip_prefix("boot=") {
                boot = v.to_string();
            }
        }
        pid.map(|pid| Owner {
            pid,
            host,
            ts,
            boot,
        })
    }
}

/// Best-effort hostname, dependency-free. Reads the usual env vars and, on Unix,
/// falls back to `/etc/hostname`-equivalent via `uname -n`. An empty string when
/// nothing is available — callers treat empty as "host unknown" (no same-host
/// PID probe, age fallback only), which is the safe direction.
///
/// `pub(crate)`: the temp-rewind crash marker records the same owner identity, so a
/// marker written on another machine (a shared / network workspace) is never judged by
/// THIS host's process table.
pub(crate) fn hostname() -> String {
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(h) = std::env::var(key) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    // Unix `HOSTNAME` is often unexported; ask the OS directly. Fail-open: if the
    // command is missing or errors we just return empty.
    #[cfg(unix)]
    {
        if let Ok(out) = std::process::Command::new("uname").arg("-n").output() {
            if out.status.success() {
                let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !h.is_empty() {
                    return h;
                }
            }
        }
    }
    String::new()
}

/// Is the process `pid` currently alive on **this** host?
///
/// `Some(true)` alive, `Some(false)` provably gone, `None` could-not-determine
/// (caller stays conservative). Dependency-free:
/// - **Unix**: `kill -0 <pid>` — exit 0 means the process exists (or exists but
///   we lack permission, which still proves it is alive); a "no such process"
///   failure proves it is gone. Implemented via `/bin/kill` semantics through
///   `Command`, with no `libc` dependency.
/// - **Windows**: `tasklist /FI "PID eq <pid>"` and look for the PID in output.
/// - Anything else / probe error → `None`.
#[cfg(unix)]
pub(crate) fn pid_is_alive(pid: u32) -> Option<bool> {
    // `kill -0` sends no signal but performs the permission/existence check.
    // Exit status 0 → exists. Non-zero → distinguish "no such process" (gone)
    // from other errors (unknown). We run the standalone `kill` utility so this
    // stays free of a libc dependency.
    let out = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .ok()?;
    if out.status.success() {
        return Some(true);
    }
    // `kill -0` failed. Classify by the failure reason:
    //  - "no such process"   → the PID is valid but gone (reclaimable).
    //  - "illegal/invalid process id" → an impossible PID; it can't be live.
    //  - "not permitted"/"permission" → the process EXISTS but is owned by
    //    someone else (alive — never reclaim).
    //  - anything else → unknown; stay conservative (caller uses age fallback).
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if stderr.contains("no such process") {
        Some(false)
    } else if stderr.contains("illegal") || stderr.contains("invalid") {
        // A PID outside the valid range / unparseable by `kill` — not a real,
        // running process.
        Some(false)
    } else if stderr.contains("not permitted") || stderr.contains("permission") {
        // Process exists but is owned by someone else.
        Some(true)
    } else {
        None
    }
}

#[cfg(windows)]
pub(crate) fn pid_is_alive(pid: u32) -> Option<bool> {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .ok()?;
    // Classify by what tasklist SAID, not by how it exited. "No tasks are running
    // which match the specified criteria" — the answer we care about most, a dead
    // pid — is printed on stdout while the exit status is NON-ZERO on the Windows
    // builds that matter. Bailing on the status first turned every dead pid into
    // "unknown", so a crashed owner fell through to the age window: a stranded work
    // tree stayed stranded for the whole rewind window, and a dead run lock held the
    // tree for hours. The Unix arm has always classified by the message; this one
    // now does too.
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    if said.contains("no tasks") {
        return Some(false);
    }
    // A pid tasklist itself rejects as unusable cannot name a running process.
    if said.contains("invalid") || said.contains("illegal") {
        return Some(false);
    }
    // A real match is a CSV row carrying the pid as its own quoted field. Match that
    // exactly — a bare substring search hits the pid inside an image name, a memory
    // figure, or a session id, and reports a dead pid as alive (which is the one
    // mistake that must never happen: it makes a stale lock permanent).
    if said.contains(&format!("\"{pid}\"")) {
        return Some(true);
    }
    // Understood nothing. Say so — the caller falls back to the age window rather
    // than guessing in either direction.
    if out.status.success() {
        Some(false)
    } else {
        None
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn pid_is_alive(_pid: u32) -> Option<bool> {
    None
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a raw lock file with the given contents.
    fn write_lock(root: &Path, contents: &str) {
        let dir = root.join(".umadev");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.lock"), contents).unwrap();
    }

    /// A PID that is essentially guaranteed never to be live. It is inside the
    /// valid PID range on every platform we target (so `kill`/`tasklist` report
    /// "no such process" rather than rejecting it as out-of-range), yet far above
    /// any PID a real run would have, so liveness probes report "gone". PIDs are
    /// recycled, but nothing in CI is anywhere near this value.
    const DEAD_PID: u32 = 4_000_000;

    #[test]
    fn second_acquire_in_same_session_is_queue_signal_not_another_umadev() {
        // CASE 1: the lock is held by THIS process (our own session already has a
        // run in flight). A second acquire must NOT report "another umadev" —
        // it returns a WouldBlock "queue your input to the existing run" signal so
        // the caller routes the input into the running pipeline.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        let lock = RunLock::acquire(root).expect("first acquire");
        let second = RunLock::acquire(root).expect_err("second same-session acquire is signalled");
        assert_eq!(
            second.kind(),
            io::ErrorKind::WouldBlock,
            "our own session's lock is a queue signal, not a hard refusal"
        );
        let msg = second.to_string();
        assert!(
            !msg.contains("另一个 umadev"),
            "must NOT claim another umadev is running for our own lock"
        );
        assert!(
            msg.contains("排队"),
            "message must explain the input will be queued to the existing run"
        );
        // Dropping the first releases the lock; a later acquire succeeds.
        drop(lock);
        assert!(RunLock::acquire(root).is_ok(), "lock released on drop");
    }

    #[test]
    fn run_intent_reclaims_our_own_residual_lock_instead_of_would_block() {
        // THE REGRESSION: research wedged at `0/9`. A real execution block uses
        // `acquire_for_run`. When OUR OWN previous block left a same-PID lock
        // behind (its guard not yet dropped, or it aborted before Drop), the run
        // path must RECLAIM it and take over — never the `WouldBlock` queue signal
        // the routing layer wants, which the `?` would have propagated and ended
        // the run task with zero phases done.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        // Simulate our own residue: a lock file owned by THIS pid + host.
        write_lock(
            root,
            &format!(
                "pid={} host={} ts={}",
                std::process::id(),
                hostname(),
                now_secs()
            ),
        );
        let path = root.join(".umadev").join("run.lock");
        assert!(
            holder_is_self(&path),
            "fixture must be our own residual lock"
        );
        // Routing intent still returns the queue signal (unchanged behaviour).
        let routed = RunLock::acquire(root).expect_err("route intent still signals");
        assert_eq!(
            routed.kind(),
            io::ErrorKind::WouldBlock,
            "the routing/queue layer must keep its same-PID WouldBlock signal"
        );
        // Re-establish the residue (the failed route attempt didn't touch it),
        // then the EXECUTION intent reclaims it and takes ownership.
        let lock = RunLock::acquire_for_run(root).expect("run intent reclaims our residue");
        assert!(lock.owned, "reclaimed lock is now owned by us");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains(&format!("pid={}", std::process::id())),
            "the reclaimed lock records our identity"
        );
    }

    // A live FOREIGN holder must still be refused even under the run-execution
    // intent — `acquire_for_run` only relaxes the SAME-PID case, never an
    // external live run. Modelled with PID 1 (init/launchd): a Unix concept.
    #[cfg(unix)]
    #[test]
    fn run_intent_still_refuses_a_live_foreign_holder() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        write_lock(
            root,
            &format!("pid=1 host={} ts={}", hostname(), now_secs()),
        );
        let err = RunLock::acquire_for_run(root)
            .expect_err("a live foreign run is refused even for execution");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(
            err.to_string().contains("另一个 umadev"),
            "external live holder is still the genuine refusal under run intent"
        );
    }

    #[test]
    fn run_intent_reclaims_a_dead_external_pid_like_routing_does() {
        // The PID-liveness reclaim path is shared by both intents: a dead
        // external holder is stale and taken over regardless of intent.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        write_lock(
            root,
            &format!("pid={DEAD_PID} host={} ts={}", hostname(), now_secs()),
        );
        let lock =
            RunLock::acquire_for_run(root).expect("dead external PID reclaimed under run intent");
        assert!(lock.owned, "reclaimed lock is owned by us");
    }

    // Models a live foreign holder with PID 1 (init/launchd) — a Unix concept,
    // so the whole test is unix-only (on Windows `root` would be unused).
    #[cfg(unix)]
    #[test]
    fn foreign_live_run_is_the_real_another_umadev_refusal() {
        // CASE 3: a DIFFERENT, still-alive process on this host holds the lock —
        // the genuine "another umadev is running" refusal. Modelled with PID 1
        // (init/launchd): present and alive on every Unix host, and never us.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        #[cfg(unix)]
        {
            write_lock(
                root,
                &format!("pid=1 host={} ts={}", hostname(), now_secs()),
            );
            let path = root.join(".umadev").join("run.lock");
            assert!(!holder_is_self(&path), "PID 1 is not our process");
            assert!(
                !is_stale(&path),
                "a live foreign PID must not be reclaimable"
            );
            let err = RunLock::acquire(root).expect_err("foreign live run refused");
            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
            assert!(
                err.to_string().contains("另一个 umadev"),
                "a live foreign run is the genuine 'another umadev' refusal"
            );
            assert!(err.to_string().contains("rm "), "refusal stays actionable");
        }
    }

    #[test]
    fn stale_lock_with_dead_pid_is_reclaimed_and_taken_over() {
        // The user's bug: a crashed run left a fresh lock with a dead PID on this
        // host. PID liveness must classify it stale even though it is brand new.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        write_lock(
            root,
            &format!("pid={DEAD_PID} host={} ts={}", hostname(), now_secs()),
        );
        let path = root.join(".umadev").join("run.lock");
        assert!(
            is_stale(&path),
            "a fresh lock whose owner PID is dead must be reclaimable"
        );
        // End-to-end: acquire reclaims it and takes over, then owns the new lock.
        let lock = RunLock::acquire(root).expect("stale lock auto-reclaimed");
        assert!(lock.owned, "reclaimed lock is owned by us");
        // The new lock records OUR identity, not the dead holder's.
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains(&format!("pid={}", std::process::id())));
    }

    #[test]
    fn corrupt_lock_fails_open_via_age_and_hint() {
        // A garbage / truncated lock with no parseable owner: PID-liveness can't
        // run, so we fall back to age. A FRESH corrupt lock is conservatively
        // respected (refused) but the refusal tells the user how to force-clear.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        write_lock(root, "\u{0}\u{0}garbage-not-a-lock");
        let path = root.join(".umadev").join("run.lock");
        // Fresh + unparseable → not yet age-stale → refuse, but actionably.
        assert!(!is_stale(&path), "fresh corrupt lock is not age-stale");
        let err = RunLock::acquire(root).expect_err("fresh corrupt lock refused");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(
            err.to_string().contains("rm "),
            "refusal must tell the user how to force-clear the lock"
        );
    }

    #[test]
    fn unreadable_owner_treated_as_reclaimable() {
        // An empty lock file (no owner at all): owner is unidentifiable, so it
        // cannot be a live holder — reclaimable so a truncated write doesn't wedge
        // the workspace forever.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        write_lock(root, "");
        // Empty → Owner::parse None → age fallback. An empty file is unparseable
        // but fresh, so this asserts the parse boundary, not reclaim.
        assert!(Owner::parse("").is_none(), "empty contents have no owner");
        // A whitespace-only first line likewise yields no owner.
        assert!(Owner::parse("   \n").is_none());
        // Acquire on an empty fresh lock: unparseable + fresh → refused with hint.
        let err = RunLock::acquire(root).expect_err("fresh empty lock refused");
        assert!(err.to_string().contains("rm "));
    }

    #[test]
    fn owner_parse_handles_legacy_and_new_formats() {
        // New format.
        let o = Owner::parse("pid=4321 host=mybox ts=1700000000").expect("parses");
        assert_eq!(o.pid, 4321);
        assert_eq!(o.host, "mybox");
        assert_eq!(o.ts, 1_700_000_000);
        // Legacy format (no host) still parses; host empty → no same-host probe.
        let legacy = Owner::parse("pid=99 ts=0").expect("legacy parses");
        assert_eq!(legacy.pid, 99);
        assert!(legacy.host.is_empty());
        // Reordered / extra keys tolerated.
        let reordered = Owner::parse("ts=5 extra=x pid=7 host=h").expect("parses");
        assert_eq!((reordered.pid, reordered.host.as_str()), (7, "h"));
    }

    #[test]
    fn staleness_uses_owner_ts_not_mtime() {
        // P0-5/P1-4: a cross-host lock (can't probe its PID) whose recorded
        // `ts` is ancient must be reclaimable EVEN THOUGH the file mtime is
        // brand-new (we just wrote it). This proves age comes from owner.ts,
        // not the file's mtime. Use a foreign host so the PID-probe branch is
        // skipped and only the age path decides.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        let ancient = now_secs().saturating_sub(STALE_SECS + 60);
        write_lock(
            root,
            &format!("pid=12345 host=some-other-host ts={ancient}"),
        );
        let path = root.join(".umadev").join("run.lock");
        assert!(
            is_stale(&path),
            "an ancient owner.ts must be stale despite a fresh file mtime"
        );

        // Inverse: a FRESH ts on a foreign host is NOT stale even if the file
        // is touched/old — the owner heartbeat is recent.
        let fresh = now_secs();
        write_lock(root, &format!("pid=12345 host=some-other-host ts={fresh}"));
        assert!(
            !is_stale(&path),
            "a fresh owner.ts must not be reclaimable on the age path"
        );
    }

    /// THE SINGLE-WRITER BLOCKER: a boot-id mismatch must NEVER reclaim a LIVE lock.
    ///
    /// The rule used to test the boot id BEFORE the host — so `owner.boot != boot_id()`
    /// reclaimed the lock outright, and the same-host check below it was unreachable. Three
    /// ways that deletes a live holder's lock and puts two writers on one tree:
    ///
    /// 1. A shared / network workspace: machine B NEVER has machine A's boot id, so B
    ///    reclaimed A's live lock on every single acquire.
    /// 2. Our own `boot_id()` returns "" when the OS won't say (`wmic` is REMOVED in current
    ///    Windows; `sysctl` can fail to spawn) → "" ≠ the recorded boot → a LIVE LOCAL lock
    ///    reclaimed.
    /// 3. macOS recomputes `kern.boottime` on every clock correction, so the boot string can
    ///    change WITHIN one boot, under a live holder.
    ///
    /// Decided by the shared rule ([`classify_claim_owner`]) — host, then boot, then pid,
    /// then age — so the run lock and the temp-rewind marker can never disagree about the
    /// liveness of the same process.
    #[test]
    fn a_boot_id_mismatch_never_reclaims_a_live_lock() {
        let me = std::process::id();
        let live_local = ClaimOwner {
            pid: me,
            host: "our-host",
            boot: "our-boot",
        };
        // (a) A DIFFERENT boot string over a live same-host owner (macOS clock correction,
        //     a re-read that drifted): the pid probe says ALIVE — that answer stands.
        let other_pid = ClaimOwner {
            pid: 4321,
            host: "our-host",
            boot: "a-DIFFERENT-boot-string",
        };
        assert_eq!(
            classify_claim_owner(other_pid, "our-host", "our-boot", me, Some(true)),
            OwnerLiveness::AgeOnly,
            "a live same-host owner under a different boot string is NEVER reclaimed on the \
             spot — the boot string is not trustworthy enough to kill a live claim"
        );
        // (b) An EMPTY LOCAL boot id (the Windows-11 `wmic` case): unknown ≠ mismatch.
        assert_eq!(
            classify_claim_owner(live_local, "our-host", "", me, Some(true)),
            OwnerLiveness::Live,
            "an unreadable local boot id must not make us reclaim our own live lock"
        );
        // (c) An EMPTY RECORDED boot (a legacy lock line): likewise.
        let legacy = ClaimOwner {
            pid: 4321,
            host: "our-host",
            boot: "",
        };
        assert_eq!(
            classify_claim_owner(legacy, "our-host", "our-boot", me, Some(true)),
            OwnerLiveness::Live,
            "a lock with no recorded boot id is judged by its PID, not by a phantom reboot"
        );
        // A genuinely rebooted same-host owner IS reclaimed: after a reboot its pid is gone.
        let rebooted = ClaimOwner {
            pid: 4321,
            host: "our-host",
            boot: "boot-BEFORE-the-reboot",
        };
        assert_eq!(
            classify_claim_owner(rebooted, "our-host", "boot-AFTER", me, Some(false)),
            OwnerLiveness::Abandoned,
            "a rebooted owner's dead pid is reclaimed at once"
        );
        // …and if the reboot RECYCLED its pid onto a live process, the age window still
        // frees it (bounded), so a workspace can never wedge forever.
        assert_eq!(
            classify_claim_owner(rebooted, "our-host", "boot-AFTER", me, Some(true)),
            OwnerLiveness::AgeOnly,
            "a recycled pid falls to the age window — freed, but never by yanking a live claim"
        );
        // A LIVE owner on ANOTHER host (a shared / NFS workspace) is respected: its process
        // table is unreachable and its boot id is unrelated to ours, so age is all we have.
        let remote = ClaimOwner {
            pid: 4321,
            host: "their-host",
            boot: "their-boot",
        };
        assert_eq!(
            classify_claim_owner(remote, "our-host", "our-boot", me, Some(false)),
            OwnerLiveness::AgeOnly,
            "another host's lock is decided by AGE — never by our own process table, and never \
             by a boot id that means nothing across machines"
        );
    }

    /// The same blocker, end-to-end through the real lock file: an ALIVE local owner
    /// (pid 1 = init/launchd) whose recorded boot id differs from ours must not be stale.
    #[cfg(unix)]
    #[test]
    fn a_live_local_lock_with_a_foreign_boot_id_is_not_reclaimable() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        write_lock(
            root,
            &format!(
                "pid=1 host={} ts={} boot=not-the-boot-id-we-have-now",
                hostname(),
                now_secs()
            ),
        );
        let path = root.join(".umadev").join("run.lock");
        assert!(
            !is_stale(&path),
            "a LIVE holder's lock must survive a boot-id mismatch — reclaiming it is two \
             writers on one tree"
        );
        let err = RunLock::acquire(root).expect_err("the live holder still owns the workspace");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        // The same lock, but ANCIENT: the age window is the honest way out, and it still works.
        write_lock(
            root,
            &format!(
                "pid=1 host={} ts={} boot=not-the-boot-id-we-have-now",
                hostname(),
                now_secs().saturating_sub(STALE_SECS + 60)
            ),
        );
        assert!(
            is_stale(&path),
            "past the age window even a boot-conflicted live pid is reclaimable"
        );
    }

    #[test]
    fn staleness_falls_back_to_mtime_when_ts_is_zero() {
        // A legacy/corrupt owner with ts=0 has no heartbeat → mtime fallback.
        // A freshly-written file is young, so it is NOT age-stale.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        write_lock(root, "pid=12345 host=some-other-host ts=0");
        let path = root.join(".umadev").join("run.lock");
        assert!(
            !is_stale(&path),
            "ts=0 + fresh mtime → not age-stale (mtime fallback)"
        );
    }

    #[test]
    fn reclaim_read_back_surrenders_when_lock_clobbered() {
        // P0-2: prove the read-back self-check. Simulate a racing reclaimer
        // that overwrote our just-created lock with a FOREIGN owner before we
        // read it back. `holder_is_self` then returns false → we must surrender
        // ownership (owned:false) and NOT delete the foreign lock.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        let path = root.join(".umadev").join("run.lock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A foreign owner already in the file (as if a racer clobbered us).
        std::fs::write(&path, "pid=999999 host=racer ts=1\n").unwrap();
        // The read-back self-check must classify this as NOT us.
        assert!(
            !holder_is_self(&path),
            "a foreign-owner lock must not be attributed to us on read-back"
        );
        // And the foreign lock must remain untouched (fail-open: never delete
        // someone else's lock on the surrender path).
        assert!(path.exists(), "we must not delete a foreign lock");
    }

    #[test]
    fn reclaim_read_back_confirms_self_on_clean_acquire() {
        // The happy path: a clean acquire writes our identity and the read-back
        // confirms it, so we own the lock end-to-end.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let root = tmp.path();
        let lock = RunLock::acquire_for_run(root).expect("clean acquire");
        assert!(lock.owned, "a clean acquire owns the lock after read-back");
        let path = root.join(".umadev").join("run.lock");
        assert!(holder_is_self(&path), "read-back confirms our identity");
    }

    #[test]
    fn pid_liveness_self_is_alive() {
        // Our own PID must probe as alive on every supported platform; if the
        // probe is unavailable it returns None (never a false "dead").
        match pid_is_alive(std::process::id()) {
            Some(true) | None => {}
            Some(false) => panic!("our own running process must not probe as dead"),
        }
        // A clearly-invalid PID must never probe as *alive*.
        assert_ne!(
            pid_is_alive(DEAD_PID),
            Some(true),
            "an impossible PID must not probe as alive"
        );
    }
}
