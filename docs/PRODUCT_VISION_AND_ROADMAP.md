# UmaDev — Product Vision & Roadmap to Commercial Launch

> **Status (reviewed 2026-07-17): historical implementation roadmap, not a live gap report.**
> The product direction below remains valid, but the “today/current” observations and
> Wave 1–6 checklists describe the baseline from which the director architecture was
> built. Router, owned plan, firmware injection, step scheduling, visible events,
> deterministic verification, delivery artifacts, persistent conversation and
> evidence-gated memory now exist in the default path. Do not use the historical gap table
> to infer present behavior.
>
> Normative behavior comes from [`UMADEV_HOST_SPEC_V1`](../spec/UMADEV_HOST_SPEC_V1.md).
> The current crate topology is documented in [`ARCHITECTURE.md`](ARCHITECTURE.md), and
> the dated, evidence-backed maturity snapshot is
> [`ENTERPRISE_MATURITY_AUDIT_2026-07-14.md`](ENTERPRISE_MATURITY_AUDIT_2026-07-14.md).
> This document preserves why the architecture was chosen and the sequence in which
> it was implemented.
> It is not a completion certificate. The linked audit is a dated snapshot; current
> release readiness must be established from the present test/CI results, the manual
> terminal/OS matrix, and unresolved architecture/security findings rather than a
> fixed count copied into this roadmap.
>
> It defines the target — **UmaDev: a coding agent that works like a real dev team,
> commanding one of five AI coding CLIs you already use**: Claude Code, Codex, and
> OpenCode through vendor-specific transports; Grok Build and Kimi Code through
> isolated vendor profiles on the hardened ACP v1 core.
> The product can simulate product, architecture, UI/UX, frontend, backend, QA,
> security, and DevOps responsibilities through bounded base sessions coordinated by
> a scheduling seat. These are role contracts, not independent people, and the roster
> is proportional to task depth. The remainder preserves an honest gap analysis against the code at the time
> and the executable, impact-ordered roadmap used to reach the target. The headline
> — a coding agent that works like a real dev team — frames this
> whole document: the team is how it works, a coordinator seat schedules and gates it,
> and the layered architecture below is how that team is wired.
>
> **The product is role-based team orchestration.** A coordinator routes the request,
> owns the visible plan, schedules single-writer work and isolated advisory reviews,
> enforces deterministic gates, and leaves an audit trail. This can reproduce useful
> team disciplines; it does not make probabilistic model output equivalent to human
> sign-off or guarantee that every task reaches commercial readiness.
>
> **Non-negotiable identity:** UmaDev is **firmware over a borrowed brain**. It owns
> **no model**, brokers **no endpoint**, and does **not** re-implement the base's
> agentic loop. It borrows the base CLI's brain to **THINK** (route, plan, judge,
> adapt) and directs the base's body to **WORK** (write code, run, fix). Every
> capability below is built as *deterministic Rust orchestration + injected prompt
> firmware + structured artifacts UmaDev owns* — never as cognition UmaDev performs
> itself.

---

## 0. Historical baseline that motivated the roadmap

At the time this roadmap was written, the product the README/spec **described** (a whole development team — driven by a
coordinator that routes, plans, decomposes, schedules the team, delivers a proof-pack,
and learns) was **not the product that shipped by default**. The then-default path (`drive_director_loop`) was *one base build turn + ≤3
rounds of read-only QC*, fed by a *static* directive, with **no routing, no visible
plan, no team scheduling, no learned-memory injection, and no codebase-context
engine**. All of that intelligence exists in the tree but is either stranded in the
env-gated legacy pipeline (`UMADEV_LEGACY_PIPELINE=1`), reduced to a non-binding
prompt hint, or dead code (`director::summon`). The launch work is **composition and
wiring of assets that already exist** plus three new owned primitives (Router, Plan,
visible event surface) — not greenfield invention.

---

## 1. Target architecture that guided the implementation

UmaDev is one engine with five layers — the machinery that turns the eight-role team
into a running system, coordinated by the scheduling seat. The base brain is
consulted at **explicit decision points** to produce **typed artifacts UmaDev owns**;
UmaDev then drives the base body deterministically against those artifacts. Advisory
brain consults, retrieval and governance helpers have bounded degradation; they do
not receive authority merely because a consult failed. Authentication, transport,
hard-gate and verification failures are surfaced as degraded, incompatible, blocked
or failed work rather than being translated into success.

“Fail-open” here describes unavailable advisory cognition and governance error
handling; it does **not** grant authority. Brain-selected writer admission is
fail-closed: the current typed model route must contain the exact legal
authorization `mutating`, and missing/blank/invalid authorization becomes
read-only. The independent deterministic fallback can recognize only an
unmistakable, explicitly scoped current-user request on the resident lane; it
never inherits a malformed model verdict. Plan mode is an independent read-only
ceiling.

