//! `umadev ci` — run governance on eligible project files in the workspace.
//!
//! This is the CI/CD entry point: scan governance-eligible files under the
//! project root and exit non-zero if any file's blocking rule fires.
//! In a Git worktree the full scan includes tracked and untracked non-ignored
//! files; untracked ignored files and generated/vendor paths are excluded.
//!
//! ## Usage
//! ```bash
//! umadev ci                      # scan + fail on any violation
//! umadev ci --report-only        # scan but always exit 0 (for reporting)
//! umadev ci --changed-only       # scan only git-changed files
//! ```
//!
//! ## Output
//! Enforcing mode emits the first hit per file. Report-only mode emits every
//! enabled content-rule hit so its aggregate is suitable for governance audits.

use std::path::{Path, PathBuf};
use umadev_governance::{
    check_sensitive_path, pre_write_floor_decision, scan_content_findings_with_context,
    scan_content_with_context, Decision, Policy, ProjectContext,
};

/// File extensions the CI scan considers "source" (governance-eligible).
const SCAN_EXTENSIONS: &[&str] = &[
    "js", "jsx", "mjs", "cjs", "ts", "tsx", "py", "rb", "go", "rs", "java", "kt", "swift", "php",
    "vue", "svelte", "astro", "html", "htm", "css", "scss", "sass", "yml", "yaml", "sh", "bash",
    "zsh",
];

/// Is `rel` a security-sensitive path that MUST be scanned by the bypass-immune
/// floor REGARDLESS of its extension?
///
/// The `SCAN_EXTENSIONS` allow-list silently drops exactly the files most likely
/// to leak a live secret — `.env` (no extension), `id_rsa`, `*.pem`, `credentials`,
/// anything under `.ssh/` — so a staged `.env` with a `sk_live_…` key used to scan
/// as "0 files, 0 blocked, exit 0". This predicate pulls those paths back into
/// scope so [`pre_write_floor_decision`] can block them. It is a SUPERSET of the
/// floor's own path guard ([`check_sensitive_path`]) plus the two dotenv/cert
/// forms the guard's fixed suffix list omits (`.env.<anything>`, `*.pem`), so a
/// secret in any of them reaches the content floor. Segment-aware via the reused
/// guard, so `messages.ts` never matches.
fn is_sensitive_scan_path(rel: &str) -> bool {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    // *.pem (private keys / certs) anywhere in the tree.
    if std::path::Path::new(&lower)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pem"))
    {
        return true;
    }
    // Any dotenv variant: `.env`, `.env.local`, `.env.staging`, `.env.<x>`.
    let last = lower.rsplit('/').next().unwrap_or("");
    if last == ".env" || last.starts_with(".env.") {
        return true;
    }
    // Reuse the floor's EXACT path guard for the rest (`.ssh/` `.aws/` `.git/`
    // segments, `id_rsa` / `credentials` / `.npmrc` / … suffixes) so CI stays in
    // lockstep with the floor rather than drifting from a hand-copied list.
    check_sensitive_path(rel, "").block
}

/// Directories to skip during the scan (deps, build output, VCS).
/// Dot-directories the FULL scan DESCENDS into anyway (a leading `.` normally skips a dir).
/// These legitimately carry secrets / CI config a commit could leak, so a full `umadev ci`
/// must see them (the changed-only git path already lists their tracked files, so this keeps
/// the two scopes in sync). Kept small so the walk stays fast (no descent into arbitrary
/// dot-dirs).
const SCAN_DOT_DIRS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".docker",
    ".config",
    ".github",
    ".circleci",
    ".env.d",
];

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".output",
    ".svelte-kit",
    "vendor",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    "coverage",
    ".turbo",
];

/// CI scan options.
#[derive(Debug, Clone)]
pub struct CiOptions {
    /// Report first hits without failing (exit 0).
    pub report_only: bool,
    /// Only scan governance-eligible staged Git blobs (vs the full worktree scope).
    pub changed_only: bool,
    /// Project root to scan.
    pub project_root: PathBuf,
}

/// Result of a CI scan.
#[derive(Debug, Default)]
pub struct CiResult {
    /// Files selected after scope, extension, ignore, and directory filters.
    pub files_selected: usize,
    /// Selected files whose UTF-8 content was actually scanned.
    pub files_scanned: usize,
    /// Number of scanned files with at least one blocking governance decision.
    pub files_blocked: usize,
    /// Governance findings emitted. Complete in report-only mode; first-hit per
    /// file in enforcing mode.
    pub governance_findings: usize,
    /// High/critical dependency findings returned by `npm audit`.
    pub npm_audit_findings: usize,
    /// How the candidate file set was obtained.
    pub scan_scope: CiScanScope,
    /// Whether enforcing mode should fail CI.
    pub failed: bool,
}

/// Reproducible source used to select files for a CI scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CiScanScope {
    /// Git's staged index (`--changed-only`).
    StagedIndex,
    /// Git tracked plus untracked non-ignored files.
    GitWorktree,
    /// Filesystem walk used when Git metadata is unavailable.
    #[default]
    FilesystemFallback,
}

impl CiScanScope {
    fn description(self) -> &'static str {
        match self {
            Self::StagedIndex => "staged Git blobs only",
            Self::GitWorktree => "Git tracked + untracked non-ignored files",
            Self::FilesystemFallback => {
                "filesystem fallback (Git metadata and ignore rules unavailable)"
            }
        }
    }
}

struct FileSelection {
    files: Vec<PathBuf>,
    scope: CiScanScope,
}

struct ScanTask {
    path: PathBuf,
    rel: String,
    ctx: ProjectContext,
}

struct FileScan {
    rel: String,
    scanned: bool,
    findings: Vec<Decision>,
}

fn scan_task(
    task: &ScanTask,
    root: &Path,
    changed_only: bool,
    report_only: bool,
    policy: &Policy,
) -> FileScan {
    let content = if changed_only {
        read_staged_blob(root, &task.rel)
    } else {
        std::fs::read_to_string(&task.path).ok()
    };
    let Some(content) = content else {
        return FileScan {
            rel: task.rel.clone(),
            scanned: false,
            findings: Vec::new(),
        };
    };

    let floor = pre_write_floor_decision(&task.rel, &content);
    let findings = if report_only {
        let mut findings = Vec::new();
        if floor.block {
            findings.push(floor);
        }
        for decision in scan_content_findings_with_context(&task.rel, &content, policy, task.ctx) {
            if !findings
                .iter()
                .any(|existing: &Decision| existing.clause == decision.clause)
            {
                findings.push(decision);
            }
        }
        findings
    } else {
        let decision = if floor.block {
            floor
        } else {
            scan_content_with_context(&task.rel, &content, policy, task.ctx)
        };
        if decision.block {
            vec![decision]
        } else {
            Vec::new()
        }
    };
    FileScan {
        rel: task.rel.clone(),
        scanned: true,
        findings,
    }
}

