# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working
with code in this repository.

## What this project is

UmaDev is a Rust workspace that ships a **single binary** (`umadev`): a
persistent **project-director Agent** that loads a base CLI's brain as one
continuous session and leads a full delivery team — product manager,
architect, UI/UX designer, frontend, backend, QA, security, DevOps, plus
the director — to turn a requirement into a shippable product. It embeds
`UMADEV_HOST_SPEC_V1` (see `spec/`). It does **not** own a model endpoint:
the base does all the cognition (thinking, research, design, writing code,
review); UmaDev is the deterministic shell that **routes** the request,
**owns and drives a visible plan**, **schedules the team**, injects its
**firmware**, enforces gates and governance, and leaves the audit trail.

**The product is "firmware over a borrowed brain," not a fixed pipeline.**
The authoritative product spec is
[`docs/PRODUCT_VISION_AND_ROADMAP.md`](docs/PRODUCT_VISION_AND_ROADMAP.md)
— read it first; it supersedes the older migration-wave framing in
`docs/AGENT_WIELDS_BASE_ARCHITECTURE.md` and the per-phase narrative in
`docs/CONTINUOUS_SESSION_ARCHITECTURE.md`. The normative-prose mirror of the
runtime is `spec/UMADEV_HOST_SPEC_V1.md` §9.3–§9.5 (§9.5 is the
route → plan → schedule → deliver model). A turn flows through up to five
layers, every brain consult fail-open to a deterministic floor:

- **L0 firmware** (`umadev_agent::context::compose_firmware`) — a curated,
  token-budgeted system prompt: identity + craft / anti-AI-slop taste + JIT
  knowledge digest + learned-pitfall recall + a **repo-map slice**
  (`umadev_knowledge::repo_map`) of the user's existing code — injected on
  every path through each base's own native system-prompt surface.
- **L1 router** (`umadev_agent::router`) — classifies the turn into a typed
  `RoutePlan { class, kind, depth, team, scope, … }` (deterministic Tier-0
  floor + an optional forked brain consult that may escalate, never silently
  de-scope). The route is **surfaced** (`EngineEvent::IntentDecided`) so the
  user sees "small change, on it" vs "full build, entering the delivery
  flow," and can override it.
- **L2 plan + scheduling** (`umadev_agent::plan_state`) — for a deliberate
  build the director asks the brain for a strict plan it **parses and owns**
  as a dependency DAG (`.umadev/plan.json`), rendered as a live, steerable
  checklist (`/plan`, `EngineEvent::PlanPosted` / `PlanStepStatus`). Steps
  are driven via `director::summon` (single-writer doers serial; critics
  fork read-only in parallel).
- **L3–L5 drive / verify / learn** (`director_loop`) — walks the plan step
  by step, verifies each step against its acceptance on the deterministic
  floor, self-corrects blocking findings with a typed evidence-bearing
  rework directive (diagnosed, not "go fix it"), and exits cleanly when
  stuck. `director::finalize` produces the delivery artifacts + proof-pack
  once the floor is clean; the run's episodes feed self-evolving memory.

The **full commercial phase chain** (`research → … → delivery`,
`UD-FLOW-001` / spec §4.1) is the *deepest play* the director routes to and
its plan expands into for a heavyweight greenfield build — **not** a funnel
every message is forced through.

**One continuous session = the Agent's working context.** A run opens a
single base session and drives the *same* session through every step, so
the base keeps its accumulated context instead of being re-primed from
cold each phase. The persistent session is the **default** (see
`umadev_agent::continuous_enabled_from_env` and the `*_session.rs` drivers
in `umadev-host`); the per-phase single-shot path (`claude --print` /
`codex exec` / `opencode run`) is retained only as a **fail-open
fallback** — reached when the session can't start, when the brain is the
offline runtime, or on an explicit opt-out (`UMADEV_CONTINUOUS=0` /
`UMADEV_LEGACY_RUN=1`).