```
                         ┌─────────────────────────────────────────────┐
   user turn  ─────────► │  L1  INTENT ROUTER                          │
   (chat / /run /        │  classify → RoutePlan{class,kind,depth,team, │
    ad-hoc task)         │            scope,needs_clarify,est_budget}   │
                         └───────────────┬─────────────────────────────┘
                                         │ RoutePlan
              ┌──────────────────────────┼───────────────────────────────┐
              ▼                          ▼                                ▼
   ┌───────────────┐         ┌───────────────────────┐        ┌────────────────────┐
   │  LIGHT PATH   │         │  DELIBERATE PATH       │        │  CLARIFY           │
   │  chat/explain │         │  L2 PLAN → L3 DRIVE     │        │  one batched, MCQ  │
   │  or quick edit│         │  → L4 VERIFY → L5 LEARN │        │  question, then    │
   │  lock if write│         │  (run-lock, gates)      │        │  re-route          │
   └───────────────┘         └───────────────────────┘        └────────────────────┘
              │                          │
              └──────────┬───────────────┘
                         ▼
        L0  ROUTE-PROPORTIONAL FIRMWARE:
            stable identity; then, as warranted, craft + JIT knowledge/pitfalls
            + repo-map + project facts/run notes  (token-budgeted, curated)
```

### L0 — Firmware injection (the borrowed-brain conditioning layer)

The single boundary where UmaDev conditions the base session. **At roadmap
authoring this was asymmetric and thin on the live path** — `session_for` injected *no* system prompt;
the only firmware is a static `director_build_directive` string. Target: **one
`umadev_agent::context::compose_firmware(root, route, requirement)`** that *every*
path (chat, fast, deliberate) calls to build a curated, token-budgeted system prompt:

- **Identity** — senior director + the seat the current step needs (`experts::agentic_team_identity`).
- **Craft / anti-slop law** — selected for mutating craft routes rather than imposed on pure Chat.
- **JIT knowledge + pitfalls** — a bounded digest keyed on requirement, stack and step; the full knowledge/pitfall path is reserved for Full-tier work. BM25 is the lexical floor, while vectors/HyDE are conditional.
- **Repo-map slice** — a token-budgeted, intent-personalized signature outline for work routes when source is present.
- **Project facts** — safe, non-stale entries from `.umadev/memory/facts.jsonl`, recalled on work-class turns.
- **Run notes** — bounded, untrusted same-run history from `.umadev/run-notes.md`, written by UmaDev only after a step made progress and passed deterministic acceptance. Failed/blocked/empty-review steps do not write, and notes never authorize work.

> **Governance contract is preserved:** firmware is delivered through the selected
> base's supported prompt or protocol surface. Claude Code, Codex, and OpenCode keep
> their vendor-specific transports; Grok Build uses a dedicated profile over the
> hardened ACP v1 core. Authentication, permission
> modes, and resume support are negotiated or handled by the base, never invented by
> UmaDev. UmaDev declares policy; the base executes. We never own cognition.

**Token economy** (the discipline that prevents "context rot"): identity is always
small; craft, governance, knowledge and repo-map are selected only after the current
turn is routed and are budgeted in proportion to that route; prior-phase artifacts
are compacted to decisions, not transcripts. Plain conversation therefore does not
inherit build/governance instructions.

### L1 — Intelligent Router (`umadev_agent::router`, shipping)

The missing brain. Replaces the two current "classifiers" — `looks_like_work_request`
(a substring toggle that only gates injection) and `planner::classify` (a keyword
table reachable only via `/run`). Produces one typed `RoutePlan`:

```rust
pub struct RoutePlan {
    pub class: RouteClass,        // Chat | Explain | QuickEdit | Debug | Build
    pub kind: TaskKind,           // Greenfield | FrontendOnly | Bugfix | Refactor | ...
    pub depth: Depth,             // Fast | Standard | Deep
    pub team: Vec<Seat>,          // who to convene (doers serial, critics parallel)
    pub scope: Vec<PathHint>,     // likely-relevant files (feeds repo-map + retrieval)
    pub needs_clarify: Option<ClarifyQuestion>,
    pub est_budget: Budget,       // rough tool-calls / token ceiling for this turn
    pub confidence: f32,
}
```

**Model-first, fail-open:**
- **Healthy path:** every ordinary natural-language turn receives one structured-JSON semantic decision on a fresh read-only child of the selected base. The decision contains `{class, authorization, kind, complexity, needs, scope, risks, confidence}` and is authoritative in both directions: terse real work may be promoted, while keyword-heavy explanation/status questions may be downgraded to read-only.
- **Availability fallback:** deterministic parsing is used only when the selected model cannot return a valid decision. It is conservative and may execute an unmistakable scoped request on the resident lane, but it cannot launch Director, a role team, or flagship post-build QC on its own.
- **Authorization ceiling:** a brain-selected writer route requires the exact valid typed value
  `authorization: "mutating"`; missing, blank, or invalid authorization fails closed
  to read-only Explain with no team. Explicit read-only wording and Plan mode can
  narrow a verdict, and Plan can never be widened by the model. Inherited
  conversation, plans, TODOs and project documents are context, never current-turn
  authority. Deterministic fallback is computed independently from explicit current
  user text and cannot reinterpret an invalid authorization field as permission.

