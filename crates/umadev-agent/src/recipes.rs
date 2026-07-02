//! Durable cross-project **SUCCESS-RECIPE** memory — the store that lets the team
//! learn from its WINS, not just its failures.
//!
//! ## Why this exists (the "learn from success" gap)
//!
//! UmaDev already has a rich failure→pitfall pipeline: a build that trips over a
//! dependency, a flaky test, or a contract drift is captured as a pitfall
//! ([`crate::lessons`]), given a frequency signal, and recalled the next time a
//! similar error looms. But when a deliberate build passes CLEANLY the *winning
//! approach* — the plan shape that worked, the scaffold it produced, the patterns
//! it leaned on — is DISCARDED. A memory audit called this the single biggest
//! "team gets stronger" gap: the reusable-SOP capability a real senior team
//! accretes over many clean deliveries. This module is that place.
//!
//! It is the WIN sibling of the pitfall ledger, modelled on the same
//! read/recall/bound/fail-open pattern as the durable fact + open-decision stores
//! ([`crate::project_facts`] / [`crate::open_decisions`]) — but it lives in the
//! **cross-project global tier** under the user's home (like the promoted global
//! lessons in [`crate::lessons::global_learned_dir`]) and is keyed by a task
//! **fingerprint** (stack + kind + rough feature shape), so a clean build of a
//! React dashboard in one project can prime the plan for the next React dashboard
//! in a *different* project.
//!
//! ## The loop
//!
//! - **CAPTURE** ([`capture_at_delivery`]) — at the SAME finalize/delivery seam
//!   where the memory reconcile runs ([`crate::self_evolve::reconcile_at_delivery`]),
//!   when a build settled CLEAN on a DELIBERATE route, distill a [`Recipe`] from the
//!   plan the team actually executed (the ordered step titles/seats that reached
//!   `Done`), the scaffold it produced (the concrete files its evidence contracts
//!   named), and the detected stack + requirement shape. The "patterns" +
//!   extra-scaffold enrichment MAY use ONE read-only forked brain consult; if that
//!   consult fails, a MECHANICAL recipe (skeleton + evidence-derived scaffold +
//!   stack) is still stored. A capture error NEVER affects delivery.
//! - **RECALL** ([`recall_prior_block`]) — when the coordinator is about to
//!   synthesize a plan for a NEW deliberate build, look up the closest recipe by
//!   fingerprint (exact stack+kind, else nearest above a floor) and inject it as a
//!   PRIOR into the plan-synthesis prompt ("a past clean build of a similar
//!   stack/feature used this shape — adapt if it fits"). It is a prior the brain can
//!   adapt or ignore, NEVER a forced template. No match ⇒ no-op (unchanged
//!   behaviour).
//!
//! ## Bounded + fail-open by contract
//!
//! The store is capped at [`MAX_RECIPES`] entries (lowest-evidence evicted) with
//! every list field capped ([`MAX_SKELETON_STEPS`] / [`MAX_SCAFFOLD`] /
//! [`MAX_PATTERNS`] / [`MAX_SHAPE_TOKENS`]) and each string field truncated, and the
//! recall block is capped at [`RECIPE_PRIOR_BUDGET`] characters — so neither the
//! store nor the prompt can bloat. A recipe is a PRIOR/suggestion, NEVER a gate: it
//! does not touch loop control, the deterministic floor, or any acceptance verdict.
//! Every path is fail-open: a missing/corrupt store, no home dir, an offline brain,
//! or a failed write degrades to "no recipe" and behaves exactly as before — this
//! module NEVER panics and NEVER returns an error that could block a delivery.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use umadev_runtime::BaseSession;

use crate::events::{EngineEvent, EventSink};
use crate::experts::excerpt;
use crate::plan_state::{EvidenceContract, Plan, StepKind, StepStatus};
use crate::router::RoutePlan;

/// Home-relative directory holding the cross-project recipe store. Mirrors the
/// promoted-global-lessons tier (`~/.umadev/learned`) so all durable cross-project
/// memory lives under one `~/.umadev` root.
pub const RECIPES_DIRNAME: &str = ".umadev/recipes";

/// The recipe store filename inside [`RECIPES_DIRNAME`] — an append-friendly JSONL
/// file (one self-contained [`Recipe`] JSON object per line).
pub const RECIPES_FILE: &str = "recipes.jsonl";

/// Optional environment override for the recipe store directory. When set (and
/// non-empty) it takes precedence over the home-based default — used so an operator
/// can relocate the store, and so the store can be isolated in a scratch dir. Read
/// fail-open (a blank value falls back to the home default).
pub const RECIPES_DIR_ENV: &str = "UMADEV_RECIPES_DIR";

/// Hard cap on distinct recipes retained on disk so a long-lived global store never
/// bloats. When exceeded, the LOWEST-evidence recipes (fewest clean builds) are
/// evicted first — the most-proven shapes are the ones worth keeping.
const MAX_RECIPES: usize = 128;

/// Per-recipe cap on plan-skeleton step lines. A skeleton is a compact "shape", not
/// the whole build log.
const MAX_SKELETON_STEPS: usize = 16;

/// Per-recipe cap on notable scaffold paths recorded.
const MAX_SCAFFOLD: usize = 24;

/// Per-recipe cap on short pattern notes recorded.
const MAX_PATTERNS: usize = 12;

/// Per-fingerprint cap on rough feature-shape tokens.
const MAX_SHAPE_TOKENS: usize = 12;

/// Per-line char cap for a skeleton step / scaffold path / pattern note, so one
/// runaway string can't dominate a recipe or the recall budget.
const MAX_LINE_CHARS: usize = 160;

/// Per-token char cap for a shape token.
const MAX_TOKEN_CHARS: usize = 40;

/// Character budget for the plan-time RECALL prior block. Tight by design: it rides
/// on TOP of the plan-synthesis prompt as a single adaptable prior, so it must stay a
/// small, high-signal overlay.
pub const RECIPE_PRIOR_BUDGET: usize = 1_400;

/// The minimum fingerprint similarity for a recipe to surface as a prior. Calibrated
/// so an EXACT stack match (or an exact kind match with shape overlap) clears the bar
/// but an unrelated stack+kind+shape does not — see [`similarity`]. Below this floor,
/// recall is a no-op (unchanged behaviour).
const MIN_RECALL_SIMILARITY: f32 = 0.35;