fn scan_tasks(
    tasks: &[ScanTask],
    root: &Path,
    changed_only: bool,
    report_only: bool,
    policy: &Policy,
) -> Vec<FileScan> {
    if tasks.is_empty() {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(8)
        .min(tasks.len());
    let chunk_size = tasks.len().div_ceil(workers);

    std::thread::scope(|scope| {
        let handles: Vec<_> = tasks
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|task| {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                scan_task(task, root, changed_only, report_only, policy)
                            }))
                            .unwrap_or_else(|_| FileScan {
                                rel: task.rel.clone(),
                                scanned: false,
                                findings: Vec::new(),
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut scans = Vec::with_capacity(tasks.len());
        for handle in handles {
            if let Ok(mut chunk) = handle.join() {
                scans.append(&mut chunk);
            }
        }
        scans
    })
}

/// Run the CI governance scan. Prints findings to stdout and returns the
/// summary. Exit code is 1 when `failed` is true (the caller maps this).
///
/// # Errors
/// Returns an error only on a filesystem traversal failure.
pub fn run(opts: &CiOptions) -> std::io::Result<CiResult> {
    let policy = Policy::load(&opts.project_root);
    // The run's own governance context — READ IT, don't assume. `umadev ci` is the surface
    // that actually BLOCKS (the PreToolUse hook downgrades every non-floor finding to a
    // pass, and `install --base pre-commit` writes `umadev ci --changed-only` into
    // `.git/hooks/pre-commit`), so a decision the run already honoured and a decision this
    // gate makes MUST be the same decision. Judging with a hardcoded `unknown()` context is
    // what let a user say "our brand is violet", watch the run accept it, and then be unable
    // to COMMIT it: the pre-commit hook blocked UD-CODE-002 on the very color they asked
    // for, with no way to converge — the finding just moved one surface over.
    //
    // Resolved PER FILE, not once for the scan root: git runs its hooks with the cwd set to
    // the repository TOP LEVEL, so in a monorepo (`/repo/apps/web/.umadev/`) the scan root is
    // `/repo` — which carries no `.umadev/` at all — while the files being judged live inside
    // a real UmaDev workspace one level down. A root-only lookup finds nothing there, falls
    // back to `unknown()`, and reproduces the exact unconvergeable block this reader exists to
    // prevent. So each file is judged by the context of the nearest workspace that CONTAINS
    // it (memoized per directory; the single-workspace case resolves once and costs nothing).
    let mut contexts = ContextCache::new(&opts.project_root);
    let selection = collect_source_files(&opts.project_root, opts.changed_only)?;
    let mut result = CiResult {
        files_selected: selection.files.len(),
        scan_scope: selection.scope,
        ..Default::default()
    };

    let tasks: Vec<_> = selection
        .files
        .iter()
        .map(|file| ScanTask {
            path: file.clone(),
            rel: normalized_relative_path(&opts.project_root, file),
            ctx: contexts.for_file(file),
        })
        .collect();
    for scan in scan_tasks(
        &tasks,
        &opts.project_root,
        opts.changed_only,
        opts.report_only,
        &policy,
    ) {
        if !scan.scanned {
            continue;
        }
        result.files_scanned += 1;
        if !scan.findings.is_empty() {
            result.files_blocked += 1;
            result.governance_findings += scan.findings.len();
            for decision in scan.findings {
                let label = finding_label(opts.report_only);
                println!(
                    "{label:<5}  {}  {}  {}",
                    decision.clause,
                    scan.rel,
                    finding_summary(&decision.reason),
                );
            }
        }
    }

    // UD-SEC-016: run `npm audit` if a package-lock.json is present, to catch
    // known-vulnerable dependencies (OWASP A06). Best-effort: if npm isn't
    // installed or the audit fails, skip silently (the file scan still ran).
    //
    // NOT in `--changed-only` mode (the pre-commit gate): a dependency audit judges the
    // WHOLE lockfile, so a PRE-EXISTING transitive CVE - unrelated to the staged change -
    // would fail-CLOSE every commit until it is patched upstream (possibly never),
    // contradicting the changed-only contract ("judge only the staged change"). The full
    // `umadev ci` still runs it.
    if !opts.changed_only && opts.project_root.join("package-lock.json").exists() {
        if let Ok(audit_result) = npm_audit(&opts.project_root) {
            if audit_result.critical + audit_result.high > 0 {
                result.npm_audit_findings = audit_result.critical + audit_result.high;
                let label = finding_label(opts.report_only);
                println!(
                    "{label:<5}  UD-SEC-016  package.json  {} critical, {} high vulnerabilities in dependencies",
                    audit_result.critical, audit_result.high,
                );
            } else if audit_result.total() > 0 {
                println!(
                    "WARN   UD-SEC-016  {} lower-severity vulnerabilities (moderate/low) in dependencies",
                    audit_result.moderate + audit_result.low,
                );
            }
        }
    }

    result.failed =
        (result.governance_findings > 0 || result.npm_audit_findings > 0) && !opts.report_only;
    println!("{}", scan_summary(&result, opts.report_only));
    Ok(result)
}

/// The run's persisted governance [`ProjectContext`] for `root` —
/// `.umadev/governance-context.json`, written by the agent runner and read by the
/// PreToolUse hook.
///
/// It carries what the RUN already established and the user already decided: whether the
/// project is a proven static frontend (so server-surface rules have nothing to guard), and
/// whether the requirement asked for a purple/violet brand (the ONE stand-down of the
/// banned-hue default-reject). `umadev ci` is the surface that actually fails a commit, so
/// it must read the same context every other surface reads — a gate that judges by a
/// different rule book than the run is unconvergeable by construction.
///
/// **Conservative & fail-open**: no context file, an unreadable one, or malformed JSON →
/// [`ProjectContext::unknown`] (full strictness). Governance is never relaxed because we
/// *couldn't read* the context.
///
/// **And never relaxed by a context we cannot attribute.** The file is a *permission*, and
/// a permission belongs to a requirement. `umadev ci` runs long after the run that wrote it
/// — from `.git/hooks/pre-commit`, on whatever the tree is now — so it checks the context's
/// provenance against the workspace's live requirement before honouring it
/// ([`ProjectContext::if_current`]): a `purple_allowed: true` left behind by last quarter's
/// violet rebrand must not stand the banned-hue band down for today's "no purple anywhere".
/// A context that still matches the current requirement is honoured whatever its age — that
/// is the legitimately-purple project, and blocking it is the very bug this reader exists
/// to avoid.
fn load_project_context(root: &Path) -> ProjectContext {
    let Ok(raw) = std::fs::read_to_string(root.join(".umadev").join("governance-context.json"))
    else {
        return ProjectContext::unknown();
    };
    let ctx = serde_json::from_str::<ProjectContext>(&raw).unwrap_or_else(|_| {
        // Malformed / partial JSON → strict.
        ProjectContext::unknown()
    });
    ctx.if_current(now_secs(), workspace_requirement(root).as_deref())
}

/// Per-file governance-context resolution, memoized by directory.
///
/// The scan root is NOT necessarily the UmaDev workspace. `install --base pre-commit` writes
/// `umadev ci --changed-only` into `.git/hooks/pre-commit`, and git runs hooks with the cwd
/// set to the repository TOP LEVEL — so in a monorepo whose workspace is `/repo/apps/web`,
/// this gate runs at `/repo`, where there is no `.umadev/` at all. Judging `apps/web`'s files
/// with the resulting `unknown()` context is the same unconvergeable block as writing no
/// context: the run accepted the brand the commit gate now refuses.
///
/// So a file is judged by the nearest workspace that CONTAINS it: walk up from the file's own
/// directory, stopping at the scan root, and take the first ancestor with a `.umadev/` dir.
/// Nothing found → the scan root's own context (which is `unknown()` when it has none — the
/// strict default, unchanged).
struct ContextCache<'a> {
    root: &'a Path,
    by_dir: std::collections::HashMap<PathBuf, ProjectContext>,
}