**Decision points:**
- `class == Chat | Explain` → read-only light path, no writer run-lock.
- `class == QuickEdit | Debug` and `depth == Fast` → fast single-writer turn;
  after a code write, completion additionally requires an observed successful
  targeted verification after the last code write. Mutation alone is neither
  Director admission nor full completion.
- every `class == Build`, plus `class == Debug` at `Standard | Deep` → **Director path** (L2→L5), owned plan, run-lock, proportional gates/team/QC.
- `needs_clarify.is_some()` → emit **one** batched multiple-choice question via the existing `ClarifyGate`, then re-route. Hard rule injected: *never ask what you can discover by reading the code.*

**The decision is surfaced** (`EngineEvent::IntentDecided`) on the governed path so
the user sees why a request entered the development workflow. Explicit `/run` is an
unambiguous Build entry; ordinary conversation remains quiet and proportional.

**Live authority remains separated after routing.** During a writer run, only an
explicit correction to the current task enters steer; questions and future/ambiguous
work queue FIFO for fresh routing after settlement. At a confirmation gate, a
question is answered through an independent read-only query without advancing the
gate, and the gate becomes actionable only after the writer session has ended its
current boundary. Cancellation clears the native resume/session hand-back and writes
a conversation control boundary so the next turn cannot revive cancelled work.

### L2 — Planning & Scheduling (`umadev_agent::plan_state`; introduced by this roadmap)

The keystone of "feels like a real agent." At roadmap authoring there was **no `Plan` data structure**;
the plan lives invisibly in the base's head and the 9-dot phase bar sits frozen at
`0/9` during a director run.

- **Plan synthesis:** before the build loop, one **forked planning turn** asks the brain to emit a strict-JSON plan UmaDev parses and **owns**:
  ```rust
  pub struct Plan { pub steps: Vec<PlanStep>, pub risks: Vec<String>, pub open_questions: Vec<String> }
  pub struct PlanStep { pub id, pub title, pub seat: Seat, pub kind: StepKind /*Build|Review*/,
                        pub depends_on: Vec<StepId>, pub acceptance: AcceptanceSpec, pub status }
  ```
  This is a **dependency DAG**, not a flat list — independent nodes are parallelizable, and `acceptance` makes each step's "done" mechanical, not vibes-based.
- **Visible & steerable:** persisted to `.umadev/plan.json`; rendered live in the TUI as a checklist that ticks off (`[x] scaffold · [~] auth route · [ ] login form  3/8`). A `/plan` command lets the user **reorder / skip / add / veto** a step, folded back into the next directive over the same session. This converts gate-only intervention into step-level control.
- **Scheduling = single-writer + map-reduce-manage:** doing roles drive the main session serially under the run-lock; reviewing critics use fresh Plan-profile child sessions and return bounded `RoleVerdict`s. Critic verdicts are advisory and may be unavailable/empty without deciding success. Observable base-native child work is journaled, but vendor completion is not acceptance; the parent step must still pass the deterministic floor. The team scales with `RoutePlan.depth/team`, and Chat/Explain or narrow fast edits do not convene the full roster.

### L3 — Director Intelligence: drive, adapt, self-correct (`director_loop` rebuilt)

The loop changes from "one mega-turn + generic QC" to **plan-driven, step-by-step,
evidence-grounded**:

1. **Drive** each ready step (deps satisfied) as a `summon(Serial)` directive — finally using the dead `director::summon` lever.
2. **Verify against that step's `acceptance`** using the **deterministic floor only** (coverage / contract / runtime-proof / build-test). Critic verdicts stay **advisory**; the floor is the only thing that flips a step to done or forces rework. Hard rule in firmware: *an open loop that declares done without a green gate is a defect.*
3. **Self-correct with diagnosis, not "go fix it":** tag each blocking finding with a class (build / contract / coverage / behavior / craft), attach the matching `error_kb` playbook + recalled `lessons`, and fold a **concrete** rework directive (with raw failing-test/stderr evidence) back into the session.
4. **Targeted adapt / escalate:** every current blocker receives a typed disposition (`Investigate | FixAdjacent | NoteAndContinue | Escalate`). A run-local tracker counts stable error fingerprints independently and compares source-tree snapshots; the second unchanged observation changes strategy, while the third stops the ineffective repair loop with bounded evidence. Source progress resets the count. An unverified finding is never written directly into `lessons`.
5. **Decide when to involve the user:** clarify only at L1 (intent) and at the two confirm gates; otherwise drive autonomously within the trust tier. Irreversible/out-of-scope actions always hit the floor.