/// A task **fingerprint** — the key a recipe is stored + looked up under. Coarse on
/// purpose: `stack` + `kind` are the strong signals; `shape` is a rough,
/// order-insensitive token set distilled from the requirement so two "todo list"
/// builds on the same stack land near each other.
///
/// `shape` is normalised (lowercased, de-duplicated, SORTED, bounded) at construction
/// so `#[derive(PartialEq, Eq)]` is a true exact-fingerprint test for dedup/merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// The detected project stack (`node` / `rust` / `python` / `go` / `deno` /
    /// `none`) — [`crate::verify::ProjectKind::as_str`].
    pub stack: String,
    /// The routed task kind (`greenfield` / `frontend_only` / … ) —
    /// [`crate::planner::TaskKind::id`].
    pub kind: String,
    /// Rough feature-shape tokens distilled from the requirement (sorted, unique,
    /// bounded). May be empty (a bare requirement).
    #[serde(default)]
    pub shape: Vec<String>,
}

/// Outcome statistics for a recipe — the confidence signal a later run reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeStats {
    /// How many CLEAN deliberate deliveries have folded into (captured/merged) this
    /// recipe. Every capture is by definition a clean build, so this is a count of
    /// PROVEN-clean uses of this plan shape — the primary confidence weight.
    #[serde(default)]
    pub clean_builds: u32,
    /// How many times this recipe has been RECALLED as a plan-time prior.
    #[serde(default)]
    pub times_reused: u32,
    /// How many of those reuses went on to a clean delivery (a matching prior existed
    /// at capture time). `reuse_wins / times_reused` is the clean-pass rate — see
    /// [`OutcomeStats::clean_pass_rate`]. Undercounts by construction (a recalled run
    /// that ends NON-clean never reaches capture), so it is a directional signal, not
    /// a precise probability.
    #[serde(default)]
    pub reuse_wins: u32,
}

impl OutcomeStats {
    /// The reuse clean-pass rate, `reuse_wins / times_reused`, or `None` when the
    /// recipe has never been reused (no denominator). In `0.0..=1.0`.
    #[must_use]
    pub fn clean_pass_rate(&self) -> Option<f32> {
        if self.times_reused == 0 {
            None
        } else {
            Some((self.reuse_wins as f32 / self.times_reused as f32).min(1.0))
        }
    }
}

/// One durable success recipe — a proven plan shape for a task fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipe {
    /// The task fingerprint this recipe is keyed under.
    pub fingerprint: Fingerprint,
    /// The ordered step titles/seats that WORKED — `seat · title`, in execution
    /// order (bounded to [`MAX_SKELETON_STEPS`]).
    pub plan_skeleton: Vec<String>,
    /// Notable files/dirs the clean build created (bounded to [`MAX_SCAFFOLD`]).
    #[serde(default)]
    pub key_scaffold: Vec<String>,
    /// Short pattern notes (e.g. "used repository pattern", "vitest + msw for API
    /// tests"), bounded to [`MAX_PATTERNS`].
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Reuse + clean-pass statistics.
    #[serde(default)]
    pub stats: OutcomeStats,
}

// ── Fingerprint construction ────────────────────────────────────────────────────

/// A small English stopword set stripped from feature-shape tokens so the shape
/// keeps only distinguishing words. Deliberately tiny — generic scaffolding verbs
/// and product nouns that add no signal.
const SHAPE_STOPWORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "using",
    "use",
    "add",
    "make",
    "build",
    "create",
    "app",
    "application",
    "system",
    "page",
    "please",
    "that",
    "this",
    "into",
    "from",
    "new",
    "a",
    "an",
    "of",
    "to",
    "in",
    "on",
    "it",
    "is",
];

/// Distil a rough, order-insensitive feature-shape token set from a requirement:
/// split on non-alphanumeric boundaries (keeping CJK runs whole), lowercase ASCII,
/// drop pure numbers + stopwords + very short tokens, then de-duplicate, SORT, and
/// bound. Deterministic and pure.
#[must_use]
pub fn shape_tokens(requirement: &str) -> Vec<String> {
    let mut toks: Vec<String> = requirement
        .split(|c: char| !(c.is_alphanumeric()))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| excerpt(&t.to_lowercase(), MAX_TOKEN_CHARS))
        .filter(|t| t.chars().count() >= 2)
        .filter(|t| !t.chars().all(|c| c.is_ascii_digit()))
        .filter(|t| !SHAPE_STOPWORDS.contains(&t.as_str()))
        .collect();
    toks.sort();
    toks.dedup();
    toks.truncate(MAX_SHAPE_TOKENS);
    toks
}

/// Build the [`Fingerprint`] for a run: the detected stack, the routed kind, and the
/// requirement's rough shape. Fail-open (stack detection degrades to `none`).
#[must_use]
pub fn fingerprint_for(root: &Path, route: &RoutePlan, requirement: &str) -> Fingerprint {
    Fingerprint {
        stack: crate::verify::detect_project(root).as_str().to_string(),
        kind: route.kind.id().to_string(),
        shape: shape_tokens(requirement),
    }
}

// ── Similarity / matching ───────────────────────────────────────────────────────

/// Jaccard overlap of two sorted-unique token sets, `|A∩B| / |A∪B|` in `0.0..=1.0`.
/// Two empty sets overlap `0.0` (no shared signal, not a spurious match).
fn jaccard(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.iter().filter(|t| b.contains(t)).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
    }
}

/// Fingerprint similarity in `0.0..=1.0`. Stack is the strongest signal (a recipe for
/// the wrong stack is misleading), kind next, feature-shape overlap a light tiebreak.
/// An EXACT stack+kind match scores `0.8 + shape`; a same-stack-only match `0.45`; a
/// same-kind-only match `0.35`.
#[must_use]
pub fn similarity(a: &Fingerprint, b: &Fingerprint) -> f32 {
    let stack = if a.stack == b.stack { 0.45 } else { 0.0 };
    let kind = if a.kind == b.kind { 0.35 } else { 0.0 };
    let shape = jaccard(&a.shape, &b.shape) * 0.20;
    stack + kind + shape
}