impl<'a> ContextCache<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            by_dir: std::collections::HashMap::new(),
        }
    }

    /// The context governing `file`. Memoized per directory, so the common
    /// single-workspace scan does exactly one lookup.
    fn for_file(&mut self, file: &Path) -> ProjectContext {
        let dir = file.parent().unwrap_or(self.root).to_path_buf();
        if let Some(hit) = self.by_dir.get(&dir) {
            return *hit;
        }
        let ctx = self.resolve(&dir);
        self.by_dir.insert(dir, ctx);
        ctx
    }

    /// First ancestor of `dir` (up to and including the scan root) that carries a `.umadev/`
    /// directory; the scan root's own context when none does.
    fn resolve(&self, dir: &Path) -> ProjectContext {
        let mut at = Some(dir);
        while let Some(cur) = at {
            if cur.join(".umadev").is_dir() {
                return load_project_context(cur);
            }
            if cur == self.root {
                break;
            }
            at = cur.parent();
        }
        load_project_context(self.root)
    }
}

/// The requirement this workspace is currently being built from
/// (`.umadev/workflow-state.json`), or `None` when no run has recorded one (a hand-written
/// repo, a fresh clone). `None` means "nothing to match against" — the context then falls
/// back to its age ([`ProjectContext::MAX_UNMATCHED_AGE_SECS`]) rather than being trusted
/// forever. Fail-open: an unreadable / corrupt state file reads as `None`.
fn workspace_requirement(root: &Path) -> Option<String> {
    umadev_agent::state::read_workflow_state(root)
        .map(|s| s.requirement)
        .filter(|r| !r.trim().is_empty())
}

/// UNIX seconds, or 0 when the clock is unreadable (which ages every unmatched context out
/// — the strict direction).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Render an honest summary of scan scope and counting semantics.
fn scan_summary(result: &CiResult, report_only: bool) -> String {
    let skipped = result.files_selected.saturating_sub(result.files_scanned);
    let audit = if result.npm_audit_findings == 0 {
        String::new()
    } else {
        format!(
            " {} high/critical npm-audit finding(s) reported separately.",
            result.npm_audit_findings
        )
    };
    let mode = if report_only {
        " Report-only mode: findings do not change the exit status."
    } else {
        ""
    };
    let governance = if report_only {
        format!(
            "{} file(s) with a governance hit, {} governance finding(s). The count is complete across all enabled content rules.",
            result.files_blocked, result.governance_findings
        )
    } else {
        format!(
            "{} file(s) with a governance hit, {} first-hit finding(s). The enforcing gate stops after the first hit per file; run --report-only for the complete count.",
            result.files_blocked, result.governance_findings
        )
    };
    format!(
        "\nUmaDev scope: {}.\n\
         UmaDev excluded: untracked ignored files (full Git-worktree scope), unsupported types, generated/vendor, and non-allowlisted dot-directories.\n\
         UmaDev policy: path exclusions apply after the irreversible security floor.\n\
         UmaDev: {} file(s) selected, {} scanned, {} unreadable/binary skipped; \
         {governance}{audit}{mode}",
        result.scan_scope.description(),
        result.files_selected,
        result.files_scanned,
        skipped,
    )
}

fn finding_label(report_only: bool) -> &'static str {
    if report_only {
        "HIT"
    } else {
        "BLOCK"
    }
}

/// Keep the first diagnostic sentence without treating a dot in `file.rs`,
/// `package.json`, or a decimal as the sentence boundary.
fn finding_summary(reason: &str) -> &str {
    let first_line = reason.lines().next().unwrap_or("finding").trim();
    first_line
        .find(". ")
        .map_or(first_line, |boundary| &first_line[..boundary])
}

/// Result of an `npm audit --json` scan.
#[derive(Debug, Default)]
pub struct NpmAuditResult {
    pub critical: usize,
    pub high: usize,
    pub moderate: usize,
    pub low: usize,
}

impl NpmAuditResult {
    fn total(&self) -> usize {
        self.critical + self.high + self.moderate + self.low
    }
}