> **Implementation status (honest).** Steps 1–4 ship on the default Director path.
> `drive_plan_steps` invokes `director::summon` per ready step and accepts only the
> deterministic floor. Step, team-review and whole-build QC findings are classified
> as build / contract / coverage / behavior / craft, receive classifier-owned
> `error_kb` root-cause/playbook guidance, and carry raw failing-test/stderr evidence
> plus recalled exact lessons where available. The run-local fingerprint tracker
> changes strategy on the second unchanged observation and settles `Failed` on the
> third, unless a source-tree change resets it. The same `Failed` settlement is
> mandatory for Active/Pending/incomplete plans, dirty final QC, or residual findings
> after the round/time budget; none may become `Done`. This diagnoses known families;
> generic findings deliberately stay `Investigate` rather than inventing a repair.

**Context survival across a long build:** a **compaction module** clears consumed
tool outputs first, then (if still over budget) summarizes into a fixed template
(*Primary Intent · Files & Code · Errors & Fixes · Pending Tasks · Current Work · Next
Step*), reopening the session seeded with the summary + most-recent artifacts. Plan
progress from `.umadev/plan.json` plus bounded `.umadev/run-notes.md` history can be
re-read at step boundaries. Run notes are not a complete transcript or a promise of
exact vendor-session restoration; exact resume remains capability-specific.

### L4 — Mature functions & verification (deterministic floor, on by default)