/// Index of the best-matching recipe for `fp` in `store` whose similarity clears
/// [`MIN_RECALL_SIMILARITY`], or `None`. Ties are broken by MORE clean builds
/// (higher evidence), then by earlier index (stable) — so the match is deterministic.
/// The SINGLE match function shared by recall (what to inject) and capture (which
/// recipe a clean build is a reuse-win for), so the two seams stay consistent.
fn best_match(store: &[Recipe], fp: &Fingerprint) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, r) in store.iter().enumerate() {
        let s = similarity(&r.fingerprint, fp);
        if s < MIN_RECALL_SIMILARITY {
            continue;
        }
        let better = match best {
            None => true,
            Some((bi, bs)) => {
                s > bs
                    || (score_eq(s, bs)
                        && store[i].stats.clean_builds > store[bi].stats.clean_builds)
            }
        };
        if better {
            best = Some((i, s));
        }
    }
    best.map(|(i, _)| i)
}

/// Float equality within a tiny epsilon (score ties). Free function to keep
/// [`best_match`] readable.
fn score_eq(a: f32, b: f32) -> bool {
    (a - b).abs() < f32::EPSILON
}

// ── Store I/O ────────────────────────────────────────────────────────────────────

/// Resolve the recipe store directory: the [`RECIPES_DIR_ENV`] override when set,
/// else `~/.umadev/recipes`. Creates the directory best-effort so a fresh machine can
/// accumulate recipes. Returns `None` only when no home dir is resolvable AND no
/// override is set (fail-open — the caller then does nothing).
#[must_use]
pub fn recipes_dir() -> Option<PathBuf> {
    let dir = if let Some(over) = std::env::var(RECIPES_DIR_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        PathBuf::from(over)
    } else {
        home_dir()?.join(RECIPES_DIRNAME)
    };
    if !dir.is_dir() {
        let _ = std::fs::create_dir_all(&dir);
    }
    Some(dir)
}

/// Cross-platform home directory: `HOME` then `USERPROFILE` (Windows).
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Absolute path of the store file inside a recipes dir.
fn store_path(dir: &Path) -> PathBuf {
    dir.join(RECIPES_FILE)
}

/// Load all recipes from the store in `dir`, oldest FIRST.
///
/// Fail-open + forgiving: a missing/unreadable file yields an empty vec; a
/// corrupt/garbage line is skipped (a single bad line never loses the rest). Each
/// loaded recipe is normalised (fields trimmed + bounded) and dropped if it has no
/// usable skeleton. Bounded at [`MAX_RECIPES`] (lowest-evidence dropped).
#[must_use]
pub fn load_recipes(dir: &Path) -> Vec<Recipe> {
    let Ok(text) = std::fs::read_to_string(store_path(dir)) else {
        return Vec::new();
    };
    let parsed: Vec<Recipe> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Recipe>(l).ok())
        .map(normalize)
        .filter(|r| !r.plan_skeleton.is_empty())
        .collect();
    cap_store(parsed)
}

/// Trim + field-truncate + bound a recipe to the per-recipe caps.
fn normalize(mut r: Recipe) -> Recipe {
    r.fingerprint.stack = excerpt(r.fingerprint.stack.trim(), MAX_TOKEN_CHARS);
    r.fingerprint.kind = excerpt(r.fingerprint.kind.trim(), MAX_TOKEN_CHARS);
    r.fingerprint.shape = bound_lines(r.fingerprint.shape, MAX_SHAPE_TOKENS, MAX_TOKEN_CHARS);
    // Re-normalise the shape set so a hand-edited / older line still compares by value.
    r.fingerprint.shape.sort();
    r.fingerprint.shape.dedup();
    r.plan_skeleton = bound_lines(r.plan_skeleton, MAX_SKELETON_STEPS, MAX_LINE_CHARS);
    r.key_scaffold = bound_lines(r.key_scaffold, MAX_SCAFFOLD, MAX_LINE_CHARS);
    r.patterns = bound_lines(r.patterns, MAX_PATTERNS, MAX_LINE_CHARS);
    r
}

/// Trim, drop-empty, truncate each entry, de-duplicate (order-preserving), and cap
/// the length of a string list.
fn bound_lines(items: Vec<String>, max_items: usize, max_chars: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for it in items {
        let t = excerpt(it.trim(), max_chars);
        if t.is_empty() || out.contains(&t) {
            continue;
        }
        out.push(t);
        if out.len() >= max_items {
            break;
        }
    }
    out
}

/// Cap the store at [`MAX_RECIPES`], evicting the LOWEST-evidence recipes first
/// (fewest clean builds; ties broken by earliest position). Deterministic.
fn cap_store(mut store: Vec<Recipe>) -> Vec<Recipe> {
    while store.len() > MAX_RECIPES {
        // Index of the lowest-evidence recipe (min clean_builds, earliest on tie).
        let mut victim = 0usize;
        for i in 1..store.len() {
            if store[i].stats.clean_builds < store[victim].stats.clean_builds {
                victim = i;
            }
        }
        store.remove(victim);
    }
    store
}

/// Serialise the store to JSONL (one recipe per line). A recipe that fails to
/// serialise is skipped (fail-open).
fn render_jsonl(store: &[Recipe]) -> String {
    let mut buf = String::new();
    for r in store {
        if let Ok(line) = serde_json::to_string(r) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    buf
}

/// Atomically write `body` to `path` via a unique temp file + rename, so a reader (or
/// a concurrent writer) never observes a torn file. Best-effort cleanup of the temp on
/// rename failure. Mirrors [`crate::project_facts`]' atomic writer.
fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("recipes"),
        std::process::id(),
        stamp,
    ));
    std::fs::write(&tmp, body)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// A process-wide lock serialising the read-modify-write on the SHARED global store so