/// How long to wait for `npm audit` before giving up. `npm audit` reaches out
/// to the registry and can stall indefinitely (a hung registry, a proxy, a
/// broken lockfile). 60s is generous for a real audit yet bounds a stuck one.
const NPM_AUDIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Run `cmd` to completion, capping the wait at `timeout`. Returns
/// `Ok(Some(stdout))` when the child exits within budget, `Ok(None)` when it
/// overruns (the child is killed + reaped — fail-open), or `Err(..)` only when
/// the child can't be spawned or polled.
///
/// stdout is drained on a worker thread so a large report can't fill the OS
/// pipe buffer and deadlock the poll loop; a killed child closes the pipe,
/// which ends the read, so the thread always terminates.
fn run_capturing_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<Option<String>> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut out) = stdout {
            let _ = out.read_to_string(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            break; // child exited within budget
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // Do NOT join the reader here. A shell child can fork a grandchild
            // (e.g. `sh -c "sleep 10"` -> `sleep`) that inherits the stdout pipe and
            // holds it open until IT exits, so `read_to_string` would block far past
            // our budget. On timeout we skip the audit and discard its output, so we
            // detach the reader and return promptly (it ends when the pipe closes).
            return Ok(None); // timed out — fail-open (skip), promptly
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(Some(reader.join().unwrap_or_default()))
}

/// Run `npm audit --json` and count vulnerabilities by severity (UD-SEC-016).
/// Returns an error only if npm isn't available or the command can't be
/// spawned; a successful run with zero vulns returns an all-zero result.
///
/// **Bounded + fail-open.** The subprocess is capped at [`NPM_AUDIT_TIMEOUT`];
/// if it overruns, the child is killed and an all-zero result is returned (the
/// audit is SKIPPED, never hangs CI).
fn npm_audit(project_root: &Path) -> std::io::Result<NpmAuditResult> {
    let mut cmd = umadev_host::std_command("npm");
    cmd.args(["audit", "--json"]).current_dir(project_root);
    // npm audit exits non-zero when vulns are found, but stdout still has JSON.
    match run_capturing_with_timeout(cmd, NPM_AUDIT_TIMEOUT)? {
        Some(text) => Ok(parse_npm_audit(&text).unwrap_or_default()),
        None => Ok(NpmAuditResult::default()), // timed out → skip (fail-open)
    }
}

/// Parse `npm audit --json` output into a severity-count summary.
/// Handles both npm 7+ format (top-level `vulnerabilities` map) and the
/// legacy `metadata.vulnerabilities` format.
fn parse_npm_audit(text: &str) -> Option<NpmAuditResult> {
    let val: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut result = NpmAuditResult::default();
    // npm 7+: top-level "vulnerabilities" object with per-advisory "severity".
    if let Some(vulns) = val.get("vulnerabilities").and_then(|v| v.as_object()) {
        for (_, info) in vulns {
            let severity = info.get("severity").and_then(|s| s.as_str()).unwrap_or("");
            match severity {
                "critical" => result.critical += 1,
                "high" => result.high += 1,
                "moderate" => result.moderate += 1,
                "low" => result.low += 1,
                _ => {}
            }
        }
        return Some(result);
    }
    // Legacy: "metadata.vulnerabilities" with counts.
    if let Some(meta) = val.get("metadata").and_then(|m| m.get("vulnerabilities")) {
        let get = |k: &str| meta.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        result.critical = usize::try_from(get("critical")).unwrap_or(0);
        result.high = usize::try_from(get("high")).unwrap_or(0);
        result.moderate = usize::try_from(get("moderate")).unwrap_or(0);
        result.low = usize::try_from(get("low")).unwrap_or(0);
        return Some(result);
    }
    None
}

/// Select governance-eligible files with an explicit, reproducible scope.
fn collect_source_files(root: &Path, changed_only: bool) -> std::io::Result<FileSelection> {
    if changed_only {
        return Ok(FileSelection {
            files: git_changed_files(root)?,
            scope: CiScanScope::StagedIndex,
        });
    }
    if let Some(files) = git_worktree_files(root) {
        return Ok(FileSelection {
            files,
            scope: CiScanScope::GitWorktree,
        });
    }
    let mut files = Vec::new();
    walk_dir(root, &mut files);
    sort_scan_files(root, &mut files);
    Ok(FileSelection {
        files,
        scope: CiScanScope::FilesystemFallback,
    })
}

fn normalized_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn sort_scan_files(root: &Path, files: &mut Vec<PathBuf>) {
    files.sort_by_key(|path| normalized_relative_path(root, path));
    files.dedup();
}

/// In a Git worktree, scan tracked files plus untracked files not excluded by
/// standard ignore rules. A failed command means Git metadata is unavailable,
/// so the caller uses the documented filesystem fallback.
fn git_worktree_files(root: &Path) -> Option<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "core.quotePath=false",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(scan_paths_from_git_output(root, &output.stdout))
}

/// Recursive directory walk collecting source files.
fn walk_dir(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            // Skip deps/build/VCS directories.
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            // Skip build/VCS dirs and dot-dirs BY DEFAULT, but DESCEND into a small
            // allowlist of security-relevant dot-dirs (.ssh / .aws / .github / ...) so a
            // committed secret or a weakened workflow there is scanned (it was silently
            // skipped by the blanket dot-prefix rule).
            if should_skip_directory(name) {
                continue;
            }
            walk_dir(&path, files);
        } else if ft.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            // Source by extension, OR a sensitive path regardless of extension, so
            // a committed `.env` / `*.pem` / `credentials` reaches the floor in a
            // full scan too (the same set the changed-only path pulls in).
            if SCAN_EXTENSIONS.contains(&ext.as_str())
                || is_sensitive_scan_path(&path.to_string_lossy())
            {
                files.push(path);
            }
        }
    }
}

fn should_skip_directory(name: &str) -> bool {
    SKIP_DIRS.contains(&name) || (name.starts_with('.') && !SCAN_DOT_DIRS.contains(&name))
}

/// Whether a repository-relative path lives under a directory excluded from
/// governance scans. Only parent components are inspected, so a sensitive file
/// such as `.env` is not mistaken for a dot-directory.
fn has_skipped_directory(path: &Path) -> bool {
    path.parent().is_some_and(|parent| {
        parent.components().any(|component| {
            let std::path::Component::Normal(name) = component else {
                return false;
            };
            name.to_str().is_some_and(should_skip_directory)
        })
    })
}

fn is_scan_candidate(path: &str) -> bool {
    let path_obj = Path::new(path);
    if has_skipped_directory(path_obj) {
        return false;
    }
    let ext = path_obj
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    SCAN_EXTENSIONS.contains(&ext.as_str()) || is_sensitive_scan_path(path)
}

fn scan_paths_from_git_output(root: &Path, output: &[u8]) -> Vec<PathBuf> {
    let text = String::from_utf8_lossy(output);
    let mut files: Vec<PathBuf> = text
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter(|path| is_scan_candidate(path))
        .map(|path| root.join(path))
        .collect();
    sort_scan_files(root, &mut files);
    files
}

/// Get the files in the STAGED index that differ from `HEAD` — the exact set a
/// commit would capture. This powers the pre-commit hook, so it must be the
/// staged scope (`--cached`), NOT the working tree: a `git diff HEAD` would also
/// include unstaged edits, blocking a commit on a violation that isn't part of
/// it. With no commits yet, `--cached` compares against the empty tree (all
/// staged files appear as new). Git failures yield an empty set (fail-open),
/// never a broader tracked-file fallback that would violate `--changed-only`.
fn git_changed_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    // `-c core.quotePath=false` + `-z`: emit NUL-separated, UNQUOTED paths so a
    // staged file with a non-ASCII (`café.tsx`) or spaced name is scanned rather
    // than dropped. At git's default (`core.quotePath=true`) such a path is
    // octal-escaped + double-quoted (`"caf\303\251.tsx"`), so `extension()`
    // yields `tsx"` and it silently falls out of SCAN_EXTENSIONS — a real
    // violation would never be scanned. `-z` also removes the quoting entirely,
    // so the raw path round-trips to `git show :<rel>` in `read_staged_blob`.
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "core.quotePath=false",
            "diff",
            "--name-only",
            "-z",
            "--cached",
            "--diff-filter=ACMR",
        ])
        .current_dir(root)
        .output();
    let out = match output {
        Ok(output) if output.status.success() => output.stdout,
        _ => return Ok(Vec::new()),
    };
    Ok(scan_paths_from_git_output(root, &out))
}