- **Acceptance gate is real and required for heavyweight kinds:** `coverage` (FR→step) + `acceptance` (task→API) + `umadev-contract` validation + `verify --runtime` (boot + route probe → `runtime-proof.json`) become **required signals on the default deliberate path**, not legacy-only. For bugfixes, require a **failing reproduction test that actually reproduces the issue**, gated on red→green plus regression staying green.
- **Delivery artifacts restored on the default path:** `output/*-prd.md`, `-architecture.md`, `-uiux.md`, scorecard HTML, and the zipped **proof-pack** — lifted out of `phases.rs` into a `director::finalize()` that runs once QC is clean (gated by depth, so a todo page doesn't get a proof-pack).
- **Owned baseline SAST** so `security` produces findings tool-free (extend the `rules` engine with injection / missing-auth / hardcoded-secret heuristics); gitleaks/semgrep remain optional upgrades.
- **Git trust substrate (target, not fully shipped):** the roadmap proposed auto-committing every accepted step and implementing rollback as real `git revert`. Current isolation is narrower: a clean Git worktree can use a derived `umadev/<task>` branch; non-Git or dirty worktrees report that isolation was skipped. Checkpoints are not a claim that every accepted step is auto-committed. UmaDev never auto-merges or pushes.

### L5 — Memory & learning (evidence-gated on the live path)

- **Pitfalls and lessons:** project incidents count independent episodes, not repeated stderr lines. A recurrence may produce a pending candidate; validation requires the exact repair attempt followed by the same verifier passing. Generic/unclassified incidents are quarantined rather than injected as advice.
- **Learned skills and recipes:** both are project-local candidates. Non-trivial clean delivery is required for skill graduation; recipes require strict stack/kind/shape matching. Retrieval alone is not use evidence: an exact prompt-delivery receipt plus deterministic pass/fail/unknown outcome settles later utility, with unknown neutral.
- **Facts:** stable project/environment facts are extracted after meaningful work by a bounded read-only pass, secret-filtered, and recalled only on work-class turns. Stale, contradictory, or missing-path facts are demoted/tombstoned.
- **Run notes:** UmaDev writes at most one bounded note after a plan step both makes progress and passes deterministic acceptance. Failures, blocked steps and empty reviews do not write; the base cannot write the file directly. Notes are same-run untrusted history, not authorization or completion evidence.
- **No automatic policy promotion:** the live path does not promise periodic semantic consolidation, automatic prevention, cross-project procedural learning, or auto-commits into `AGENTS.md`/rules. Those are product hypotheses requiring separate evidence and user review.

Memory capture and recall are fail-soft, while exact sent-memory receipts keep outcome attribution auditable. Event richness remains transport-specific; a capability that Grok Build does not advertise is not guessed.

### Conversation as a first-class surface

Chat is the everyday surface and was then a goldfish: the `conversation` buffer was
built, trimmed to 16, and **never sent** to the brain (memory was delegated entirely
to a native resume mechanism); restart meant amnesia; offline chat returned empty; chat and
`/run` use **disjoint** sessions. Target:

- **Send UmaDev's own bounded transcript every turn** (native resume or negotiated ACP `session/resume` / `session/load` becomes belt-and-suspenders, not the only memory).
- **Persist + resume chat sessions per project** (`.umadev/chat/<id>.json`), with `/sessions` and `/resume`.
- **Unify chat ↔ `/run` memory:** hand the finished director session back to chat so "what did you just build?" continues the *same* session that did the build.
- **Context management:** token-budgeted summarize-and-fold (a `/compact` verb) instead of FIFO-drop-at-16.
- **Offline fallback:** context-aware deterministic templates may support demos/tests, but they are not a coding brain and must not claim completed work. The proposed `External`/HTTP model arm is retired as contrary to the product identity: UmaDev owns no model endpoint; real cognition requires one of the five authenticated bases.

---

## 2. Historical gap analysis — roadmap baseline vs target

> The table in this section is intentionally retained as implementation history.
> It is not a current defect list. Consult the dated enterprise audit for remaining
> gaps and the code/tests for behavior.

Ranked by impact on "not a real agent / not launchable."

| # | Gap | Current reality (file evidence) | Target |
|---|-----|--------------------------------|--------|
| **G1** | **No router on the live surface.** Every non-slash message is one undifferentiated base turn; `/run` planner is keyword-only and walled off. | `app.rs:2439`, `lib.rs:2183`, `director_build:false` hardcoded `lib.rs:1479`; `planner::classify` substring table `planner.rs:288`. | L1 Router; auto fast-vs-deliberate; visible route. |
| **G2** | **No plan UmaDev owns; phase bar frozen 0/9.** Plan lives in the base's head; only `Note`+`WorkerStream` emitted. | `director_loop.rs:237,261-308`; `experts.rs:804` ("no fixed phase checklist"); `app.rs:8166` ("the 0/9 window"). | L2 `Plan` DAG; live, steerable `PlanUpdated` surface. |
| **G3** | **Firmware is thin/asymmetric on the default path.** Default session gets no system prompt + a static directive; knowledge/lessons/repo-map never reach it. | `host/lib.rs:1205-1219` ("we append no extra system prompt"); lessons calls only in `runner.rs`. | L0 `compose_firmware` on every path. |
| **G4** | **No codebase-context engine on the live path.** No repo-map/symbol graph; `adopt` is manual + legacy-only; brownfield/explain/navigate add nothing over the bare base. | no tree-sitter/symbol dep; `brownfield_*` wired only into `runner.rs:2966,2991`. | L0 repo-map slice + auto-adopt + JIT retrieval everywhere. |
| **G5** | **Learned memory is dark in the shipping path.** Sophisticated lessons/reflection/error_kb fire only in legacy. | `lessons.rs` (5.8k LOC) called only from `runner.rs`. | L5 wired into `director_loop`. |
| **G6** | **The "team" is invisible and post-hoc, not a driven workflow.** `summon` is dead; critics review *after* the base built solo; verdicts stranded as bland `Note`s. | `director.rs:14-15` (admits summon unused, task #22); `director_loop.rs:494`. | L2 scheduling + `EngineEvent::CriticVerdict` panel. |
| **G7** | **Self-correction is shallow & generic.** Catches only no-source / emoji-color / build-fail; responds with one undifferentiated "go fix it"; re-tries identically. | `director_loop.rs:451-466,489,530`; acceptance/coverage legacy-only. | L3 typed disposition + diagnosed correction + stuck-detector. |
| **G8** | **Delivery artifacts gone by default.** No PRD/architecture/scorecard/proof-pack — only source files. | artifact writers stranded in `phases.rs:2129-2269`. | L4 `director::finalize()`. |
| **G9** | **`continue`/`revise`/`redo` don't compose with the default engine.** They resume on gates the director path never emits; `continue` opens a cold session. | `main.rs` `cmd_continue` cold-session; gates legacy-only. | Unified engine; persist + resume the same base session. |
| **G10** | **First-run "ready" mark is a lie.** Picker probes `--version` (PATH), not auth; a not-logged-in base shows green and fails mid-run. | `claude.rs:704`, `host/lib.rs:1274`, `doctor.rs:238`. | Real auth probe in picker (Wave 1 quick win). |
| **G11** | **Chat memory thrown away; restart = amnesia; offline silent; chat↔run split.** | `lib.rs:1352` (transcript not sent), `app.rs:593-595,683-700`, `runtime/lib.rs:312`, `lib.rs:1877-1883`. | First-class conversation surface. |
| **G12** | **Spec/docs/code three-way drift.** Spec normative chain = 9 phases; architecture doc says "no fixed chain"; `CLAUDE.md` says "9-phase runner is core." | `spec:894`, `AGENT_WIELDS_BASE_ARCHITECTURE.md:49,133`. | One canonical truth (Wave 6). |
| **G13** | **Governance asymmetric across bases.** Real-time PreToolUse hook is claude-only; audit/usage empty on a default codex/opencode run. | `hook.rs:3`. | Per-turn governance scan for all bases; honest in-product disclosure. |

**The throughline:** UmaDev's *real, working asset today is the firmware* (craft taste +
honesty floor + curated knowledge) — a genuine reason to prefer it over the bare base,
but it is thin and **invisible**, and everything *above* the firmware (routing, plan,
team, delivery, memory) is dead code, legacy-only, or aspirational. The launch gap is
**wiring + visibility + three owned primitives**, not invention.

---

## 3. Historical implementation roadmap — impact-ordered waves

Each wave is a coherent shippable increment. **TS** = table-stakes (a launch can't ship
without it). **DIFF** = differentiating (the reason a user picks UmaDev). Every brain
consult is fail-open to a deterministic floor; no wave makes a governance function
fail-closed; no wave adds a model endpoint or re-implements the base loop.

---

### WAVE 1 — "It thinks, plans, and shows it." *(the user's sharpest pain)*  — TS + DIFF
**Goal:** the default surface routes intelligently, owns a visible/steerable plan, and
*looks and behaves* like a director. This single wave converts "feels like the bare
base, slower" into "a director ran my project."

**Deliverables**
1. **Intent Router** — `umadev-agent/src/router.rs` (NEW): `RoutePlan`, Tier-0 deterministic + Tier-1 forked-JSON consult (clone `surface_intake_plan`'s pattern), reconciliation with safe floor. Wire into the **one bypassed entry point**: `umadev-tui/src/app.rs:2439-2453` / `lib.rs:2183-2193`, and make `AgenticTurn` (`lib.rs:755`) carry `route: RoutePlan`, dropping the hardcoded `director_build:false` (`lib.rs:1479`) so chat can auto-promote into a build.
2. **Owned Plan** — `umadev-agent/src/plan_state.rs` (NEW): `Plan`/`PlanStep` DAG; a forked planning turn before the build loop in `director_loop.rs:261`; persist `.umadev/plan.json`; parse via the existing `extract_json_object`. Fail-open: unparseable → today's single-turn behavior.
3. **Visible surface** — `events.rs`: add `IntentDecided`, `PlanPosted`, `PlanStepStatus`/`PlanUpdated`, `CriticVerdict`. Emit from the router + plan loop + critic fan-out (replace the bland `Note` at `director_loop.rs:494`). Render in `umadev-tui/src/app.rs`/`ui.rs`: an intent pre-commitment card, a live plan checklist (replacing the frozen 0/9 bar on the director path), and a collapsible team-review panel.
4. **Step-level steering** — `/plan` command to reorder/skip/add/veto; fold edits into the next directive over the same session (reuse `queued_steer`).
5. **Auth probe in picker** (cheap, P0 trust fix) — `umadev-host`: add `auth_state` to `ProbeResult` (cheapest authenticated no-op per base); three picker states (`logged in` / `installed · not logged in → <login cmd>` / `not installed → <install cmd>`); block commit on not-authed.

**Touches:** `umadev-agent/{router,plan_state,events,director_loop}.rs`, `umadev-tui/{lib,app,ui}.rs`, `umadev-host/{lib,claude,codex,opencode}.rs`.

**User-visible outcome:** type "build me X" in chat → UmaDev says *"I'll BUILD this (~N min, files under src/) — here's my plan: [8 steps]. [c] go · [revise] · [chat]"*, then drives it step-by-step with a checklist ticking off and a visible team review — and the first-run picker tells the truth about login.

---

### WAVE 2 — "It actually directs a team, with real firmware." — DIFF
**Goal:** make the plan *driven* (not just shown) and deliver UmaDev's firmware on the
live path, so the depth that already exists in the tree is on by default.

**Deliverables**
1. **`compose_firmware`** — `umadev-agent/src/context.rs` (NEW/expanded): one builder for stable identity plus a route-proportional overlay. Chat stays light; work routes select craft, repo context, facts, and (at Full tier) JIT knowledge/pitfalls. The original wave applied the delivery contract to the three vendor-specific bases; the current implementation extends it to Grok Build through its negotiated ACP surface.
2. **Drive the plan via `summon`** — wire `director::summon(Serial)` into `director_loop` for each Build step; `summon(Parallel)`/`review` for Review steps. Map-reduce-manage: decomposable steps fan to isolated forked sub-sessions returning structured summaries.
3. **Brain-assisted team sizing** — lift `*_team_for_kind` (`critics.rs:570`) out of `/run`-only; size the roster from `RoutePlan.team`/depth on every path.
4. **Wire learned memory + audit + usage into the default loop** — call `lessons::capture_*` / `record_tool_call` / `record_usage` from the `drive_one_turn` event pump (`director_loop.rs:365-378`); inject recalled pitfalls into the directive.

**Touches:** `umadev-agent/{context,director,director_loop,critics,lessons}.rs`, `umadev-host/src/lib.rs`.

**User-visible outcome:** `/lessons` and `/usage` are real; the team is an actual workflow (architect → backend → frontend, governed, serial) not a post-hoc critique; the same prompt now visibly carries UmaDev's craft + project knowledge + learned pitfalls into the base.

---

### WAVE 3 — "It understands your codebase." — TS (for brownfield) + DIFF
**Goal:** close the single biggest competitive gap — on a real repo, UmaDev currently
adds nothing over the bare base.

**Deliverables**
1. **Repo-map / symbol graph** — `umadev-knowledge` (NEW module): a dependency-light symbol scan (regex/ctags-style to honor the anti-rule against heavy parser trees; tree-sitter only if budget allows), ranked (PageRank-style) and **intent-personalized** by `RoutePlan.scope` + current step; rendered as a token-budgeted signature outline; cached by mtime in `.umadev/`.
2. **Auto-adopt + shared project context** — `umadev-agent/src/context.rs::project_context(root, query)`: auto-run a fast incremental index on first turn in a non-empty repo; promote `brownfield_code_snippets` out of `runner.rs:2991` into this shared helper consumed by L0 on every path.
3. **Incremental verify** — in `run_auto_qc`, prefer reading the base's *own* just-run build/test result over re-running the full suite; only re-run the specific failing step (kills the duplicate-build cost on heavy QC).

**Touches:** `umadev-knowledge/src/*`, `umadev-agent/{context,adopt,director_loop}.rs`.

**User-visible outcome:** "explain this code", "fix the bug in checkout", "add a field to the user model" all get a real repo-aware answer/edit; brownfield feature-add stops feeling like the bare CLI.

---

### WAVE 4 — "It delivers proof and verifies itself." — TS
**Goal:** restore commercial-grade delivery + make the acceptance gate real on the
default path.

**Deliverables**
1. **`director::finalize()`** — lift artifact writers from `phases.rs:2129-2269`; produce PRD/architecture/uiux/scorecard/proof-pack once QC is clean, gated by depth.
2. **Required acceptance floor for heavyweight kinds** — promote `coverage` + `acceptance` + `umadev-contract` + `verify --runtime` into the default deliberate path; for Bugfix, require a reproduction test (red→green) + green regression.
3. **Diagnosed, escalating self-correction** — typed blocker disposition (`Investigate|FixAdjacent|NoteAndContinue|Escalate`); `error_kb` playbook + `lessons` folded into the fix directive; stuck-detector over the event log → strategy change or a public `Failed` settlement carrying bounded blocking evidence.
4. **Owned baseline SAST** — extend `rules` with injection/missing-auth/secret heuristics so `security`/`report --review` find things tool-free.

**Touches:** `umadev-agent/{director,director_loop,acceptance,coverage,security,error_kb}.rs`, `umadev-contract`, `umadev-governance/src/rules.rs`.

**User-visible outcome:** a default `/run` again leaves a PRD, an architecture doc, a runnable proof, a scorecard, and a shareable proof-pack — and won't declare done without a green gate.

---

### WAVE 5 — "It remembers and you can converse with it." — TS
**Goal:** fix the conversation goldfish + unify engines for honest resume.

**Deliverables**
1. **Send UmaDev's transcript every turn** (`lib.rs:1352`); thread `conversation` into `AgenticTurn`. **Persist + resume per-project chat** (`.umadev/chat/<id>.json`); `/sessions`, `/resume`, `/compact` (token-budgeted summarize-and-fold replacing FIFO-16).
2. **Unify chat ↔ `/run` memory** — don't close+clear the held session on run finish (`lib.rs:1877-1883`); hand it back to chat as the active session.
3. **Engine unification & session-carrying `continue`** — `drive_director_loop` emits `GateOpened` + persists `workflow-state.json` (incl. `plan_steps`) at checkpoints so `continue`/`revise`/`redo` operate on the live engine; expose the gate only after the writer boundary has ended, answer gate-local questions through an independent read-only query, and resume the **same** base session id (drivers already support `set_session_id`) instead of a cold one.
4. **Cross-session goal continuity** — on launch, if `.umadev/plan.json` has an unfinished plan, surface "resume goal X (step 3/7)?"; in `auto` tier, drive to completion.
5. **Offline chat reply** — context-aware deterministic fallback (no more empty `String::new()`). The once-proposed `External`/HTTP model arm was deliberately retired: it would make UmaDev a model-endpoint broker and violate the borrowed-brain identity.

**Touches:** `umadev-tui/{lib,app}.rs`, `umadev-agent/{state,director_loop}.rs`, `umadev/src/main.rs`, `umadev-runtime/src/lib.rs`.

**User-visible outcome:** reopen UmaDev and the conversation + goal are still there; ask "why did you do it that way?" after a build and it answers from the same session; `continue` actually continues.

---

### WAVE 6 — "Trustworthy, legible, and consistent." — TS (launch hardening)
**Goal:** the unglamorous commercial-grade + credibility layer.

**Deliverables**
1. **Graduated trust + recovery UX** — decompose `TrustMode` into per-capability auto-approve (read auto / write guarded / shell confirm / network allowlist) with the always-on irreversible floor + a circuit breaker; turn aborts into action cards (`Retry / Switch base / Open log / Run doctor / Show login cmd`); adaptive stall threshold (longer during installs/builds).
2. **Branch isolation + git substrate** — use a derived `umadev/<task>` branch when a clean Git worktree permits it; report a skip for non-Git/dirty worktrees and preserve existing changes; never auto-merge. Real-git rollback remains a separate target, not an implication of the current shadow checkpoint mechanism.
3. **Governance parity disclosure** — run the content-governance scan after every base turn for codex/opencode; honestly surface real-time-hook parity.
4. **Spec/docs/code reconciliation** — make the director/USB model canonical; demote the 9-phase chain to "the deep-commercial-build play the director may choose"; rewrite README "how it works / deliverables / team"; finish/close tasks #22 (delete or wire dead `summon`/`checkpoint`/`director_build` branches) and #23; retire the env-gated legacy engine or make it the director's internal "Deep" tier.
5. **Cost/time expectation setting** + contextual discoverability nudges.

**Touches:** `umadev-agent/{trust,run_lock,pr,checkpoint}.rs`, `umadev-tui/*`, `umadev/src/hook.rs`, `spec/`, `docs/`, `README`.

**User-visible outcome:** users can watch, steer, stop, undo, and trust; the docs match the product; codex/opencode users know exactly what they get.

---

**Sequencing rationale:** W1 attacks the literal complaint (routing + visible plan + "feels real") and the worst first-run cliff. W2 makes the plan *driven* and delivers the firmware that justifies the product. W3 closes the biggest competitive gap (real repos). W4 restores table-stakes delivery + a gate that says no. W5 makes it remember. W6 hardens for trust and reconciles the story. W1–W3 are the launch-critical trio; without them a user on a real repo bounces back to their base CLI.

---

## 4. Competitive Positioning

### Why a user picks UmaDev

**vs using a supported base CLI directly:** UmaDev adds a governance-and-delivery
director: typed routing, a visible owned plan for deliberate work, single-writer
execution, isolated advisory reviews, route-proportional standards/project memory,
and a deterministic acceptance floor that can refuse completion. Some base CLIs may
offer their own planning, subagents, memory, or safety features; the differentiator is
UmaDev's consistent, inspectable contract over those capabilities, not a claim that a
bare base has none. UmaDev keeps the base's brain and subscription (no UmaDev-owned
model bill, API key, or login) and exposes that contract across **all five** first-class bases. The
three vendor-specific transports and the isolated Grok Build/Kimi Code ACP profiles are not identical: authentication,
permission modes, resume, and optional events remain vendor-specific and are used
only when advertised or otherwise documented by that base.

**vs using a base directly:** the selected base keeps its own model, tool loop, and
vendor strengths. UmaDev adds a terminal-native, governance-first director layer:
multi-seat review, an anti-slop craft floor, proof-pack delivery, and evolving project
memory without taking over the base's account or model endpoint.

**vs Devin:** Devin owns its model, VMs, and ACU economics. UmaDev is the **open,
local-first, bring-your-own-brain** director: the same single-writer/map-reduce-manage
doctrine and plan-before-spend checkpoint, but running on the user's existing base
subscription, on their machine, with their data, with a transparent audit trail — no
proprietary endpoint, no ACU meter.

### Original launch bar (retained for architectural review)

1. **The default path must deliver the firmware** (W2) — today it barely beats the bare base. *Non-negotiable.*
2. **Routing + a visible, steerable plan must be real** (W1) — this is the felt difference between "agent" and "wrapper."
3. **It must understand the user's existing code** (W3) — most dev work is brownfield; without repo-awareness UmaDev loses every non-greenfield scenario.
4. **The acceptance gate must say no, and delivery must produce proof** (W4) — "commercial-grade" is verification + shareable artifacts, not vibes.
5. **It must remember across turns and sessions** (W5) — a goldfish never feels like a colleague.
6. **The first-run signal must be honest and the docs must match the product** (W1 auth probe, W6 reconciliation) — trust is lost permanently at the first lie.
7. **Advisory subsystems stay fail-soft over a borrowed brain** — retrieval, critics and governance bugs degrade safely, while writer authorization, irreversible actions, hard gates and completion evidence remain fail-closed or explicitly failed. The day UmaDev owns a model endpoint or forks the base loop, it loses its identity and its reason to exist.

> **The bet:** the 2026 differentiator is no longer price or raw model IQ — it's *the
> loop, the gates, the plan, the taste, the memory, and the transparency*. The tree
> contains implementations for these areas, but commercial readiness still depends
> on present CI, real-terminal/platform validation, evidence quality, and the honest
> capability boundaries documented above.