/// two concurrent runs (or a run + a forked consult) can't clobber each other. Recovers
/// from poison so a panic elsewhere never wedges this fail-open path.
fn store_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Persist a store to `dir` atomically, capping it first. Fail-open (a failed write is
/// swallowed). Caller holds [`store_lock`].
fn persist(dir: &Path, store: Vec<Recipe>) {
    let store = cap_store(store);
    let _ = std::fs::create_dir_all(dir);
    let _ = write_atomic(&store_path(dir), &render_jsonl(&store));
}

// ── Capture (write path) ─────────────────────────────────────────────────────────

/// Capture `recipe` into the store at `dir` — the core, dir-explicit write path.
///
/// Under the shared [`store_lock`] (one atomic read-modify-write):
/// 1. **Reuse-win credit.** If a PRE-EXISTING recipe matches this run's fingerprint
///    ([`best_match`] — the same match recall would have surfaced at plan time), this
///    clean delivery counts as a reuse-win for it (`reuse_wins += 1`), so the
///    clean-pass rate reflects reality.
/// 2. **Merge or insert.** If a recipe with the EXACT same fingerprint already exists,
///    MERGE (union skeleton/scaffold/patterns, bounded; `clean_builds += 1`) so a
///    repeat of the same fingerprint updates stats without duplicating. Otherwise
///    INSERT the new recipe (`clean_builds = 1`).
///
/// Returns `true` when a recipe was written/updated. Fail-open: an empty skeleton is a
/// no-op (`false`); a write error is swallowed (still `true` for the in-memory apply,
/// but never an error) — capturing a recipe must NEVER block a delivery.
pub fn capture_recipe(dir: &Path, recipe: Recipe) -> bool {
    let recipe = normalize(recipe);
    if recipe.plan_skeleton.is_empty() {
        return false; // nothing proven → nothing to store
    }
    let _guard = store_lock();
    let mut store = load_recipes(dir);

    // 1. Credit the pre-existing best match (if any) with a reuse-win for this clean
    //    delivery — computed BEFORE the merge so a same-fingerprint match is the
    //    prior, not the row we're about to bump.
    if let Some(i) = best_match(&store, &recipe.fingerprint) {
        store[i].stats.reuse_wins = store[i].stats.reuse_wins.saturating_add(1);
    }

    // 2. Merge into an EXACT-fingerprint row, or insert a new one.
    if let Some(existing) = store
        .iter_mut()
        .find(|r| r.fingerprint == recipe.fingerprint)
    {
        existing.plan_skeleton = bound_lines(
            [existing.plan_skeleton.clone(), recipe.plan_skeleton].concat(),
            MAX_SKELETON_STEPS,
            MAX_LINE_CHARS,
        );
        existing.key_scaffold = bound_lines(
            [existing.key_scaffold.clone(), recipe.key_scaffold].concat(),
            MAX_SCAFFOLD,
            MAX_LINE_CHARS,
        );
        existing.patterns = bound_lines(
            [existing.patterns.clone(), recipe.patterns].concat(),
            MAX_PATTERNS,
            MAX_LINE_CHARS,
        );
        existing.stats.clean_builds = existing.stats.clean_builds.saturating_add(1);
    } else {
        let mut fresh = recipe;
        fresh.stats.clean_builds = fresh.stats.clean_builds.max(1);
        store.push(fresh);
    }

    persist(dir, store);
    true
}

// ── Recall (read path) ───────────────────────────────────────────────────────────

/// Look up the closest recipe for `fp` in the store at `dir`, bumping its
/// `times_reused` (it is being surfaced as a prior). Returns a clone of the matched
/// recipe, or `None` when nothing clears [`MIN_RECALL_SIMILARITY`] (a no-op —
/// unchanged behaviour). Fail-open at every step.
///
/// The stat bump is a best-effort atomic read-modify-write under [`store_lock`]; a
/// write failure still returns the matched recipe (so recall works even if the store
/// is read-only). The returned recipe is the PRE-bump snapshot (its content, which is
/// what the prior block renders — the counter is irrelevant to the prompt).
#[must_use]
pub fn recall_best(dir: &Path, fp: &Fingerprint) -> Option<Recipe> {
    let _guard = store_lock();
    let mut store = load_recipes(dir);
    let i = best_match(&store, fp)?;
    let matched = store[i].clone();
    store[i].stats.times_reused = store[i].stats.times_reused.saturating_add(1);
    persist(dir, store);
    Some(matched)
}

/// The plan-time RECALL prior block for `fp`, ready to splice into the plan-synthesis
/// prompt — or `None` when no recipe matches (recall is then a no-op and the plan is
/// synthesised exactly as before). Bumps the matched recipe's `times_reused`.
///
/// The block frames the recipe as an ADAPTABLE PRIOR, never a template, and is bounded
/// by `budget_chars` (typically [`RECIPE_PRIOR_BUDGET`]).
#[must_use]
pub fn recall_prior_block(dir: &Path, fp: &Fingerprint, budget_chars: usize) -> Option<String> {
    let recipe = recall_best(dir, fp)?;
    Some(recipe_prior_block(&recipe, budget_chars))
}

/// Render a recipe as an adaptable plan-time prior. Pure + deterministic; bounded by
/// `budget_chars`.
#[must_use]
pub fn recipe_prior_block(recipe: &Recipe, budget_chars: usize) -> String {
    let fp = &recipe.fingerprint;
    let rate = recipe
        .stats
        .clean_pass_rate()
        .map(|r| format!(", clean-pass rate {:.0}%", r * 100.0))
        .unwrap_or_default();
    let mut block = format!(
        "## PRIOR — a past CLEAN build of a similar stack/feature (ADAPT if it fits — this is NOT a template)\n\n\
         A previous clean delivery on a [{stack} · {kind}] project \
         ({builds} clean build(s){rate}) used this plan shape. Treat it as a hint from a \
         senior teammate: reuse the parts that fit THIS requirement, drop the parts that \
         don't, and never copy it blindly.\n\nPlan shape that worked:\n",
        stack = fp.stack,
        kind = fp.kind,
        builds = recipe.stats.clean_builds,
    );
    for (n, step) in recipe.plan_skeleton.iter().enumerate() {
        block.push_str(&format!("{}. {step}\n", n + 1));
    }
    if !recipe.key_scaffold.is_empty() {
        block.push_str(&format!(
            "Notable scaffold: {}\n",
            recipe.key_scaffold.join(", ")
        ));
    }
    if !recipe.patterns.is_empty() {
        block.push_str(&format!(
            "Patterns that worked: {}\n",
            recipe.patterns.join("; ")
        ));
    }
    excerpt(&block, budget_chars)
}

// ── Capture orchestration at the delivery seam ───────────────────────────────────

/// Distil + capture a success recipe at a CLEAN deliberate delivery — called at the
/// SAME finalize seam as [`crate::self_evolve::reconcile_at_delivery`], AFTER the run
/// has already settled clean. A pure SIDE EFFECT: it never changes the delivery, the
/// gate, the plan status, or any verdict.
///
/// The mechanical recipe (plan skeleton + evidence-derived scaffold + fingerprint) is
/// built deterministically from the executed plan. ONE read-only fork consult MAY then
/// enrich it with `patterns` + extra scaffold; if the consult fails (offline / no fork
/// / timeout / unparseable) the mechanical recipe is stored as-is. Every path is
/// fail-open: no home dir, an empty skeleton, or a write error records nothing and
/// never affects the just-finished delivery.
pub(crate) async fn capture_at_delivery(
    session: &mut dyn BaseSession,
    root: &Path,
    route: &RoutePlan,
    plan: &Plan,
    requirement: &str,
    events: &Arc<dyn EventSink>,
) {
    let Some(dir) = recipes_dir() else {
        return; // no home / override → fail-open no-op
    };
    capture_at_delivery_in(session, &dir, root, route, plan, requirement, events).await;
}

/// Dir-explicit core of [`capture_at_delivery`] — the whole capture logic against a
/// caller-supplied recipes dir, so it is exercised in tests with a temp dir (no
/// process-env mutation, no race). The public wrapper just resolves [`recipes_dir`].
async fn capture_at_delivery_in(
    session: &mut dyn BaseSession,
    dir: &Path,
    root: &Path,
    route: &RoutePlan,
    plan: &Plan,
    requirement: &str,
    events: &Arc<dyn EventSink>,
) {
    // Mechanical skeleton from the steps that actually reached Done — the proven order.
    let skeleton = skeleton_from_plan(plan);
    if skeleton.is_empty() {
        return; // no proven build steps → nothing worth remembering
    }
    let fingerprint = fingerprint_for(root, route, requirement);
    let mut scaffold = scaffold_from_plan(plan);
    let mut patterns: Vec<String> = Vec::new();

    // OPTIONAL enrichment: ONE read-only fork consult for short pattern notes + any
    // extra notable scaffold the mechanical scan missed. Fail-open — a `None` reply
    // leaves the mechanical recipe intact (patterns empty, scaffold as scanned).
    if let Some(distilled) = distill_enrichment(session, &skeleton).await {
        patterns = distilled.patterns;
        scaffold = bound_lines(
            [scaffold, distilled.key_scaffold].concat(),
            MAX_SCAFFOLD,
            MAX_LINE_CHARS,
        );
    }

    let recipe = Recipe {
        fingerprint,
        plan_skeleton: skeleton,
        key_scaffold: scaffold,
        patterns,
        stats: OutcomeStats::default(),
    };
    if capture_recipe(dir, recipe) {
        events.emit(EngineEvent::Note(
            "[learned] 交付通过:已把这次干净构建的成功计划形态(可复用配方)记入跨项目记忆库,下次相似任务会作为先验提示。"
                .to_string(),
        ));
    }
}

/// The proven plan skeleton — every step that reached [`StepStatus::Done`], in plan
/// order, as `seat · title`. A build with no Done step yields an empty skeleton (the
/// caller then skips capture).
fn skeleton_from_plan(plan: &Plan) -> Vec<String> {
    let lines: Vec<String> = plan
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Done)
        .map(|s| format!("{} · {}", s.seat.role_id(), s.title))
        .collect();
    // A recipe is only worth storing if a real DOER step (not just a review) worked.
    let has_build = plan
        .steps
        .iter()
        .any(|s| s.kind == StepKind::Build && s.status == StepStatus::Done);
    if has_build {
        bound_lines(lines, MAX_SKELETON_STEPS, MAX_LINE_CHARS)
    } else {
        Vec::new()
    }
}