/// Read the STAGED content of `rel` (a workspace-relative, forward-slash path)
/// from the git index via `git show :<rel>`. Returns `None` when the path isn't
/// staged, the blob is binary/unreadable, or git is unavailable (fail-open: the
/// caller skips the file).
fn read_staged_blob(root: &Path, rel: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["show", &format!(":{rel}")])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // A staged binary blob isn't valid UTF-8 — treat as unreadable (skip), the
    // same as the on-disk `read_to_string` path does.
    String::from_utf8(output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_scans_clean_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("clean.ts"), "export const x: number = 1;").unwrap();
        let result = run(&CiOptions {
            report_only: false,
            changed_only: false,
            project_root: tmp.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_scanned, 1);
        assert_eq!(result.files_blocked, 0);
        assert!(!result.failed);
    }

    /// Run `umadev ci` over `root` and return how many files it blocked.
    fn ci_blocked(root: &Path) -> usize {
        run(&CiOptions {
            report_only: false,
            changed_only: false,
            project_root: root.to_path_buf(),
        })
        .unwrap()
        .files_blocked
    }

    /// The gate must judge by the SAME rule book the run does — and the DEFAULT run path
    /// must actually write that rule book.
    ///
    /// `umadev ci` is the surface that actually blocks — the PreToolUse hook downgrades every
    /// non-floor finding to a pass, and `install --base pre-commit` writes
    /// `umadev ci --changed-only` into `.git/hooks/pre-commit`. So while the run honoured
    /// "our brand is violet" and wrote the palette, this gate blocked UD-CODE-002 on that
    /// very color and the user COULD NOT COMMIT the brand they asked for. There is no fix
    /// from inside that loop; the finding had just been relocated one surface over.
    ///
    /// The context file was only ever written by the legacy gated walk and the single-shot
    /// runner — never by the DEFAULT director path — so this is driven here through the same
    /// entry point the director loop calls, not a hand-written JSON blob that could pass while
    /// the product path still wrote nothing.
    #[test]
    fn ci_honours_the_runs_governance_context() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // The hue the user asked for, written as the run wrote it. Named hues, so the ONLY
        // rule with anything to say about this file is the banned-hue one — the one the
        // permission governs (a hardcoded-color finding would be a different, still-correct
        // complaint about literals vs tokens).
        let requested_purple = "export const hero = 'linear-gradient(135deg, purple, pink)';";
        std::fs::write(root.join("brand.ts"), requested_purple).unwrap();

        // No context ⇒ default-REJECT: a purple nobody asked for is still caught.
        assert_eq!(
            ci_blocked(root),
            1,
            "with no recorded permission the banned hue still blocks (default-reject)"
        );

        // THE RUN. The same call `director_loop` makes at its door, before it writes a single
        // file — carrying the BRAIN's verdict on the requirement (`color_permission`), which is
        // the only thing that may grant this permission. A word list used to answer it here and
        // leaked on every review round; a run whose brain says "yes, they chose violet" records
        // that, and this gate must read the same decision.
        let requirement = "做一个品牌落地页,主色用紫色 #7c3aed 的渐变";
        let ctx = umadev_agent::planner::persist_project_context_with_color(
            requirement,
            root,
            "brand",
            true,
        );
        assert!(
            ctx.purple_allowed,
            "the run recorded the brain's grant — the context must carry it"
        );

        // …and the commit gate now reads the SAME decision.
        assert_eq!(
            ci_blocked(root),
            0,
            "the user asked for this color and the run agreed — the commit gate cannot be the \
             one surface that says no"
        );

        // A LEGITIMATELY CURRENT context stays honoured however old it gets: the workspace's
        // live requirement still matches the one the permission was derived from.
        let state = umadev_agent::state::WorkflowState {
            requirement: requirement.to_string(),
            ..umadev_agent::state::WorkflowState::new(umadev_spec::Phase::Frontend)
        };
        umadev_agent::state::write_workflow_state(root, &state).unwrap();
        assert_eq!(
            ci_blocked(root),
            0,
            "a context that matches the workspace's own requirement is current — blocking it \
             would re-open the very bug this test exists for"
        );
    }

    /// A permission belongs to the requirement it was derived from. `umadev ci` runs from
    /// `.git/hooks/pre-commit` — long after the run that wrote the context, on whatever the
    /// tree is now — so a `purple_allowed: true` left behind by an OLD run must not stand the
    /// banned-hue band down for a NEW requirement whose first line is "no purple".
    #[test]
    fn a_stale_context_from_a_different_requirement_does_not_stand_the_rule_down() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("brand.ts"),
            "export const hero = 'linear-gradient(135deg, purple, pink)';",
        )
        .unwrap();

        // LAST quarter's run: the brand really was violet, the brain said so, and the run
        // recorded the permission.
        let old = umadev_agent::planner::persist_project_context_with_color(
            "品牌主色用紫色",
            root,
            "brand",
            true,
        );
        assert!(old.purple_allowed);
        assert_eq!(ci_blocked(root), 0, "that run's own commit was fine");

        // THIS quarter's requirement is the opposite — and it is what the workspace is being
        // built from now. The permission on disk belongs to a requirement that is no longer
        // the one in force, so it is not evidence for anything.
        let state = umadev_agent::state::WorkflowState {
            requirement: "重做品牌:不要任何紫色".to_string(),
            ..umadev_agent::state::WorkflowState::new(umadev_spec::Phase::Frontend)
        };
        umadev_agent::state::write_workflow_state(root, &state).unwrap();
        assert_eq!(
            ci_blocked(root),
            1,
            "a permission derived from a DIFFERENT requirement is not a permission for this one"
        );

        // And the new run's own door re-decides it (its brain reads a requirement that forbids
        // the hue), so the band is armed on disk too — not merely ignored at read time.
        let fresh = umadev_agent::planner::persist_project_context_with_color(
            "重做品牌:不要任何紫色",
            root,
            "brand",
            false,
        );
        assert!(!fresh.purple_allowed);
        assert_eq!(ci_blocked(root), 1);

        // THE CARRY-FORWARD, which is what every later surface depends on: the per-tool-call
        // refresh (`persist_project_context`) has no brain and must NEVER invent a permission.
        // For the requirement the door already decided, it reproduces that decision; for any
        // other, it grants nothing.
        let carried =
            umadev_agent::planner::persist_project_context("重做品牌:不要任何紫色", root, "brand");
        assert!(
            !carried.purple_allowed,
            "the refresh carries the door's verdict forward — it does not re-derive one"
        );
        assert_eq!(ci_blocked(root), 1);
    }

    /// Git runs its hooks with the cwd set to the repository TOP LEVEL. In a monorepo whose
    /// UmaDev workspace is `apps/web`, the pre-commit gate therefore runs at `/repo` — where
    /// there is no `.umadev/` at all — while the files it judges live inside a real workspace
    /// one level down. A root-only context lookup finds nothing, falls back to strict, and
    /// blocks the color the run in `apps/web` had accepted: HIGH 1 all over again, one
    /// directory deeper.
    #[test]
    fn a_workspace_in_a_monorepo_subdir_is_still_governed_by_its_own_context() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path(); // the git top-level: no .umadev of its own
        let web = repo.join("apps").join("web");
        std::fs::create_dir_all(&web).unwrap();
        let purple = "export const hero = 'linear-gradient(135deg, purple, pink)';";
        std::fs::write(web.join("brand.ts"), purple).unwrap();

        // Nothing recorded anywhere → strict, as always.
        assert_eq!(ci_blocked(repo), 1);

        // The run happened INSIDE apps/web, and wrote its context there.
        let ctx = umadev_agent::planner::persist_project_context_with_color(
            "做一个品牌落地页,主色用紫色",
            &web,
            "brand",
            true,
        );
        assert!(ctx.purple_allowed);

        // The pre-commit gate still runs at the repo top level — and must find it.
        assert_eq!(
            ci_blocked(repo),
            0,
            "the gate at the git top level must judge apps/web by apps/web's own rule book"
        );

        // A sibling package with NO workspace of its own is still governed strictly: the
        // permission belongs to apps/web, not to the whole monorepo.
        let api = repo.join("apps").join("api");
        std::fs::create_dir_all(&api).unwrap();
        std::fs::write(api.join("theme.ts"), purple).unwrap();
        assert_eq!(
            ci_blocked(repo),
            1,
            "a permission recorded in one package does not leak into its siblings"
        );
    }

    /// An UNSTAMPED context (hand-written, or from a build that predates the provenance
    /// fields) has nothing to date it or attribute it to — so it cannot stand a rule down.
    #[test]
    fn an_unstamped_context_is_not_a_permission() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("brand.ts"),
            "export const hero = 'linear-gradient(135deg, purple, pink)';",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".umadev")).unwrap();
        std::fs::write(
            root.join(".umadev").join("governance-context.json"),
            r#"{"static_frontend_only":false,"purple_allowed":true}"#,
        )
        .unwrap();
        assert_eq!(
            ci_blocked(root),
            1,
            "a permission with no provenance is not honoured — anyone could drop that file in"
        );
    }

    #[test]
    fn ci_context_is_conservative_when_unreadable() {
        // FAIL-OPEN, in the SAFE direction: a malformed / partial context is not a permission.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".umadev")).unwrap();
        for body in ["{ not json", "{}", ""] {
            std::fs::write(root.join(".umadev").join("governance-context.json"), body).unwrap();
            let ctx = load_project_context(root);
            assert!(
                !ctx.purple_allowed,
                "an unreadable context is never a stand-down: {body:?}"
            );
        }
        // …and a missing file, likewise.
        std::fs::remove_file(root.join(".umadev").join("governance-context.json")).unwrap();
        assert!(!load_project_context(root).purple_allowed);
    }

    #[test]
    fn ci_flags_violation() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("bad.tsx"), "<b>🔍</b>").unwrap();
        let result = run(&CiOptions {
            report_only: false,
            changed_only: false,
            project_root: tmp.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_blocked, 1);
        assert_eq!(result.governance_findings, 1);
        assert!(result.failed);
    }

    #[test]
    fn ci_report_only_does_not_fail() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("bad.tsx"), "<b>🔍</b>").unwrap();
        let result = run(&CiOptions {
            report_only: true,
            changed_only: false,
            project_root: tmp.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_blocked, 1);
        assert_eq!(result.governance_findings, 1);
        assert_eq!(finding_label(true), "HIT");
        assert_eq!(finding_label(false), "BLOCK");
        assert!(!result.failed); // report-only → exit 0
    }

    #[test]
    fn finding_summary_preserves_file_extensions_and_line_numbers() {
        assert_eq!(
            finding_summary(
                "UmaDev: deep nesting at `src/app.rs:42` (UG-LINT-004). Extract a helper."
            ),
            "UmaDev: deep nesting at `src/app.rs:42` (UG-LINT-004)"
        );
        assert_eq!(finding_summary("one-line finding"), "one-line finding");
    }

    #[test]
    fn ci_report_only_emits_all_enabled_findings_per_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("bad.ts"),
            "export function echo(value: any) { console.log(value); return value; }",
        )
        .unwrap();
        let result = run(&CiOptions {
            report_only: true,
            changed_only: false,
            project_root: tmp.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_blocked, 1);
        assert!(result.governance_findings >= 2, "{result:?}");
        assert!(!result.failed);
    }

    #[test]
    fn ci_skips_node_modules() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("node_modules")).unwrap();
        // A violation inside node_modules must NOT be scanned.
        std::fs::write(tmp.path().join("node_modules/x.tsx"), "<b>🔍</b>").unwrap();
        std::fs::write(tmp.path().join("clean.ts"), "export const x = 1;").unwrap();
        let result = run(&CiOptions {
            report_only: false,
            changed_only: false,
            project_root: tmp.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_blocked, 0);
        assert!(!result.failed);
    }

    #[test]
    fn ci_respects_disabled_policy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sd_dir = tmp.path().join(".umadev");
        std::fs::create_dir_all(&sd_dir).unwrap();
        std::fs::write(
            sd_dir.join("rules.toml"),
            "[disabled]\nclauses = [\"UD-CODE-001\"]\n",
        )
        .unwrap();
        // Emoji is UD-CODE-001 — disabled → should pass.
        std::fs::write(tmp.path().join("bad.tsx"), "<b>🔍</b>").unwrap();
        let result = run(&CiOptions {
            report_only: false,
            changed_only: false,
            project_root: tmp.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_blocked, 0);
    }

    #[test]
    fn walk_collects_only_source_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("app.ts"), "x").unwrap();
        std::fs::write(tmp.path().join("readme.md"), "x").unwrap();
        std::fs::write(tmp.path().join("data.json"), "x").unwrap();
        let mut files = Vec::new();
        walk_dir(tmp.path(), &mut files);
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"app.ts".to_string()));
        assert!(!names.contains(&"readme.md".to_string()));
        assert!(!names.contains(&"data.json".to_string()));
    }

    #[test]
    fn filesystem_fallback_selection_is_sorted_and_explicit() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("nested")).unwrap();
        std::fs::write(tmp.path().join("z.ts"), "export const z = 1;").unwrap();
        std::fs::write(
            tmp.path().join("nested").join("m.ts"),
            "export const m = 1;",
        )
        .unwrap();
        std::fs::write(tmp.path().join("a.ts"), "export const a = 1;").unwrap();

        let selection = collect_source_files(tmp.path(), false).unwrap();
        assert_eq!(selection.scope, CiScanScope::FilesystemFallback);
        let paths: Vec<String> = selection
            .files
            .iter()
            .map(|path| normalized_relative_path(tmp.path(), path))
            .collect();
        assert_eq!(paths, ["a.ts", "nested/m.ts", "z.ts"]);
    }

    #[test]
    fn selected_and_actually_scanned_counts_are_distinct() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("clean.ts"), "export const clean = 1;").unwrap();
        std::fs::write(tmp.path().join("binary.ts"), [0xff, 0xfe]).unwrap();

        let result = run(&CiOptions {
            report_only: true,
            changed_only: false,
            project_root: tmp.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_selected, 2);
        assert_eq!(result.files_scanned, 1);
    }

    #[test]
    fn walk_collects_file_types_with_active_governance_rules() {
        let tmp = tempfile::TempDir::new().unwrap();
        let expected = [
            "workflow.yml",
            "config.yaml",
            "script.sh",
            "script.bash",
            "script.zsh",
            "styles.css",
            "module.mjs",
            "index.html",
        ];
        for name in expected {
            std::fs::write(tmp.path().join(name), "clean fixture").unwrap();
        }

        let mut files = Vec::new();
        walk_dir(tmp.path(), &mut files);
        let names: std::collections::HashSet<String> = files
            .iter()
            .map(|file| file.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        for name in expected {
            assert!(names.contains(name), "{name} must be scanned: {names:?}");
        }
    }

    #[test]
    fn ci_skips_generated_out_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("out")).unwrap();
        std::fs::write(
            tmp.path().join("out/generated.mjs"),
            "export const icon = '🚀';",
        )
        .unwrap();
        std::fs::write(tmp.path().join("clean.mjs"), "export const label = 'Save';").unwrap();

        let result = run(&CiOptions {
            report_only: false,
            changed_only: false,
            project_root: tmp.path().to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_scanned, 1, "generated out/ must be skipped");
        assert_eq!(result.files_blocked, 0);
        assert!(!result.failed);
    }

    // --- M2: changed-only uses the STAGED index, not the working tree -------

    /// Run a git command in `dir`; returns false if git is missing/fails.
    fn git(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Init a throwaway repo with a committed identity, or `false` if git is
    /// unavailable (the caller then skips — no hard git dependency in tests).
    fn init_repo(dir: &Path) -> bool {
        git(dir, &["init", "-q"])
            && git(dir, &["config", "user.email", "t@t.test"])
            && git(dir, &["config", "user.name", "test"])
    }

    #[test]
    fn full_git_scope_includes_tracked_and_untracked_nonignored_in_stable_order() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        if !init_repo(root) {
            return;
        }
        std::fs::write(root.join(".gitignore"), "ignored.ts\nforced.ts\n").unwrap();
        std::fs::write(root.join("tracked.ts"), "export const tracked = 1;\n").unwrap();
        std::fs::write(root.join("forced.ts"), "export const forced = 1;\n").unwrap();
        std::fs::write(root.join("untracked.ts"), "export const local = 1;\n").unwrap();
        std::fs::write(root.join("ignored.ts"), "export const ignored = 1;\n").unwrap();
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(root.join(".github/workflows/ci.yml"), "name: CI\n").unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        std::fs::write(root.join(".hidden/local.ts"), "export const hidden = 1;\n").unwrap();
        std::fs::create_dir(root.join("node_modules")).unwrap();
        std::fs::write(
            root.join("node_modules/generated.ts"),
            "export const generated = 1;\n",
        )
        .unwrap();
        assert!(git(root, &["add", "tracked.ts"]));
        assert!(git(root, &["add", "-f", "forced.ts"]));

        let selection = collect_source_files(root, false).unwrap();
        assert_eq!(selection.scope, CiScanScope::GitWorktree);
        let paths: Vec<String> = selection
            .files
            .iter()
            .map(|path| normalized_relative_path(root, path))
            .collect();
        assert_eq!(
            paths,
            [
                ".github/workflows/ci.yml",
                "forced.ts",
                "tracked.ts",
                "untracked.ts",
            ]
        );
    }

    #[test]
    fn changed_only_scans_staged_blob_not_dirty_working_tree() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        if !init_repo(root) {
            return; // git not available — skip
        }
        let file = root.join("app.tsx");
        // Commit a clean baseline.
        std::fs::write(&file, "export const x = 1;\n").unwrap();
        assert!(git(root, &["add", "app.tsx"]));
        assert!(git(root, &["commit", "-q", "-m", "base"]));
        // STAGE a different but still CLEAN version (so it appears in --cached).
        std::fs::write(&file, "export const y = 2;\n").unwrap();
        assert!(git(root, &["add", "app.tsx"]));
        // Dirty the WORKING TREE with an emoji violation — but do NOT stage it.
        std::fs::write(&file, "<b>\u{1f50d}</b>\n").unwrap();

        let result = run(&CiOptions {
            report_only: false,
            changed_only: true,
            project_root: root.to_path_buf(),
        })
        .unwrap();
        // The staged version is clean → no block, even though the working copy
        // (which the OLD `git diff HEAD` + on-disk read judged) is dirty.
        assert_eq!(
            result.files_blocked, 0,
            "must judge the STAGED blob, not the dirty working copy"
        );
        assert!(!result.failed);
    }

    #[test]
    fn changed_only_scans_new_rule_extensions_and_skips_generated_out() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        if !init_repo(root) {
            return; // git not available — skip
        }
        std::fs::create_dir(root.join("out")).unwrap();
        std::fs::write(root.join("workflow.yaml"), "name: clean\n").unwrap();
        std::fs::write(
            root.join("out/generated.mjs"),
            "export const icon = '🚀';\n",
        )
        .unwrap();
        assert!(git(root, &["add", "."]));

        let listed = git_changed_files(root).unwrap();
        assert!(
            listed.iter().any(|path| path.ends_with("workflow.yaml")),
            "YAML must enter changed-only governance: {listed:?}"
        );
        assert!(
            listed
                .iter()
                .all(|path| !path.ends_with("out/generated.mjs")),
            "generated out/ must stay outside changed-only governance: {listed:?}"
        );

        let result = run(&CiOptions {
            report_only: false,
            changed_only: true,
            project_root: root.to_path_buf(),
        })
        .unwrap();
        assert_eq!(result.files_scanned, 1);
        assert_eq!(result.files_blocked, 0);
        assert!(!result.failed);
    }

    #[test]
    fn changed_only_flags_a_violation_in_the_staged_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        if !init_repo(root) {
            return; // git not available — skip
        }
        let file = root.join("app.tsx");
        std::fs::write(&file, "export const x = 1;\n").unwrap();
        assert!(git(root, &["add", "app.tsx"]));
        assert!(git(root, &["commit", "-q", "-m", "base"]));
        // STAGE a version WITH a violation; clean it up in the working tree.
        std::fs::write(&file, "<b>\u{1f50d}</b>\n").unwrap();
        assert!(git(root, &["add", "app.tsx"]));
        std::fs::write(&file, "export const ok = 3;\n").unwrap(); // clean working copy

        let result = run(&CiOptions {
            report_only: false,
            changed_only: true,
            project_root: root.to_path_buf(),
        })
        .unwrap();
        // The STAGED blob carries the violation → blocked, regardless of the
        // now-clean working copy.
        assert_eq!(result.files_blocked, 1, "staged violation must be flagged");
        assert!(result.failed);
    }

    #[test]
    fn changed_only_blocks_a_staged_dotenv_secret() {
        // REPRODUCTION: a staged `.env` carrying a live Stripe key. `.env` has no
        // source extension, so the OLD collect-by-extension scan never saw it —
        // "0 file(s) scanned, 0 blocked, exit 0". The floor must now pull the
        // sensitive path into scope and BLOCK it (UD-SEC-001 path guard), failing
        // CI with a non-zero exit.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        if !init_repo(root) {
            return; // git not available — skip
        }
        std::fs::write(
            root.join(".env"),
            "STRIPE_SECRET_KEY=aB3xK9pQ7mNr2WvT5sZ8dF1gH4jL6cE0\n",
        )
        .unwrap();
        assert!(git(root, &["add", ".env"]));

        // Sanity: the sensitive path is now in the changed-file scan set despite
        // having no source extension.
        let listed = git_changed_files(root).unwrap();
        assert!(
            listed.iter().any(|p| p.ends_with(".env")),
            "a staged .env must be listed for scanning, got {listed:?}"
        );

        let result = run(&CiOptions {
            report_only: false,
            changed_only: true,
            project_root: root.to_path_buf(),
        })
        .unwrap();
        assert!(
            result.files_blocked >= 1,
            "a staged .env secret must be blocked, not silently skipped"
        );
        assert!(result.failed, "and it must fail CI (non-zero exit)");
    }

    #[test]
    fn changed_only_blocks_a_secret_in_a_no_extension_file() {
        // A staged `credentials` file (no extension) with a live secret must be
        // scanned + blocked too — the sensitive-path scope is not limited to
        // dotenv files.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        if !init_repo(root) {
            return; // git not available — skip
        }
        std::fs::write(
            root.join("credentials"),
            "aws_secret_access_key = aB3xK9pQ7mNr2WvT5sZ8dF1gH4jL6cE0\n",
        )
        .unwrap();
        assert!(git(root, &["add", "credentials"]));
        let result = run(&CiOptions {
            report_only: false,
            changed_only: true,
            project_root: root.to_path_buf(),
        })
        .unwrap();
        assert!(
            result.files_blocked >= 1,
            "a staged credentials secret must block"
        );
        assert!(result.failed);
    }

    #[test]
    fn is_sensitive_scan_path_matches_the_floor_set() {
        // The extension-agnostic scan predicate covers the dotenv/cert/key set …
        assert!(is_sensitive_scan_path(".env"));
        assert!(is_sensitive_scan_path("apps/api/.env.production"));
        assert!(is_sensitive_scan_path(".env.staging")); // arbitrary dotenv variant
        assert!(is_sensitive_scan_path("certs/server.pem"));
        assert!(is_sensitive_scan_path("deploy/id_rsa"));
        assert!(is_sensitive_scan_path("secrets/credentials"));
        assert!(is_sensitive_scan_path(".ssh/known_hosts"));
        // … and does NOT sweep in ordinary source (that rides SCAN_EXTENSIONS).
        assert!(!is_sensitive_scan_path("src/messages.ts"));
        assert!(!is_sensitive_scan_path("README.md"));
        // A dotenv TEMPLATE is in scan SCOPE (any `.env.*`), but the floor's path
        // guard never auto-blocks it — only its content is judged, so a
        // placeholder file passes while a real `.env` is blocked on the path.
        assert!(is_sensitive_scan_path(".env.example"));
        assert!(!check_sensitive_path(".env.example", "").block);
    }

    #[test]
    fn changed_only_scans_non_ascii_staged_filename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        if !init_repo(root) {
            return; // git not available — skip
        }
        // A non-ASCII filename: at git's default core.quotePath=true, `git diff
        // --name-only --cached` would emit `"caf\303\251.tsx"` (octal-escaped +
        // quoted), so `extension()` sees `tsx"` and the file drops out of the
        // scan. The -c core.quotePath=false + -z fix must scan it.
        let file = root.join("café.tsx");
        std::fs::write(&file, "<b>\u{1f50d}</b>\n").unwrap(); // emoji violation
        assert!(git(root, &["add", "café.tsx"]));

        // Sanity: git_changed_files must surface the non-ASCII path (unquoted).
        let listed = git_changed_files(root).unwrap();
        assert!(
            listed.iter().any(|p| p.ends_with("café.tsx")),
            "the non-ASCII staged path must be listed unquoted, got {listed:?}"
        );

        let result = run(&CiOptions {
            report_only: false,
            changed_only: true,
            project_root: root.to_path_buf(),
        })
        .unwrap();
        assert_eq!(
            result.files_blocked, 1,
            "a violation in a non-ASCII staged filename must be scanned + blocked"
        );
        assert!(result.failed);
    }

    #[test]
    fn scan_summary_distinguishes_enforcing_and_complete_report_counts() {
        let result = CiResult {
            files_selected: 4,
            files_scanned: 3,
            files_blocked: 1,
            governance_findings: 3,
            npm_audit_findings: 2,
            scan_scope: CiScanScope::GitWorktree,
            failed: true,
        };
        let line = scan_summary(&result, true);
        assert!(line.contains("tracked + untracked non-ignored"), "{line}");
        assert!(line.contains("4 file(s) selected, 3 scanned"), "{line}");
        assert!(line.contains("1 unreadable/binary skipped"), "{line}");
        assert!(line.contains("1 file(s) with a governance hit"), "{line}");
        assert!(line.contains("3 governance finding(s)"), "{line}");
        assert!(line.contains("count is complete"), "{line}");
        assert!(
            line.contains("2 high/critical npm-audit finding(s)"),
            "{line}"
        );
        assert!(line.contains("Report-only mode"), "{line}");
        let enforcing = scan_summary(&result, false);
        assert!(enforcing.contains("3 first-hit finding(s)"), "{enforcing}");
        assert!(enforcing.contains("run --report-only"), "{enforcing}");
    }

    // --- UD-SEC-016: npm audit parsing ----------------------------------

    #[test]
    fn npm_audit_parses_npm7_format() {
        let json = r#"{"vulnerabilities":{"lodash":{"severity":"high"},"react":{"severity":"critical"},"left-pad":{"severity":"low"}}}"#;
        let result = parse_npm_audit(json).unwrap();
        assert_eq!(result.critical, 1);
        assert_eq!(result.high, 1);
        assert_eq!(result.low, 1);
    }

    #[test]
    fn npm_audit_parses_legacy_format() {
        let json =
            r#"{"metadata":{"vulnerabilities":{"critical":2,"high":3,"moderate":1,"low":0}}}"#;
        let result = parse_npm_audit(json).unwrap();
        assert_eq!(result.critical, 2);
        assert_eq!(result.high, 3);
        assert_eq!(result.moderate, 1);
    }

    #[test]
    fn npm_audit_parses_clean() {
        let json = r#"{"vulnerabilities":{}}"#;
        let result = parse_npm_audit(json).unwrap();
        assert_eq!(result.total(), 0);
    }

    #[test]
    fn npm_audit_returns_none_on_garbage() {
        assert!(parse_npm_audit("not json").is_none());
    }

    // --- bounded npm-audit wait (never hangs CI) ------------------------

    #[cfg(unix)]
    #[test]
    fn capturing_timeout_kills_a_stuck_child_fast() {
        use std::time::{Duration, Instant};
        // A child that would run for 10s, capped at 200ms → returns None (skip)
        // WELL before the child's own runtime, and quickly.
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("sleep 10");
        let started = Instant::now();
        let out = run_capturing_with_timeout(cmd, Duration::from_millis(200)).unwrap();
        let elapsed = started.elapsed();
        assert!(
            out.is_none(),
            "an overrunning audit must be skipped, not returned"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "the wait must be bounded, took {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn capturing_returns_stdout_when_child_exits_in_time() {
        use std::time::Duration;
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg("printf 'hello-audit'");
        let out = run_capturing_with_timeout(cmd, Duration::from_secs(10)).unwrap();
        assert_eq!(out.as_deref(), Some("hello-audit"));
    }
}