Chat, an ad-hoc task ("review this code" / "fix this bug"), and a full
pipeline run are **not three code paths** — they are three ways the
director (via the L1 router) steers the same session over the same memory.
Conversation is itself a first-class surface: UmaDev sends its own bounded
transcript to the base every turn and **persists + resumes** per-project
chat (`.umadev/chat/<id>.json`, `/sessions` `/resume` `/compact`), so
reopening UmaDev keeps the conversation and an unfinished plan can be
resumed. Only workspace-mutating work takes the single-writer run lock and
the full gate machinery.

The binary has two backend modes:

- **base-CLI** (the product) — `run --backend <id>` drives an
  already-logged-in base CLI; **no API key of its own**, the customer's
  existing base subscription/config IS the brain. Exactly **three**
  first-class bases: `claude-code`, `codex`, `opencode`. See
  `umadev_host::BACKEND_IDS`. UmaDev injects **NOTHING** into the base —
  whatever the base is already configured with (official login OR the
  customer's own third-party / local-model routing) is exactly what runs.
  UmaDev does not own, broker, or configure any model endpoint; connecting
  a third-party/local model is the base's job, not ours.
- **offline** (internal fallback only) — deterministic templates, no
  network (demo / CI / no base reachable). NOT offered to the customer as a
  choice; the first-run picker lists only the three bases.

**The team is a roster of schedulable seats, not hand-coded heuristics.**
Doing roles (frontend / backend engineer) drive the *main* session
**serially** (single-writer); reviewing roles (`umadev_agent::critics` —
PM, architect, designer, QA, security, frontend, backend, DevOps) each run
on their **own read-only `fork()`ed session** and review **in parallel**,
returning a structured `RoleVerdict { accepts, blocking, advisory,
evidence }`. Roles communicate only through the shared **blackboard** (the
`output/*.md` artifacts + source tree) and their verdicts — never by
chatting to each other. The director aggregates **deterministically**: the
deterministic floor (coverage / contract / verify / hard gates) governs
loop control, critic opinions are advisory only, blocking findings fold
into one rework directive injected back into the main session, and rework
is **bounded** by a gap counter + stall counter (see
`umadev_agent::acceptance` / `coverage`). The team scales with task
complexity (`*_team_for_kind`): a bugfix convenes no team; a greenfield
build convenes the full roster.

Just typing `umadev` (no subcommand) launches a chat TUI over the same
engine — first launch shows a base picker (language → pick a base) that
writes `~/.umadev/config.toml`; later launches drop straight into the
conversation. Slash commands (`/run` `/continue` `/revise` `/status`
`/help` `/clear` `/quit`, etc.) live inside the chat and mirror the hidden
CLI subcommands.

This is a complete rebuild from a previous Python implementation; do not
look for `umadev/` or `pyproject.toml` — they are intentionally gone.

## Build & test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings   # CI gate: pedantic, warnings = errors
cargo fmt --all

# Run a single test (tests live next to code as `mod tests`):
cargo test -p umadev-host backend_arg_ids_match_host
cargo test -p umadev-contract --test contract        # integration test file
cargo test -p umadev --test e2e
```

Clippy runs at `pedantic` level workspace-wide; new code must pass with
`-D warnings`. `cargo fmt --all` is enforced.

## Workspace layout

Ten crates. The binary depends on everything; the lib crates do not depend
on the binary.

| Crate | Purpose |
|---|---|
| `crates/umadev` | The `umadev` binary. Visible clap verbs: `init` / `adopt` / `history` / `usage` / `lessons` / `pr` / `install` / `uninstall` / `update` / `mcp` / `ci` / `mcp-manage` / `skill` / `knowledge-manage`; hidden-but-scriptable verbs (mirror TUI slash commands): `run` / `quick` / `redo` / `continue` / `revise` / `rollback` / `spec` / `verify` / `deploy` / `report` / `doctor` / `examples` / `guide`; hidden internals: `hook`. Key flags: `run --mode plan\|guarded\|auto` (trust/autonomy tier, default `guarded`; irreversible actions always confirm), `verify --runtime` (boot the app + probe routes → `runtime-proof.json`), `report --review` (PR-ready review report + pre-PR security scan), `deploy --run`, `pr --create`. Bin-only modules: `ci`, `hook`, `mcp` (MCP stdio server), `mcp_manager`, `skill_manager`, `knowledge_manager`, `doctor`. No subcommand → TUI. |
| `crates/umadev-spec` | `UMADEV_HOST_SPEC_V1` as Rust data — clauses, phases, gates, runtime kinds. Normative prose mirror lives in `spec/` (see Spec sync contract). |
| `crates/umadev-governance` | The fail-open enforcement kernel. `rules` (block emoji / hardcoded colors / AI-slop), `policy` (configurable rule policy), `audit` (API + tool-call JSONL), `context` (session injection), `compliance` (UD-EVID-004 → SOC2 / ISO 27001 / EU AI Act), `tokenizer`. |
| `crates/umadev-agent` | The director engine. `router` (typed `RoutePlan`; deterministic Tier-0 floor + optional forked brain consult), `plan_state` (owned `Plan`/`PlanStep` DAG → `.umadev/plan.json`), `context::compose_firmware` (the L0 system-prompt builder: identity + craft + JIT knowledge + lessons digest + repo-map slice, injected on every path), `director` (`summon` / `review` / `verify` / `finalize` levers) + `director_loop` (the drive-verify-self-correct loop with diagnosed, typed blocker disposition + stuck-detector), `events` stream (incl. `IntentDecided` / `PlanPosted` / `PlanStepStatus` / `CriticVerdict`). Plus gate semantics, workflow `state`, `manifest` (UD-META-001), `coach` / `experts` / `lessons` (prompt + methodology injection), `verify`, `scaffolding`, `tech_debt`, `planner` (complexity-tiered prior the router may consult), `coverage` (FR→task spec-coverage check), `checkpoint` (file rewind), `run_lock` (single-writer lock), `acceptance`. The fixed 9-phase chain in `phases.rs` is now the director's *deepest play* (and the env-gated legacy walk), not the only path. Also: `adopt` (brownfield: detect stack + index source + reverse-derive contract), `critics` (role-critic team — PM / tech-lead / senior-design / director seats give structured `RoleVerdict`s over a read-only forked session; fail-open, advisory-only, never drives loop termination), `trust` (`TrustMode::Plan` / `Guarded` / `Auto` autonomy tiers + an always-on irreversible-action floor), `skills` (skill-pack library), `deploy` (detect target + optional deploy → `deploy-proof.json`), `security` (pre-PR scan + owned baseline SAST), `review` (PR-ready review report), `pr` (open the PR), `runtime_proof` (boot + HTTP route probes → `runtime-proof.json`), `error_kb`. Self-evolving memory: `lessons` records pitfalls with a frequency signal and, on a true recurrence, asks the base for a higher-level corrective `Reflection` strategy (`.umadev/reflections/`); retrieval adds HyDE-style query expansion fused via RRF on top of the BM25↔vector dual-channel ranking (in `umadev-knowledge`). |
| `crates/umadev-runtime` | `Runtime` trait + `OfflineRuntime` (deterministic templates) + `RuntimeKind` (wire-protocol tag used by the drivers). The three host drivers impl `Runtime`; UmaDev owns **no** HTTP/model endpoint of its own. |
| `crates/umadev-host` | `HostDriver` trait — drives a logged-in host CLI as a subprocess. Exactly three drivers: `claude` / `codex` / `opencode`. Each impls `umadev_runtime::Runtime` so `AgentRunner` drives it unchanged. `BACKEND_IDS` is the authoritative id list. |
| `crates/umadev-knowledge` | Structured retrieval over the curated `knowledge/` corpus: `chunker` (markdown-aware), `index` (pure-Rust BM25 + CJK-bigram tokeniser, cached to `.umadev/kb-index/`), optional `vector` (embeddings, only when `OPENAI_EMBED_KEY` is set), `retrieve` (single entry point, fail-open → empty result). Also `repomap` — a dependency-light per-language regex symbol scan of the *user's* repo, degree-centrality-ranked and intent-personalized by `RoutePlan.scope`, rendered as a token-budgeted signature outline (`repo_map(root, scope, budget)`), mtime-cached to `.umadev/repomap-cache` — the L0 codebase-context slice. Replaces the old keyword path matcher. |
| `crates/umadev-contract` | Machine-verifiable API contract for UD-CODE-003 (frontend↔backend alignment): parses the architecture-doc API table into a typed `ApiSpec`, renders `openapi.{json,yaml}` to `.umadev/contracts/`, extracts frontend `fetch`/`axios` calls, and cross-validates. Self-contained OpenAPI subset (no `oas3` dep). |
| `crates/umadev-tui` | ratatui terminal app over the engine event stream. |
| `crates/umadev-i18n` | Trilingual (zh-CN / zh-TW / en) string catalogs + system-locale detection for all user-facing text. |

## Conventions

- All `pub` items have docstrings.
- Every governance function is **fail-open**: an error path returns `Decision::pass()` or an empty record. The host MUST NEVER be blocked by a bug in the governor.
- Every clause in `umadev-spec::CLAUSES` is tagged with its `UD-LAYER-NNN` id (e.g. `UD-CODE-001`). When you write or modify a governance rule, reference the clause id in the docstring.
- Tests live next to code (`mod tests { ... }` at the bottom of each `.rs`).

## Spec sync contract

`spec/UMADEV_HOST_SPEC_V1.md` is the normative prose. Any change to
`umadev-spec::CLAUSES` MUST be accompanied by a change to the
matching section of the markdown, and vice versa. The unit tests in
`crates/umadev-spec/src/lib.rs` lock the data shape; add new clauses
there in `UD-LAYER-NNN` order.

## What lives outside the Rust workspace

- `knowledge/` — curated knowledge base (language-agnostic, used by the agent at runtime)
- `umadev-website/` — Next.js marketing site (independent build)
- `output/`, `.umadev/` — per-project user data (gitignored)
- `docs/assets/` — README images

## Anti-rules (do not undo these)

- Do not reintroduce Python packaging (`pyproject.toml`, `umadev/`).
- Only add adapters for hosts that have a documented non-interactive CLI
  form (`binary [flags] "<prompt>"` → stdout). The base-CLI surface has
  deliberately narrowed to **three** first-class drivers (`claude-code`,
  `codex`, `opencode`) — `umadev_host::BACKEND_IDS` is the authoritative
  list, and tests (`backend_arg_ids_match_host` in the binary,
  `BACKEND_IDS.len() == 3` in the host crate) lock it. If you add a fourth,
  update both `BACKEND_IDS` and `BackendArg`, or those tests fail. Broader
  model coverage belongs in the external-HTTP provider path, not new
  base-CLI drivers.
- Do not vendor any host SDK crate. UmaDev is pure-Rust by design.
  Driving the user's *installed* CLI as a subprocess — see
  `umadev-host` — is the intended architecture.
- Governance is **fail-open by contract**: never make a governance function
  return an error that could block the host. An exceptional input returns
  `Decision::pass()` / an empty record. Do not "harden" this into fail-closed.
- Keep `umadev-spec::CLAUSES` and `spec/UMADEV_HOST_SPEC_V1.md` in
  lockstep (see Spec sync contract). The dependency-light lib crates
  (`spec`, `governance`, `contract`) avoid heavy transitive deps on purpose
  — don't pull in large parser/ICU trees.