/// The concrete scaffold the clean build produced, scanned MECHANICALLY from the
/// evidence contracts of the Done build steps (`file-exists` / `file-contains` paths).
/// Deterministic; no consult.
fn scaffold_from_plan(plan: &Plan) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for step in plan
        .steps
        .iter()
        .filter(|s| s.kind == StepKind::Build && s.status == StepStatus::Done)
    {
        for ev in &step.evidence {
            match ev {
                EvidenceContract::FileExists { path }
                | EvidenceContract::FileContains { path, .. } => paths.push(path.clone()),
                _ => {}
            }
        }
    }
    bound_lines(paths, MAX_SCAFFOLD, MAX_LINE_CHARS)
}

/// What the optional enrichment consult returns.
struct Distilled {
    patterns: Vec<String>,
    key_scaffold: Vec<String>,
}

/// Ask the borrowed brain — on ONE read-only fork — to name the reusable PATTERNS the
/// just-finished clean build leaned on plus any extra notable scaffold, as a small
/// JSON object. Fail-open: a missing fork, an offline brain, a timeout, or an
/// unparseable reply yields `None` (the caller keeps the mechanical recipe).
async fn distill_enrichment(
    session: &mut dyn BaseSession,
    skeleton: &[String],
) -> Option<Distilled> {
    let system = "You are wrapping up a CLEAN software build and recording a reusable \
         'success recipe' for next time. Given the plan steps that worked, name the \
         reusable PATTERNS this kind of build should lean on (architecture/testing/data \
         choices — e.g. \"repository pattern for data access\", \"vitest + msw for API \
         tests\") and any NOTABLE scaffold files/dirs a teammate should expect. Keep each \
         entry SHORT and reusable — no project-specific narration, no todo items. \
         JSON shape: {\"patterns\":[\"…\"],\"key_scaffold\":[\"src/…\"]}. Empty arrays are \
         fine if nothing is genuinely reusable.";
    let user = format!("Plan steps that worked:\n{}", skeleton.join("\n"));

    let fork = crate::continuous::fork_with_timeout(session).await;
    let consult = crate::continuous::ForkConsult::new(fork);
    let reply = consult.judge_json("recipe-distill", system, user).await;
    consult.end().await;

    let json = reply?;
    let raw: DistillReply = serde_json::from_str(&json).ok()?;
    Some(Distilled {
        patterns: bound_lines(raw.patterns, MAX_PATTERNS, MAX_LINE_CHARS),
        key_scaffold: bound_lines(raw.key_scaffold, MAX_SCAFFOLD, MAX_LINE_CHARS),
    })
}

/// The strict-JSON shape the enrichment consult returns (both fields optional).
#[derive(Debug, Default, Deserialize)]
struct DistillReply {
    #[serde(default)]
    patterns: Vec<String>,
    #[serde(default)]
    key_scaffold: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::critics::Seat;
    use crate::events::NullSink;
    use crate::plan_state::{AcceptanceSpec, PlanStep};
    use crate::planner::TaskKind;
    use crate::router::{Budget, Depth, RouteClass};
    use std::collections::VecDeque;
    use umadev_runtime::{ApprovalDecision, SessionError, SessionEvent, TurnStatus};

    // ── fixtures ────────────────────────────────────────────────────────────────

    fn sink() -> Arc<dyn EventSink> {
        Arc::new(NullSink)
    }

    fn route(kind: TaskKind) -> RoutePlan {
        RoutePlan {
            class: RouteClass::Build,
            kind,
            depth: Depth::Standard,
            team: vec![Seat::FrontendEngineer],
            scope: Vec::new(),
            needs_clarify: None,
            est_budget: Budget::for_route(RouteClass::Build, Depth::Standard),
            confidence: 0.7,
        }
    }

    fn fp(stack: &str, kind: &str, shape: &[&str]) -> Fingerprint {
        let mut shape: Vec<String> = shape.iter().map(|s| (*s).to_string()).collect();
        shape.sort();
        shape.dedup();
        Fingerprint {
            stack: stack.to_string(),
            kind: kind.to_string(),
            shape,
        }
    }

    fn recipe(stack: &str, kind: &str, shape: &[&str], skeleton: &[&str]) -> Recipe {
        Recipe {
            fingerprint: fp(stack, kind, shape),
            plan_skeleton: skeleton.iter().map(|s| (*s).to_string()).collect(),
            key_scaffold: vec!["src/App.tsx".to_string()],
            patterns: vec!["used repository pattern".to_string()],
            stats: OutcomeStats::default(),
        }
    }

    fn done_build_step(id: &str, seat: Seat, title: &str, files: &[&str]) -> PlanStep {
        PlanStep {
            id: id.to_string(),
            title: title.to_string(),
            seat,
            kind: StepKind::Build,
            depends_on: Vec::new(),
            acceptance: AcceptanceSpec::SourcePresent,
            evidence: files
                .iter()
                .map(|p| EvidenceContract::FileExists {
                    path: (*p).to_string(),
                })
                .collect(),
            status: StepStatus::Done,
        }
    }

    // A scripted fake base whose read-only fork answers with a fixed reply. Used to
    // drive the distillation consult; `can_fork=false` exercises the fail-open path.
    struct Fork {
        reply: String,
        pending: VecDeque<SessionEvent>,
    }
    #[async_trait::async_trait]
    impl BaseSession for Fork {
        async fn send_turn(&mut self, _d: String) -> Result<(), SessionError> {
            self.pending = [
                SessionEvent::TextDelta(self.reply.clone()),
                SessionEvent::TurnDone {
                    status: TurnStatus::Completed,
                    usage: None,
                },
            ]
            .into_iter()
            .collect();
            Ok(())
        }
        async fn next_event(&mut self) -> Option<SessionEvent> {
            self.pending.pop_front()
        }
        async fn respond(&mut self, _r: &str, _d: ApprovalDecision) -> Result<(), SessionError> {
            Ok(())
        }
        async fn interrupt(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
        async fn end(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
    }
    struct Brain {
        reply: String,
        can_fork: bool,
    }
    #[async_trait::async_trait]
    impl BaseSession for Brain {
        async fn fork(&mut self) -> Result<Box<dyn BaseSession>, SessionError> {
            if !self.can_fork {
                return Err(SessionError::ForkUnsupported("test".into()));
            }
            Ok(Box::new(Fork {
                reply: self.reply.clone(),
                pending: VecDeque::new(),
            }))
        }
        async fn send_turn(&mut self, _d: String) -> Result<(), SessionError> {
            Ok(())
        }
        async fn next_event(&mut self) -> Option<SessionEvent> {
            None
        }
        async fn respond(&mut self, _r: &str, _d: ApprovalDecision) -> Result<(), SessionError> {
            Ok(())
        }
        async fn interrupt(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
        async fn end(&mut self) -> Result<(), SessionError> {
            Ok(())
        }
    }

    fn clean_plan() -> Plan {
        Plan {
            steps: vec![
                done_build_step(
                    "scaffold",
                    Seat::FrontendEngineer,
                    "Scaffold the app shell",
                    &["src/App.tsx"],
                ),
                done_build_step(
                    "api",
                    Seat::BackendEngineer,
                    "Wire the login API",
                    &["src/api.ts"],
                ),
                PlanStep {
                    status: StepStatus::Done,
                    kind: StepKind::Review,
                    ..done_build_step("qa", Seat::QaEngineer, "Review the build", &[])
                },
            ],
            risks: Vec::new(),
            open_questions: Vec::new(),
        }
    }

    // ── shape / fingerprint ───────────────────────────────────────────────────────

    #[test]
    fn shape_tokens_are_normalised_sorted_and_stopword_filtered() {
        let toks = shape_tokens("Build a Todo LIST app with login");
        // "build"/"a"/"app"/"with" are stopwords; the rest lowercased, sorted, unique.
        assert_eq!(toks, vec!["list", "login", "todo"]);
    }

    #[test]
    fn similarity_ranks_stack_then_kind_then_shape() {
        let a = fp("node", "greenfield", &["todo", "login"]);
        let exact = similarity(&a, &fp("node", "greenfield", &["todo", "login"]));
        let stack_only = similarity(&a, &fp("node", "bugfix", &["x"]));
        let kind_only = similarity(&a, &fp("rust", "greenfield", &["x"]));
        let none = similarity(&a, &fp("rust", "bugfix", &["x"]));
        assert!(
            exact > 0.9,
            "exact stack+kind+shape scores highest: {exact}"
        );
        assert!(stack_only >= MIN_RECALL_SIMILARITY, "same stack surfaces");
        assert!((kind_only - 0.35).abs() < 1e-6, "kind-only ~0.35");
        assert!(none < MIN_RECALL_SIMILARITY, "unrelated is below the floor");
    }

    // ── capture: a clean delivery writes a recipe (fingerprint + skeleton) ──────────

    #[test]
    fn capture_writes_a_recipe_with_fingerprint_and_skeleton() {
        let tmp = tempfile::TempDir::new().unwrap();
        let r = recipe(
            "node",
            "greenfield",
            &["todo"],
            &["frontend-engineer · scaffold"],
        );
        assert!(capture_recipe(tmp.path(), r));
        let store = load_recipes(tmp.path());
        assert_eq!(store.len(), 1);
        assert_eq!(store[0].fingerprint.stack, "node");
        assert_eq!(store[0].plan_skeleton, vec!["frontend-engineer · scaffold"]);
        assert_eq!(
            store[0].stats.clean_builds, 1,
            "first capture = 1 clean build"
        );
        assert!(tmp.path().join(RECIPES_FILE).exists());
    }

    #[test]
    fn empty_skeleton_captures_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut r = recipe("node", "greenfield", &["x"], &[]);
        r.plan_skeleton.clear();
        assert!(!capture_recipe(tmp.path(), r));
        assert!(load_recipes(tmp.path()).is_empty());
    }

    // ── second run of a similar fingerprint recalls it + merges stats (no dup) ──────

    #[test]
    fn recall_then_recapture_merges_stats_without_duplicating() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Run 1: a clean build captures the first recipe.
        assert!(capture_recipe(
            tmp.path(),
            recipe(
                "node",
                "frontend_only",
                &["todo", "list"],
                &["frontend-engineer · scaffold"]
            ),
        ));

        // Run 2 (plan time): a SIMILAR fingerprint recalls the recipe as a prior +
        // bumps times_reused.
        let query = fp("node", "frontend_only", &["todo", "tracker"]);
        let block = recall_prior_block(tmp.path(), &query, RECIPE_PRIOR_BUDGET);
        assert!(block.is_some(), "a similar fingerprint recalls the prior");
        let block = block.unwrap();
        assert!(
            block.contains("PRIOR"),
            "the block frames it as a prior: {block}"
        );
        assert!(
            block.contains("frontend-engineer · scaffold"),
            "the block carries the proven skeleton: {block}"
        );
        assert!(
            block.to_lowercase().contains("adapt"),
            "the block invites adaptation, not a template: {block}"
        );
        assert_eq!(
            load_recipes(tmp.path())[0].stats.times_reused,
            1,
            "recall bumps times_reused"
        );

        // Run 2 (delivery): the clean build re-captures the SAME fingerprint → merge,
        // NOT duplicate; clean_builds grows, and the earlier recall is credited a win.
        assert!(capture_recipe(
            tmp.path(),
            recipe(
                "node",
                "frontend_only",
                &["list", "todo"],
                &["frontend-engineer · scaffold", "qa-engineer · tests"]
            ),
        ));
        let store = load_recipes(tmp.path());
        assert_eq!(
            store.len(),
            1,
            "same fingerprint merged, not duplicated: {store:?}"
        );
        assert_eq!(store[0].stats.clean_builds, 2, "two clean builds folded in");
        assert_eq!(store[0].stats.times_reused, 1, "reuse count preserved");
        assert_eq!(
            store[0].stats.reuse_wins, 1,
            "the recalled prior is credited a clean-pass win"
        );
        assert_eq!(store[0].stats.clean_pass_rate(), Some(1.0));
        // The merged skeleton unions both runs' steps (bounded, de-duplicated).
        assert!(store[0]
            .plan_skeleton
            .contains(&"qa-engineer · tests".to_string()));
        assert_eq!(
            store[0]
                .plan_skeleton
                .iter()
                .filter(|s| *s == "frontend-engineer · scaffold")
                .count(),
            1,
            "shared step not duplicated on merge"
        );
    }

    // ── a non-matching fingerprint recalls nothing (no-op) ──────────────────────────

    #[test]
    fn a_non_matching_fingerprint_recalls_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        capture_recipe(
            tmp.path(),
            recipe(
                "node",
                "greenfield",
                &["todo"],
                &["frontend-engineer · scaffold"],
            ),
        );
        // A totally unrelated stack + kind + shape → below the floor → no prior.
        let query = fp("rust", "bugfix", &["kernel", "driver"]);
        assert!(recall_prior_block(tmp.path(), &query, RECIPE_PRIOR_BUDGET).is_none());
        // A no-op recall never bumps a stat.
        assert_eq!(load_recipes(tmp.path())[0].stats.times_reused, 0);
    }

    #[test]
    fn recall_is_fail_open_on_an_empty_store() {
        let tmp = tempfile::TempDir::new().unwrap();
        let query = fp("node", "greenfield", &["todo"]);
        assert!(recall_prior_block(tmp.path(), &query, RECIPE_PRIOR_BUDGET).is_none());
        assert!(load_recipes(tmp.path()).is_empty());
    }

    #[test]
    fn load_is_forgiving_of_corrupt_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let good = serde_json::to_string(&recipe(
            "node",
            "greenfield",
            &["todo"],
            &["frontend-engineer · scaffold"],
        ))
        .unwrap();
        std::fs::write(
            tmp.path().join(RECIPES_FILE),
            format!("{good}\nthis is not json {{{{\n"),
        )
        .unwrap();
        let store = load_recipes(tmp.path());
        assert_eq!(store.len(), 1, "good line kept, garbage skipped: {store:?}");
    }

    // ── capture is fail-open (a store error never fails delivery) ────────────────────

    #[tokio::test]
    async fn capture_at_delivery_is_fail_open_on_an_unwritable_store() {
        // Point the store at a path whose parent is a regular file — the dir can never
        // be created, so every write silently fails. capture_at_delivery must not panic
        // and must record nothing, never affecting the (already-clean) delivery.
        let tmp = tempfile::TempDir::new().unwrap();
        let blocker = tmp.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap();
        let dead_dir = blocker.join("recipes");

        let mut brain = Brain {
            reply: String::new(),
            can_fork: false,
        };
        // Drive capture with an explicit dead dir via the core write path — a write
        // failure is swallowed and load stays empty.
        let skeleton = skeleton_from_plan(&clean_plan());
        assert!(!skeleton.is_empty());
        let r = Recipe {
            fingerprint: fp("node", "greenfield", &["x"]),
            plan_skeleton: skeleton,
            key_scaffold: Vec::new(),
            patterns: Vec::new(),
            stats: OutcomeStats::default(),
        };
        // No panic; the unwritable store simply holds nothing.
        let _ = capture_recipe(&dead_dir, r);
        assert!(load_recipes(&dead_dir).is_empty());
        // And the async orchestrator's distill path is itself fail-open on no fork.
        let d = distill_enrichment(&mut brain, &["frontend-engineer · scaffold".to_string()]).await;
        assert!(
            d.is_none(),
            "no fork → no enrichment, mechanical recipe stands"
        );
    }

    // ── the distillation consult is fail-open (brain down → mechanical recipe) ───────

    #[tokio::test]
    async fn capture_at_delivery_stores_a_mechanical_recipe_when_the_brain_is_down() {
        // Two dirs: the recipes store dir (dir-explicit core, no env) and the project
        // root (only used for stack detection — an empty dir detects `none`).
        let store_dir = tempfile::TempDir::new().unwrap();
        let proj = tempfile::TempDir::new().unwrap();
        let mut brain = Brain {
            reply: String::new(),
            can_fork: false, // offline / no fork → distillation returns None
        };
        capture_at_delivery_in(
            &mut brain,
            store_dir.path(),
            proj.path(),
            &route(TaskKind::Greenfield),
            &clean_plan(),
            "build a todo list",
            &sink(),
        )
        .await;

        let store = load_recipes(store_dir.path());
        assert_eq!(
            store.len(),
            1,
            "a mechanical recipe is still stored: {store:?}"
        );
        // The skeleton came from the Done steps; the scaffold from their evidence paths.
        assert!(store[0]
            .plan_skeleton
            .iter()
            .any(|s| s.contains("Scaffold the app shell")));
        assert!(store[0].key_scaffold.contains(&"src/App.tsx".to_string()));
        assert!(store[0].key_scaffold.contains(&"src/api.ts".to_string()));
        // Brain was down → no distilled patterns, but the recipe is otherwise complete.
        assert!(store[0].patterns.is_empty(), "no consult → no patterns");
    }

    #[tokio::test]
    async fn distillation_consult_enriches_patterns_when_the_brain_answers() {
        let mut brain = Brain {
            reply: "{\"patterns\":[\"repository pattern for data access\"],\
                    \"key_scaffold\":[\"src/db/repo.ts\"]}"
                .to_string(),
            can_fork: true,
        };
        let d = distill_enrichment(&mut brain, &["backend-engineer · wire the API".to_string()])
            .await
            .expect("a forking brain enriches");
        assert_eq!(d.patterns, vec!["repository pattern for data access"]);
        assert_eq!(d.key_scaffold, vec!["src/db/repo.ts"]);
    }

    // ── bounded under many recipes ──────────────────────────────────────────────────

    #[test]
    fn store_is_bounded_under_many_recipes() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Capture well over the cap, each a DISTINCT fingerprint so none merge.
        for i in 0..(MAX_RECIPES + 40) {
            let r = Recipe {
                fingerprint: Fingerprint {
                    stack: "node".to_string(),
                    kind: "greenfield".to_string(),
                    shape: vec![format!("feat{i}")],
                },
                plan_skeleton: vec!["frontend-engineer · scaffold".to_string()],
                key_scaffold: Vec::new(),
                patterns: Vec::new(),
                stats: OutcomeStats::default(),
            };
            capture_recipe(tmp.path(), r);
        }
        let store = load_recipes(tmp.path());
        assert!(
            store.len() <= MAX_RECIPES,
            "store capped at MAX_RECIPES ({} > {MAX_RECIPES})",
            store.len()
        );
    }

    #[test]
    fn per_recipe_fields_are_bounded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let big_skeleton: Vec<String> = (0..100)
            .map(|i| format!("frontend-engineer · step {i}"))
            .collect();
        let r = Recipe {
            fingerprint: fp("node", "greenfield", &["x"]),
            plan_skeleton: big_skeleton,
            key_scaffold: (0..100).map(|i| format!("src/f{i}.ts")).collect(),
            patterns: (0..100).map(|i| format!("pattern {i}")).collect(),
            stats: OutcomeStats::default(),
        };
        capture_recipe(tmp.path(), r);
        let s = &load_recipes(tmp.path())[0];
        assert!(s.plan_skeleton.len() <= MAX_SKELETON_STEPS);
        assert!(s.key_scaffold.len() <= MAX_SCAFFOLD);
        assert!(s.patterns.len() <= MAX_PATTERNS);
    }

    #[test]
    fn prior_block_is_budget_bounded() {
        let big: Vec<String> = (0..MAX_SKELETON_STEPS)
            .map(|i| {
                format!(
                    "frontend-engineer · a long proven step title number {i} {}",
                    "x".repeat(80)
                )
            })
            .collect();
        let r = Recipe {
            fingerprint: fp("node", "greenfield", &["todo"]),
            plan_skeleton: big,
            key_scaffold: vec!["src/App.tsx".to_string()],
            patterns: vec!["used repository pattern".to_string()],
            stats: OutcomeStats {
                clean_builds: 3,
                times_reused: 4,
                reuse_wins: 2,
            },
        };
        let block = recipe_prior_block(&r, RECIPE_PRIOR_BUDGET);
        assert!(
            block.chars().count() <= RECIPE_PRIOR_BUDGET,
            "prior block within budget ({} > {RECIPE_PRIOR_BUDGET})",
            block.chars().count()
        );
        assert!(
            block.contains("clean-pass rate 50%"),
            "renders the clean-pass rate: {block}"
        );
    }
}
