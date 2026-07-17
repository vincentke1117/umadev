# UmaDev Host Specification, Version 1 (UMADEV_HOST_SPEC_V1)

> **Status:** Draft  
> **Version:** 1.0.0-draft.12
> **Date:** 2026-07-17
> **Editor:** UmaDev maintainers (`<11964948@qq.com>`)  
> **License:** MIT  

This document defines the **UmaDev Host Specification**: the set of
constraints, contracts, artifacts, and evidence requirements that an AI
coding host MUST satisfy to be called a *conformant UmaDev host*.

This specification is the normative product contract. The reference
`umadev` binary is a pure-Rust director, injector, and verifier that drives
an already-authenticated base CLI; it owns no model endpoint and the base is
the component that reasons, writes code, and runs tools. Other delivery
surfaces (`SKILL.md` packages, the governance MCP server, hooks, and adapter
recipes) inject or verify parts of the same contract.

### The coach metaphor

The reader's everyday mental model for UmaDev is this:

> **UmaDev is a coach/director for the host.** It does not generate code with
> an UmaDev-owned model; the selected base performs the coding.
> It hands the host a complete, pipeline-shaped playbook for delivering a
> commercial software project — what to research first, what artifacts to
> produce, when to pause and ask for sign-off, what to refuse to write,
> what evidence to leave behind — then remains in the loop to own the plan,
> stage gates, collect evidence, and enforce deterministic acceptance. The
> host's existing model + tools execute; the coach's standard is what
> makes the result auditable. Commercial readiness still depends on the
> observed evidence; the metaphor is not a guarantee of quality.

The remainder is the playbook, expressed as machine-verifiable normative
clauses. The user-facing CLI can install guidance, direct a run, verify the
workspace, and report the resulting evidence.

Coding hosts are independent products. UmaDev's first-class drivers are
**exactly five** base CLIs — `claude-code`, `codex`, `opencode`, `grok-build`,
and `kimi-code`
(`umadev_host::BACKEND_IDS` is the authoritative list); everything else is
out of first-class support. Any other host MAY still meet this spec by adopting
UmaDev's reference injectors, or by implementing the rules natively. Both
paths produce a conformant host.

## 1. Conformance and conventions

### 1.1 Keywords

The keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, **MAY**,
**REQUIRED**, **OPTIONAL**, and **RECOMMENDED** in this document are to be
interpreted as described in RFC 2119, when, and only when, they appear in
all capitals.

### 1.2 Clause identifiers

Every normative clause has a stable identifier of the form `UD-<LAYER>-<NUM>`,
where `<LAYER>` is one of:

- `CODE` — code-weight constraints (Layer 1, §3)
- `FLOW` — flow contract (Layer 2, §4)
- `ART` — delivery artifacts (Layer 3, §5)
- `EVID` — evidence chain (Layer 4, §6)
- `META` — versioning and conformance declaration (§8)

§7 (host surface mapping) is **non-normative guidance**: it carries no
clauses and therefore reserves no `UD-HOST-*` identifier space. Every
normative clause lives under one of the prefixes listed above.

Clause IDs are permanent. A clause MAY be deprecated in a later version
but MUST NOT be renumbered or repurposed.

### 1.3 Conformance levels

A host declares its conformance level via the `UMADEV_HOST_SPEC_V1`
manifest (§8.1). Three levels exist:

| Level | Definition |
|---|---|
| **L1 — Aware** | The host loads UmaDev's persistent rules surface (e.g. `CLAUDE.md`, `AGENTS.md`, equivalent). The model sees the rules but enforcement is advisory. |
| **L2 — Enforced** | All MUST-level rules in §3 are enforced at the host's tool-call boundary (block before write). Confirmation gates in §4 are honored. |
| **L3 — Audited** | L2 plus all evidence requirements in §6 produce machine-readable artifacts. The host produces a UmaDev–compatible proof pack. |

A host that satisfies all MUST clauses at the L3 level is **fully
conformant**. Hosts MAY claim partial conformance at L1 or L2.

### 1.4 Test vectors

Each enforceable clause SHOULD link to a test vector — a `(file_path,
content) → expected_decision` tuple that any implementation can run to
verify its enforcement layer. Reference JSON vectors live in
`tests/spec_vectors/`; `crates/umadev-governance/tests/spec_vectors.rs`
executes them against the Rust implementation.

## 2. Definitions

- **Host** — a user-facing AI coding product (CLI, IDE, agent, web app)
  that invokes a language model on the user's behalf and may call tools
  (read/write file, run shell, etc.).
- **Tool call** — any model-initiated action that mutates the user's
  workspace or environment.
- **Pre-write checkpoint** — the host-internal hook fired immediately
  before a Write/Edit/Patch tool call is dispatched. MUST be cancellable.
- **Post-write checkpoint** — the host-internal hook fired immediately
  after a Write/Edit/Patch tool call has returned.
- **Prompt checkpoint** — the host-internal hook fired immediately before
  the model receives the user's prompt.
- **Workspace** — the directory the host treats as the project root.
- **Surface** — a host-native configuration file or directory through
  which rules can be injected (e.g. `CLAUDE.md`, `AGENTS.md`, MCP server
  registration, hooks.json).
- **Spec injection** — the act of writing/updating a surface so that a
  host enforces a UmaDev clause.

## 3. Layer 1 — Code-weight constraints

This layer governs **what model-emitted code MUST NOT contain** when it
lands in a UI / source file.

### 3.1 Emoji as functional icons (`UD-CODE-001`)

> Level: **MUST**  
> Test vector: `tests/spec_vectors/UD-CODE-001.json`

A conformant host **MUST** refuse a Write/Edit/Patch operation if, in any
file whose extension is in {`tsx`, `ts`, `jsx`, `js`, `vue`, `svelte`,
`astro`}, the *new content* contains any codepoint in the ranges:

- `U+2600–U+27BF` (Miscellaneous Symbols / Dingbats)
- `U+1F300–U+1FAFF` (Pictographs / Symbols / Supplemental Symbols)
- `U+1F900–U+1F9FF` (Supplemental Symbols and Pictographs)
- `U+1FA70–U+1FAFF` (Symbols and Pictographs Extended-A)

The host **MUST** return a refusal reason that instructs the model to use
a declared icon library (e.g. Lucide, Heroicons, Tabler).

A conformant host **SHOULD NOT** apply this rule to documentation files
(`.md`, `.mdx`, `.rst`, `.txt`).

### 3.2 Hardcoded color literals (`UD-CODE-002`)

> Level: **MUST**  
> Test vector: `tests/spec_vectors/UD-CODE-002.json`

In files whose extension is in {`tsx`, `ts`, `jsx`, `js`, `vue`, `svelte`,
`astro`, `css`, `scss`, `sass`}, a conformant host **MUST** refuse a
Write/Edit if the new content contains any literal chromatic color of the
forms:

- `#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA` (case-insensitive)
- `rgb(...)`, `rgba(...)`
- `hsl(...)`, `hsla(...)`

The following are **exempt**:

- The achromatic literals `#fff`, `#ffffff`, `#000`, `#000000`
- CSS custom property references (`var(--*)`)
- Files whose path matches any of:
  - `/tokens/`, `/theme/`, `/themes/`, `/design-system/`, `/design-tokens/`
  - `/.storybook/`, `*.stories.*`, `*.test.*`, `*.spec.*`
  - `/fixtures/`, `/mocks/`

A conformant host **SHOULD** include the offending literal in the refusal
reason (up to 5 distinct examples) to aid the model's self-correction.

### 3.3 Frontend–backend API path alignment (`UD-CODE-003`)

> Level: **SHOULD** at L2; **MUST** at L3.  
> Test vector: `tests/spec_vectors/UD-CODE-003.json`

In files of frontend extension (see §3.1), a conformant host **SHOULD**
extract every URL emitted by `fetch(...)`, `axios.<verb>(...)`,
`ky.<verb>(...)`, `useSWR(...)`, `useQuery(...)`, or `http.<verb>(...)`,
filter to paths beginning with `/`, and:

- At **L2**: persist the extracted set into the workspace audit log (§6.1).
- At **L3**: additionally cross-check each path against backend route
  definitions (any of `output/*-architecture.md`, `umadev.yaml`,
  `openapi.yaml`, or an auto-discovered framework route table). If a
  frontend path does not appear in any backend source-of-truth, the host
  **MUST** surface this as a `compliance:api-mismatch` warning in the
  Quality Report (§6.3).

A conformant host **MUST NOT** block a Write solely on §3.3 — alignment
is verification-time, not write-time, because backend route definitions
may not yet exist in early phases.

### 3.4 Tech-stack pre-research (`UD-CODE-004`)

> Level: **SHOULD**

Before the host writes its first non-scaffolding source file in a fresh
project, the conformant host **SHOULD**:

1. Read the project's dependency manifest (`package.json`,
   `requirements.txt`, `pyproject.toml`, `go.mod`, ...).
2. For each top-level framework with declared version, fetch the official
   docs (or rely on cached knowledge) before generating code for it.

This is an advisory clause and is **not** machine-verifiable at L2. At
L3, the Quality Report (§6.3) **MUST** include a section indicating
whether pre-research occurred.

### 3.5 Test-integrity / anti-reward-hacking guard (`UD-QA-001`)

> Level: **MUST**

A borrowed brain can make a failing test suite report "pass" without
delivering working code by **gaming the tests** rather than fixing the
implementation. A conformant host **MUST NOT** trust a step's passing test
signal when, across that step's doer turn, the project's **test files** were
weakened. The host **MUST** deterministically compare the test surface
*before* and *after* the doer turn and treat any of the following as a
blocking integrity violation:

1. a test file or a test case (`it` / `test` / `def test_` / `#[test]` / …)
   was **deleted**;
2. the **assertion count** in a kept test dropped with no corresponding test
   removed (assertions stripped out of a surviving test);
3. a **skip / xfail / ignore / `.only` / focus** marker, or commented-out
   test code, was newly introduced;
4. a test now **asserts a hard-coded literal that matches the
   implementation's own output** (best-effort), trivially forcing a green;
5. the **test harness / runner config or the test command** (`jest.config.*`,
   `pytest.ini`, `package.json` `scripts.test`, …) was **modified or deleted**
   during the build step.

The guard is part of the **deterministic floor** (not an advisory critic): a
violation is folded into the step's acceptance as a blocking finding with a
typed, file-naming directive, driving a **bounded** rework round under the
host's existing fix-round / stall counters — never an open-ended loop. It is
**fail-open**: when integrity cannot be determined (no baseline snapshot, an
unreadable tree, an unparseable file) the guard yields **no** finding rather
than a spurious block, and **adding** new tests is never a violation — only
the destruction or weakening of pre-existing test signal is. A
genuinely-passing, un-gamed suite is unaffected.

### 3.6 Architecture-fitness floor (`UD-CODE-006`)

> Level: **MUST** (sub-rules `UD-CODE-006c/d` are advisory)

The injected firmware *preaches* layering, small focused modules, and no
copy-paste — but a prompt is not a floor. A borrowed brain under pressure
ships one giant file, imports the database from the UI, and pastes the same
block into three places while every other deterministic check (build/test,
coverage, contract, test-integrity) still reads green. A conformant host
**MUST** verify architecture fitness on its own deterministic floor, with
four sub-rules:

1. **God-file gate** (`UD-CODE-006a`, blocking) — a **NEW** source file over
   500 lines, or a touched file that **grew past** 800 lines during the
   step, is a blocking finding carrying a split directive ("split by
   feature/domain"). The grown ceiling **MAY** be overridden by host
   configuration; generated, vendored, lock, and test files are exempt.
   Without a before-baseline, newness cannot be established, so only the
   hard grown ceiling applies to a touched file — a merely-touched legacy
   file is never falsely blocked.
2. **Layer-dependency rules** (`UD-CODE-006b`, blocking) — the architecture
   document (`output/<slug>-architecture.md`) **MAY** declare a layering
   contract: a directory→layer mapping table, a one-way `->` order chain,
   and/or explicit `LAYER-RULE: a !-> b` ban lines. When declared, every
   resolved import edge of the repository **MUST** be checked against it;
   an edge that goes against the declared one-way order or crosses a
   banned pair is a blocking finding naming both files and the violated
   rule. No declaration in the document → the check silently no-ops.
3. **Clone detection** (`UD-CODE-006c`, advisory) — normalized
   (whitespace-squeezed, comment-stripped) windows of code **added** in
   touched files are compared against the rest of the repository; a
   duplicated block of at least 5 lines yields an advisory finding naming
   the sibling location ("reuse it instead"). Deduplication judgment stays
   with the critics and the user; the host **MUST NOT** block on this
   sub-rule alone — the floor only surfaces the evidence.
4. **Comment hygiene** (`UD-CODE-006d`, advisory) — the host **SHOULD** flag
   a touched source file when it newly gains an ordinary comment-only block
   of at least 8 consecutive lines, or at least 12 ordinary comment-only
   lines that outnumber its code lines. This rule **MUST NOT** require a
   comment quota and **MUST NOT** count API/documentation comments, license
   headers, generated files, vendored files, or tests. The finding should ask
   for comments that explain *why* or an invariant; change history, repair
   narration, and review discussion belong in the change report. This
   sub-rule is advisory and **MUST NOT** block delivery by itself.

Blocking findings are part of the **deterministic floor** (not an advisory
critic): each is folded into the step's acceptance as a typed, file-naming
rework directive, driving a **bounded** rework round under the host's
existing fix-round / stall counters — never an open-ended loop. The gate is
**fail-open**: an unreadable or absent architecture document, a document
with no layering declaration, an empty or unresolvable import-edge set, an
unreadable tree, and an oversized repository (a blown scan budget) all
degrade to a silent skip — the gate never fabricates a block and never
errors. The sub-rule identifiers `UD-CODE-006a/b/c/d` label individual
findings; the clause carried in the machine-readable `CLAUSES` data is the
parent `UD-CODE-006`. (The `-005` slot in this family remains reserved for
the §10 accessibility candidate.)

### 3.7 Design-system conformance floor (`UD-CODE-007`)

> Level: **MUST** (the advisory tier of sub-rule `UD-CODE-007e` is advisory)

The injected firmware *preaches* design tokens, paired foregrounds, measured
contrast, one committed hue, and one icon system — but a prompt is not a
floor. The pre-existing check (`design-tokens.{json,css} exists`) passed on
`:root{--color-bg:#000}`, which proves a file was written, not that a design
SYSTEM exists and not that the UI uses it. A conformant host **MUST** verify
design-system conformance on its own deterministic floor, with six sub-rules:

1. **Token schema floor** (`UD-CODE-007a`, blocking) — the token file **MUST**
   declare at least **6** color roles, each with a PAIRED `on-<role>`
   foreground; a type scale of at least **4** steps whose every adjacent ratio
   is at least **1.125**; a spacing scale of at least 4 steps, every step a
   multiple of **4** (the 4pt grid); a radius scale; and at least **2** motion
   durations plus at least **1** easing curve.
2. **Contrast** (`UD-CODE-007b`, blocking) — every DECLARED
   `(surface, on-surface)` pair **MUST** be MEASURED with the WCAG relative-
   luminance formula and reach **4.5:1** (body) or **3:1** (large text / UI
   chrome roles). The computation is pure arithmetic over parsed color values
   (`#hex`, `rgb()`, `oklch()`); no browser and no external dependency is
   required. A failing pair names both tokens and the measured ratio.
   A translucent surface composites over an unknown backdrop and is skipped
   rather than measured against a fiction.
3. **Token drift** (`UD-CODE-007c`, blocking) — UI source **MUST** draw its
   colors, font families, radii, and font sizes FROM the token set. A literal
   not present in the set is a blocking finding, subject to tolerances
   (color ±6 per channel, radius ±0.5px, size ±0.5px) so a legitimate derived
   value is never mistaken for drift. A `var(...)` reference never drifts.
4. **Banned brand hue** (`UD-CODE-007d`, blocking) — a declared primary /
   accent **MUST NOT** fall in the AI indigo/violet band (OKLCH hue **270–320**
   at chroma ≥ **0.09** and lightness **0.35–0.85**) unless the requirement text
   explicitly asks for purple. The band is stated perceptually, not as a hex
   list, so a near-neighbour of a canonical tell is caught while genuine blues
   (which sit below hue 270 in OKLCH) are deliberately spared.
5. **Design-lint registry** (`UD-CODE-007e`) — a registry of deterministic,
   numerically-thresholded source lints, each declaring `{id, severity,
   register_scope, tell, positive_redirect}`. A small P0 tier is **blocking**;
   the rest are **advisory** and fold into the rework directive. Every message
   **MUST** carry BOTH the observable tell and the positive target — a bare
   prohibition tells the brain what not to type and leaves it to invent the
   replacement.
6. **Visual direction** (`UD-CODE-007f`, blocking) — when a UI/UX document
   exists it **MUST** carry a `## Visual direction` section, and the plan
   **MUST** schedule the step that produces it BEFORE the design-tokens step.
   The section **MUST** state: a one-line design read (page kind / audience /
   register / vibe / aesthetic family); three forced decisions — a color
   commitment level (`restrained` | `committed` | `full-palette` | `drenched`),
   the light-vs-dark theme decided by a PHYSICAL-SCENE sentence (who uses this,
   where, under what ambient light, in what mood), and 2–3 NAMED anchor
   references each bound to a specific dimension (density from one, type from
   another, whitespace from a third — bare adjectives such as "modern" or
   "clean" are rejected); and anti-goals.

**The REGISTER** is normative to this clause. Every visual rule belongs to a
register: **`brand`** (landing / marketing / campaign / portfolio — design IS
the product) or **`product`** (app / dashboard / admin / settings / devtool —
design SERVES the task). The rules are not merely different, they are
**opposite**: on a product surface a familiar neutral system font is CORRECT,
the type scale is a fixed **1.125–1.2** rem ratio, there is NO page-load
choreography, restrained color is the floor, and density is a virtue —
while a brand surface demands a distinctive display face, a dramatic type
jump, and one orchestrated entrance. A host **MUST** scope its register-bound
rules (in the injected law and in the lint registry alike) to the declared
register. An **unknown** register **MUST** fall back to the host's full
historical rule set, never to a reduced one — an unclassifiable turn is never
under-governed.

Blocking findings are part of the **deterministic floor** (not an advisory
critic): each folds into the step's acceptance as a typed, token-naming rework
directive under the host's existing bounded fix-round / stall counters. The
gate is **fail-open**: no token file, an unparseable token file, an unreadable
tree, an unknown color syntax, and a missing UI/UX document all degrade to a
silent skip. A project that never asked for a design system is entirely
unaffected — only a project that SHIPPED one is held to the contract it
implicitly claimed. The strengthened acceptance
(`AcceptanceSpec::DesignTokensConform`) composes with, and does not remove,
the existence check (`DesignTokensPresent`).

## 4. Layer 2 — Flow contract

This layer governs **the order, gates, and continuity** of the
development process inside a conformant host.

### 4.1 Phase chain (`UD-FLOW-001`)

> Level: **MUST**

The conformant host **MUST** model the development pipeline as the
following ordered chain:

```
research → docs → docs_confirm → spec → frontend → preview_confirm
        → backend → quality → delivery
```

Each phase identifier is normative. A host MAY add sub-phases but MUST
NOT reorder, skip, or rename the listed phases without declaring a spec
profile (§8.4).

The host **MUST** persist the active phase to `.umadev/workflow-state.json`
in a format containing at least the keys `phase` and `active_gate`.

**Scope of this clause.** `UD-FLOW-001` governs the *full commercial
delivery build* — the `standard` profile of §8.4. It is the contract for
**how a heavyweight greenfield product is delivered**: when a host commits
to that play, it MUST run these phases in this order and honor the gates.
It does **not** require that *every* user turn be a nine-phase walk. A
casual question, a read-only review, a one-line edit, or a bugfix is not a
commercial delivery build and is **out of this clause's scope** (see the
team-scaling rule in §9.4 and the lightweight `seeai` profile in §8.4). The
reference implementation reaches the phase chain by **routing** a turn to
the deliberate path and synthesizing a plan whose deepest form *is* this
chain (§9.5) — the chain is the deep play the coordinator routes the team
into for a full build, not a funnel every message is forced through.

### 4.2 Docs confirmation gate (`UD-FLOW-002`)

> Level: **MUST**

After the `docs` phase produces all artifacts required by §5.2, the
conformant host **MUST**:

1. Pause the pipeline.
2. Set `workflow-state.json#active_gate` to `"docs_confirm"`.
3. Refuse any spec-writing or code-writing tool call until the user
   submits an explicit approval prompt.

Approval prompts that count as confirmation include exact matches of:
`确认`, `通过`, `继续`, `approved`, `approve`, `lgtm`, `ship it`. The
host **MAY** extend this set but **MUST NOT** infer approval from
unrelated user input.

On an interactive surface, the gate **MUST NOT** become answerable while
the writer invocation that produced it is still running. If the host
observes `GateOpened` before that writer has reported its terminal pause
and released writer ownership, it **MUST** stage the event and expose the
approval/revision controls only after the writer boundary is complete.
This prevents an approval from racing the final writer events.

### 4.3 Preview confirmation gate (`UD-FLOW-003`)

> Level: **MUST**

After the `frontend` phase produces a runnable preview, the conformant
host **MUST** apply the same gate semantics as §4.2 with
`active_gate = "preview_confirm"`. The host **MUST NOT** begin the
`backend` phase before user approval.

### 4.4 Gate-local revisions (`UD-FLOW-004`)

> Level: **MUST**

While `active_gate` is non-empty, user input **MUST** remain scoped to
that gate unless it is explicitly deferred as later work. Replies that
request revisions (`修改`, `补充`, `继续改`, free-form edit requests)
**MUST**:

1. Keep the pipeline in the same phase.
2. Update the affected artifact in place.
3. Re-stage the gate (the host MUST wait for explicit approval again).

A conformant host **MUST NOT** silently exit UmaDev mode in response
to revision requests.

A question asked at an open gate **MUST NOT** be interpreted as an
approval or revision. An interactive reference host answers it through a
fresh, independently permissioned read-only query, leaves the gate open,
and neither resumes nor advances the writer. An unrelated future task
**MUST** be deferred and routed as a new turn after the current run
settles; it does not inherit the gate's authority.

### 4.5 Phase-local artifact mutability (`UD-FLOW-005`)

> Level: **MUST**

Within a phase, the host **MUST** be able to revise the artifacts
produced by earlier phases (e.g. update `output/*-architecture.md` from
inside `backend`) as long as the active gate is re-staged afterward.

### 4.6 Session continuity (`UD-FLOW-006`)

> Level: **MUST**

On every model prompt, the conformant host **MUST**:

1. Read `.umadev/SESSION_BRIEF.md` if it exists.
2. Read `.umadev/workflow-state.json` if it exists.
3. Prepend a digest of these into the model's context (mechanism is
   host-defined: a system reminder, a hidden prefix, a tool result, etc.).

The host **MUST NOT** rely on the user to repeat workflow state between
turns.

### 4.7 Role-critic team (`UD-FLOW-007`)

> Level: **MUST**

A conformant host **MUST** model its quality judgement as a *team*, not a
single pass: a directing role drives the pipeline and one or more
**read-only role critics** cross-review the shared artifacts from their own
seat (e.g. product-manager, architect, qa-lead) and return structured
verdicts. The team layer is bound by four hard invariants:

1. **Deterministic floor is the gate.** A critic verdict is **advisory
   only**. Loop termination and gate progression **MUST** stay governed by
   the deterministic floor (coverage / contract / governance gap counts +
   stall counter). A non-deterministic critic opinion **MUST NOT** drive
   loop control. Critic findings MAY be fed back as advisory fixes.
2. **Read-only / single-writer.** A critic **MUST NOT** write files or
   mutate the workspace. It reviews artifacts on an isolated, fresh
   read-only child session that neither resumes nor branches the directing
   session's transcript; only the directing (main) session ever writes.
3. **No new endpoint.** A critic **MUST** run over the *same* borrowed base
   brain via the existing host-driver subprocess — no extra model endpoint
   and no extra API key.
4. **Fail-open.** A critic that errors, cannot be forked, or returns
   unparseable output **MUST** yield an empty verdict that accepts; an
   absent critic can never block the base.

### 4.8 Trust tiers + irreversible-action floor (`UD-FLOW-008`)

> Level: **MUST**

A conformant host **MUST** expose a progressive-trust ladder controlling
whether a run may start and, once permitted, how much autonomy it has at
the confirmation gates:

- `plan` — **read-only conversational planning**. It may inspect the
  workspace through a read-only base and return a plan in the conversation,
  but it does not open an execution pipeline or persist planning artifacts.
- `guarded` — the **default**; every gate pauses for explicit confirmation
  (the §4.2 / §4.3 human-in-the-loop behaviour).
- `auto` — fully autonomous; every gate auto-approves.

The selected trust tier is an execution ceiling, not a suggestion to the
model. In particular, a route verdict produced under `plan` **MUST** be
reconciled to a read-only execution surface before any writer is opened;
no model-supplied class, depth, team, or authorization value may widen
`plan` into a mutating run.

Every explicit execution entry under `plan` — including run/goal, quick,
continue/resume, revise/redo, and an equivalent programmatic runner call —
**MUST** settle as a typed non-execution result before acquiring a run lock,
creating or switching an isolation branch, writing workflow/governance state
or artifacts, or opening/driving a writer session. A reference `Result` API
uses `PermissionDenied`; a legacy outcome enum MAY carry an equivalent hard
non-execution variant. It **MUST NOT** report `Completed`, `Done`, or otherwise
present the refusal as successful delivery. The user switches to `guarded` or
`auto` and re-enters the request to execute it.

Riding on top of the ladder is a **hard reversibility floor**, and the floor
is **tier-aware on exactly one axis**:

- The **true-disaster classes** — a destructive shell verb, version-control
  internals / history rewrite (including a **force-push**), an obfuscated
  payload the classifier cannot vet, a **credential-exfiltrating** network
  command, a **publish-outward** network action (a **push**, opening a
  **PR**, a package **publish**, a **deploy**), and a write that **escapes
  the workspace** — **MUST** be escalated to an explicit confirmation
  **regardless of mode**. `auto` does **not** get to skip these; the only
  non-interactive bypass is an explicit `--yes`.
- The **ordinary inbound network reach** — a dependency install
  (`npm install`, `pip install`, `cargo install`, …), a plain
  fetch / clone / registry read — **MUST** be confirmed under `guarded` /
  `plan`, and **MUST run without confirmation under `auto`**: installing
  dependencies is normal development work, not an irreversible disaster,
  and the `auto` tier is the user's explicit full-trust opt-in. Under
  `auto` the host **SHOULD** also grant the base its least-restrictive
  native permission posture (the base itself never interrupts the run),
  while the host's own governance hooks and audit trail remain active on
  every tool call.

An interactive host **MUST** surface any residual floor escalation as a
visible, answerable prompt (approvable by key **and** by typed text) rather
than a silent headless deny while a live user is present. The classifier is
a pure, deterministic function of the action — it introduces no new model
endpoint and no randomness.

## 5. Layer 3 — Delivery artifacts

This layer governs **what files MUST land on disk** at each phase. Files
are normative; the format inside each file is illustrative.

### 5.1 Research artifacts (`UD-ART-001`)

> Level: **MUST**

The `research` phase **MUST** produce, in the workspace `output/`
directory, all of:

| Path | Content |
|---|---|
| `output/<slug>-research.md` | Similar-product research, citing sources |
| `output/knowledge-cache/<slug>-knowledge-bundle.json` | Local knowledge hits + research summary |

`<slug>` is the project identifier; if absent, the host **MUST** derive
one from the workspace name.

### 5.2 Core documents (`UD-ART-002`)

> Level: **MUST**

The `docs` phase **MUST** produce all of:

| Path | Required sections |
|---|---|
| `output/<slug>-prd.md` | Goal, scope, user stories, acceptance criteria |
| `output/<slug>-architecture.md` | System diagram, API surface, data model, tech stack rationale |
| `output/<slug>-uiux.md` | Design tokens, component skeleton, page hierarchy, accessibility notes |

The host **MUST NOT** advance to `docs_confirm` until all three files
exist and are non-empty.

### 5.3 Spec + tasks (`UD-ART-003`)

> Level: **MUST**

The `spec` phase **MUST** produce:

| Path | Content |
|---|---|
| `output/<slug>-execution-plan.md` | Spec narrative, task breakdown |
| `.umadev/changes/<change-id>/tasks.md` | Machine-trackable task list |

### 5.4 ADR records (`UD-ART-004`)

> Level: **SHOULD**

For every non-trivial architectural decision, the host **SHOULD** create
an ADR file under `.umadev/decisions/ADR-<id>-<slug>.md` containing:
context, decision, alternatives, consequences.

### 5.5 Mutability of artifacts (`UD-ART-005`)

> Level: **MUST**

Artifacts in §5.1–§5.4 **MUST** be re-writable by the host throughout
the pipeline (e.g. `output/*-uiux.md` is updated from inside `frontend`
when the user requests a redesign). The host **MUST NOT** treat any
artifact as read-only once produced.

### 5.6 No chat-only completion (`UD-ART-006`)

> Level: **MUST**

A conformant host **MUST NOT** declare a phase complete based on chat
output alone. The required artifact file(s) **MUST** exist in the
workspace before the host advances.

### 5.7 PR artifact (`UD-ART-007`)

> Level: **SHOULD**

When a finished run is handed off as a pull request, the conformant host
**SHOULD** render the PR body from the run's **own evidence**, not a bare
diff: the PR-ready review report (§6.8) followed by a proof-pack (§6.5)
summary. The assembled body **SHOULD** be persisted to
`output/<slug>-pr-body.md` so the same artifact backs both the dry-run
preview and the opened PR. Opening the PR is a network/VCS action governed
by the irreversibility floor (§4.8): the host **MUST NOT** push or open a
PR without confirmation, **MUST NOT** commit on the default branch (it
creates a feature branch first), and **MUST** degrade fail-open to printed
manual steps when `git` / `gh` is missing or not logged in.

## 6. Layer 4 — Evidence chain

This layer governs **what audit-grade evidence MUST be produced** so the
result of an AI-driven development run is verifiable by reviewers,
auditors, and compliance officers.

### 6.1 API audit log (`UD-EVID-001`)

> Level: **MUST** at L3

Every API path extracted under §3.3 **MUST** be appended to
`.umadev/audit/frontend-api-calls.jsonl` as one JSON object per line
containing at least:

```json
{
  "ts": <integer unix seconds>,
  "file": "<workspace-relative path>",
  "tool": "<host tool name, e.g. Write|Edit>",
  "urls": ["<extracted url>", ...],
  "session_id": "<opaque host session id>"
}
```

The host **MUST NOT** rewrite or truncate prior entries. The log is
append-only.

### 6.2 Tool-call audit log (`UD-EVID-002`)

> Level: **SHOULD** at L3

For every Write/Edit/Patch tool call, the host **SHOULD** append to
`.umadev/audit/tool-calls.jsonl` a record with: timestamp, tool name,
target file, decision (allow/block/warn), and the governance rule that
fired (clause ID, e.g. `UD-CODE-001`). This record is the primary input
for the compliance mapping (§6.4).

### 6.3 Quality report (`UD-EVID-003`)

> Level: **MUST** at L3

At the `quality` phase, the host **MUST** produce
`output/<slug>-quality-gate.json` containing at minimum:

```json
{
  "passed": <bool>,
  "total_score": <integer 0-100>,
  "weighted_score": <float 0-100>,
  "scenario": "<run scenario id>",
  "critical_failures": ["<string>", ...],
  "recommendations": ["<string>", ...],
  "summary": {
    "executive_summary": "<one-line headline>",
    "summary_context": {"<key>": "<value>", ...}
  },
  "checks": [
    {
      "name": "<check id>",
      "category": "<grouping>",
      "description": "<human readable>",
      "status": "passed|warning|failed",
      "score": <integer 0-100>,
      "weight": <float>,
      "details": "<freeform>"
    }
  ]
}
```

A run that emits `passed: false` **MUST** cause the host to refuse
advancing to `delivery`. The pass threshold defaults to `90`; projects
MAY override via `umadev.yaml#quality_gate`. The host **MAY** wrap
the document with the `evidence_identity` envelope used by the
reference implementation (`umadev.cli_release_quality_mixin`) — that
wrapping is OPTIONAL and does not break conformance.

The companion human-readable report `output/<slug>-quality-gate.md` is
RECOMMENDED but not required.

### 6.4 Compliance mapping (`UD-EVID-004`)

> Level: **SHOULD** at L3

At the `delivery` phase, the host **SHOULD** emit
`output/<slug>-compliance-mapping.json` linking each UmaDev clause
that fired during the run to its mapping into external compliance
frameworks. At minimum:

| External framework | Mapping field |
|---|---|
| SOC 2 (2017 TSC) | `soc2_cc` (e.g. `CC9.2`) |
| ISO/IEC 27001:2022 | `iso27001_annex_a` (e.g. `A.14.2.1`) |
| EU AI Act (2024/...) | `eu_ai_act_article` (e.g. `Article 15`) |

Recommended top-level shape:

```json
{
  "spec_version": "UMADEV_HOST_SPEC_V1",
  "slug": "<project slug>",
  "generated_at": "<ISO8601>",
  "quality_gate_passed": <bool>,
  "clauses": [
    {
      "id": "UD-CODE-001",
      "fired_count": <integer>,
      "soc2_cc": ["CC9.2"],
      "iso27001_annex_a": ["A.14.2.1"],
      "eu_ai_act_article": ["Article 15"],
      "evidence": [".umadev/audit/tool-calls.jsonl"]
    }
  ]
}
```

The mapping is the foundation of UmaDev's compliance evidence pack.
The reference implementation lives in `umadev.governance.compliance`.

### 6.5 Proof pack (`UD-EVID-005`)

> Level: **MUST** at L3

At the `delivery` phase, the host **MUST** assemble a *proof pack*
archive containing every artifact named in §5 plus every evidence file
named in §6.1–§6.4. Recommended location: `release/proof-pack-<run-id>.zip`.

The proof pack is the **product output of a UmaDev–conformant run**.
A reviewer SHOULD be able to verify spec conformance solely by inspecting
the pack — no host session required.

### 6.6 Runtime evidence (`UD-EVID-006`)

> Level: **SHOULD** at L3

Beyond proving the code *was written*, a conformant host **SHOULD** prove
the app *actually runs*: boot the detected dev server, wait for it to
answer, probe the documented routes over HTTP, and persist the result to
`.umadev/audit/runtime-proof.json` (per-route status + the boot command),
which is then folded into the delivery proof pack (§6.5). This clause is
**fail-open**: a missing dev server, probe tool, or route table is recorded
as `"not verified"`, never an error.

### 6.7 Deploy evidence (`UD-EVID-007`)

> Level: **SHOULD** at L3

When the post-delivery handoff actually deploys, the conformant host
**SHOULD** capture the deploy outcome — the detected target, the executed
command, the resulting preview/live URL, and a log tail — into
`.umadev/audit/deploy-proof.json`, folded into the delivery proof pack
(§6.5). The deploy itself runs through the user's own logged-in platform
CLI (UmaDev owns no credentials and injects nothing) and is gated by the
irreversibility floor (§4.8). **Fail-open**: an unknown platform, missing
CLI, or failed deploy is recorded as `"not deployed"` with a manual hint,
never an error.

### 6.8 Review-report evidence (`UD-EVID-008`)

> Level: **SHOULD** at L3

A conformant host **SHOULD** assemble the run's checks into a single
PR-ready review report at `output/<slug>-review-report.md` — contract
alignment (§3.3 / §6.1), acceptance, FR→task coverage, governance
(§6.2), the pre-PR security scan, runtime evidence (§6.6), and the
rollback/checkpoint story — each section degrading to an honest "not
available" line rather than failing. This report is the human-readable
case-for-merge that backs the PR artifact (§5.7).

## 7. Host surface mapping

Every clause in §3–§6 is layer-agnostic. This section is **non-normative
guidance** showing how each layer is realized on UmaDev's
officially-supported base CLIs. Other hosts MAY implement this spec
independently. Base execution and integration installation are separate:
`umadev install --base ...` installs an UmaDev governance hook or repository
pre-commit integration; it does **not** install, update, authenticate, or
license a vendor CLI.

### 7.1 Officially supported hosts

The reference implementation drives exactly **five** first-class base CLIs —
`claude-code`, `codex`, `opencode`, `grok-build`, and `kimi-code` — each an
already-authenticated host CLI run as a
subprocess (`umadev_host::BACKEND_IDS` is the authoritative id list). UmaDev
does not vendor or call a provider Agent SDK and does not own a model endpoint;
wider provider/model coverage is configured inside one of these bases.

| Base CLI id | Executable/session surface | Workspace guidance | User-scope configuration |
|---|---|---|---|
| `claude-code` | logged-in `claude` subprocess | `CLAUDE.md` + `.claude/` | `~/.claude/` |
| `codex` | logged-in `codex app-server` subprocess | `AGENTS.md` + `.codex/` | `~/.codex/` |
| `opencode` | logged-in `opencode serve` subprocess | `AGENTS.md` + `opencode.json` / `.opencode/` | OpenCode's own user configuration |
| `grok-build` | `grok agent … stdio` subprocess | `AGENTS.md` + negotiated ACP session rules | Grok CLI's own user configuration |
| `kimi-code` | `kimi acp` subprocess | `AGENTS.md` + first-directive firmware prefix | `$KIMI_CODE_HOME` or `~/.kimi-code/` |

Grok Build and Kimi Code use UmaDev's bounded JSON-RPC/stdio transport and ACP
v1 lifecycle with vendor-isolated policies and without fabricated capabilities.
Every ACP session performs a live
`initialize` handshake and gates load/resume, models, modes, structured
questions, permissions, plans, tools, and vendor extensions on the capabilities
actually returned by that installed version.
A binary-name or `--version` match alone is insufficient: identity, protocol,
and minimum-version checks must pass before the base is reported healthy.
Vendor-specific extensions remain in thin per-base dialects.

Authentication, permission, resume, and OS support are deliberately reported
per vendor rather than inferred from ACP:

| Base | Login/configuration owned by | Plan boundary in the reference driver | Resume | Vendor CLI OS boundary |
|---|---|---|---|---|
| Claude Code | Claude Code login/setup | native `permission-mode=plan` | exact native session id | the installed Claude Code CLI's supported platforms |
| Codex | `codex login` / Codex credential config | native `sandbox=read-only` | `thread/resume` | the installed Codex CLI's supported platforms; restrictive Windows sandboxes may block network/local ports |
| OpenCode | `opencode auth login` / provider config | deny-by-default Plan rules; version >= 1.14.31 | persisted session reattach + permission refresh | the installed OpenCode CLI's supported platforms |
| Grok Build | headless ACP reuses a cached login token or `XAI_API_KEY`; after `initialize`, the driver explicitly authenticates only with an available non-interactive method and never auto-selects OAuth or opens a browser | plan permission mode + read-only sandbox/tool set + no subagents | fresh-session handoff until both effective-sandbox attestation and native pre-start resume validation are provable; capability negotiation alone is insufficient | macOS/Linux/WSL and native Windows PowerShell installer |
| Kimi Code | exact source-audited `Kimi Code CLI` identity/version; ACP `authenticate(login)` only revalidates Kimi's on-disk token; UmaDev never runs `kimi login`, `kimi acp --login`, or opens a browser | Plan=`plan`; Guarded/Auto=`default`; UmaDev remains the authority for ordinary Auto decisions and the irreversible-action floor, so Kimi `auto`/`yolo` are never selected | standard `session/resume` when advertised, otherwise `session/load`; identity/profile/workspace mismatches fail closed | macOS/Linux/Windows; Windows tool execution requires Git Bash or an explicit `KIMI_SHELL_PATH` |

The audited Kimi adapter reuses `session/request_permission` for
`AskUserQuestion`, distinguished by its exact `AskUserQuestion` tool title and
`q0_opt_*` / `q0_skip` option namespace. A conforming host MUST surface that
shape as structured user input and echo the selected option id; it MUST NOT
coerce the question into a binary tool approval. The host MUST classify the
source-specific question/plan-review surface before validating its complete
option shape; a malformed `AskUserQuestion` or `plan_*` request MUST fail
closed on that human-input path and MUST NOT fall through to ordinary tool
approval. Ordinary Kimi permission
requests remain mediated by the UmaDev trust profile, so Auto resolves
reversible in-workspace work without becoming more interruptive than Guarded.
For the exact source-audited release, the ordinary surface MUST match the
ordered `approve_once`, `approve_always`, and `reject` source contract before
the host grants authority; widened, reordered, or relabeled options MUST NOT be
guessed.
Kimi `configOptions` updates are complete replacements: the reference host MUST
clear a thinking control omitted after a model change, preserve a one-option
locked-on control, and confirm any requested thinking change from the returned
snapshot. It MUST reject a generic binary file before `session/prompt`, because
the audited adapter explicitly drops ACP blob embedded resources; UTF-8 text
resources and separately negotiated image blocks remain native.
The adapter's title-only `tool_call_update` is non-terminal progress, not tool
output. The reference host MUST retain its stable `toolCallId` across the
presentation boundary and replace only that call's live card title; correlated
output and terminal results MUST NOT be attached by trailing-row/FIFO guesses
when calls interleave. Kimi carries final tool output on the terminal update,
so the reference host MUST retain a bounded process-log-sized result rather
than reducing it to a short diagnostic before the presentation layer can fold
or expand it.
Kimi's session-scoped `available_commands_update` is a complete replacement.
The reference host MUST surface non-conflicting native and `/skill:*` commands,
preserve their arguments exactly through the resident session, and provide an
explicit native escape for names shadowed by product commands. Skill discovery
and activation remain Kimi-owned; the host MUST NOT infer a catalog by scanning
or rewriting `.kimi-code/skills` or `.agents/skills` itself.
Kimi's advertised `session/list` enumerates vendor-owned on-disk history and MAY
filter it by `cwd`; it is not authority evidence. The reference host's
`/sessions` command MUST list only UmaDev-owned project chats whose base session
id, workspace, base identity, profile and effective authority were persisted as
one record. It MUST NOT silently import or resume an unattested vendor-global
session merely because `session/list` returned it.

ACP treats `sessionCapabilities.resume` and the legacy top-level `loadSession`
as separate optional capabilities. The former reconnects session context without
history replay; the latter restores and streams history. The reference driver
MUST NOT call either method unless its corresponding capability was advertised,
the effective sandbox matches the saved authority identity, and any required
vendor-native pre-start resume validation has succeeded.
When `sessionCapabilities.close` is advertised, the reference driver sends the
stable ACP `session/close` request before reaping its subprocess. An unadvertised,
rejected, or non-responsive close never blocks shutdown and is never inferred.

The UmaDev binary itself ships for macOS Intel/Apple Silicon, Linux
x86_64/ARM64 (glibc 2.31 floor), and Windows x86_64; Windows on ARM uses OS x64
emulation. This does not broaden a vendor's own CLI support: the selected base
must be runnable in the same environment. Protocol and flag compatibility are
also not provenance evidence; the reference implementation makes no derivation
claim based only on protocol or flag compatibility.

`umadev_spec::RuntimeKind` is a backward-compatible, coarse wire-family tag;
it is not a supported-host enum or an SDK/provider declaration. In particular,
OpenCode retains the historical `RuntimeKind::Openai` compatibility value.
Host-specific behaviour MUST use the base id and declared capabilities instead.

Hosts outside these five bases are explicitly **out of scope**
for the reference implementation. They MAY still implement this specification
by adopting the same `.umadev/` workspace layout and the same hook commands;
the reference integration bundles will not be shipped for them.

### 7.2 Reference surface inventory

Claude Code and the exact source-audited Kimi Code release expose native
PreToolUse/PostToolUse lifecycle hooks used by the reference injector. Claude's
rows live in project settings. Kimi's registry is user-level, so every
UmaDev-installed command carries an absolute project scope and immediately
fails open outside that root; merge/install/uninstall preserve unrelated
hooks and configuration. The reference implementation does not invent this
surface for the other three bases. There the same policy is delivered through
the strongest verified combination exposed by that base: typed permission
callbacks where available, runner-side tool-event governance and audit,
workspace guidance, and a per-turn firmware prefix. If an installed version
cannot expose a pre-apply boundary, §7.4 applies and the capability report
must say so honestly.

| Base | Session wire | Firmware / context surface | Governance and audit surface |
|---|---|---|---|
| Claude Code | bidirectional `stream-json` | native append-system-prompt + `CLAUDE.md` | native pre/post tool hooks + streamed tool events |
| Codex CLI | `app-server` JSON-RPC | first-directive prefix + `AGENTS.md` | typed approvals/tool events + runner audit |
| OpenCode | HTTP + SSE | first-directive prefix + `AGENTS.md` | SSE tool events + runner audit |
| Grok Build | ACP v1 + `x.ai` extensions | ACP session rules + prompt context | ACP permissions/tool/subagent updates + recursive secret redaction |
| Kimi Code | ACP v1 | first-directive prefix + `AGENTS.md` | source-audited, root-scoped native Pre/PostToolUse hooks + standard ACP permission/tool/plan updates + recursive secret redaction; native child-agent events are not claimed because the audited adapter filters them |

Every base writes artifacts to workspace `output/` and evidence to
`.umadev/audit/`. Structured permission or user-input requests are correlated
by request id; parallel tool calls are correlated by tool-call id. Unknown
requests are never auto-approved, and raw protocol frames containing headers,
environment values, tokens, keys, or secrets are never logged.

### 7.3 Reference clause → hook command

All host families invoke the **same** `umadev hook <subcommand>`
binary; only the host-specific config syntax differs. The binary itself
implements every governance clause once.

| Clause | Pipeline event | Hook subcommand |
|---|---|---|
| `UD-CODE-001` (emoji) | Pre-write | `umadev hook check-emoji` |
| `UD-CODE-002` (color) | Pre-write | `umadev hook check-color` |
| `UD-CODE-003` + `UD-EVID-001` (API audit) | Post-write | `umadev hook audit-api` |
| `UD-EVID-002` (tool-call audit) | Post-write | `umadev hook tool-audit` |
| `UD-FLOW-006` (session continuity) | Prompt-time | `umadev hook inject-context` |

### 7.4 Surface-not-available degradation

Where a host lacks a surface required by a clause (e.g. a host whose
hook contract is not yet GA), the conformant injector MUST substitute
the next-best surface — for example, the bundled
`AGENTS.md` instructs the host model to invoke the equivalent
`umadev hook …` commands manually before committing UI source. The
clause's *effect* is preserved; only the delivery pipe is host-specific.
Conformance level MAY drop from L2 to L1 on the affected clause if
substitution is not viable.

## 8. Versioning and conformance declaration

### 8.1 Spec manifest (`UD-META-001`)

> Level: **MUST**

A conformant host workspace **MUST** contain a top-level marker that
declares its spec conformance level. The canonical marker is in
`umadev.yaml`. The `spec:` block is **normative** — every key in it
is required:

```yaml
spec:
  version: UMADEV_HOST_SPEC_V1
  level: L3
  profile: standard   # or "seeai" for competition mode
  declared_by: umadev@1.0.0
```

A host MAY append **non-normative** blocks that its own tooling reads;
these MUST NOT affect conformance judgement. The reference
implementation appends two:

```yaml
project:
  slug: <project-slug>      # used in artifact filenames
quality_gate:
  threshold: 90             # UD-EVID-003 pass threshold
```

A conformant verifier **MUST** ignore blocks it does not recognise and
**MUST NOT** fail conformance because of them.

### 8.2 Version negotiation (`UD-META-002`)

> Level: **MUST**

When a host attaches to a workspace whose declared spec version is
higher than the host supports, the host **MUST** refuse to proceed and
emit `compliance:version-mismatch`. It **MUST NOT** silently downgrade
the workspace.

### 8.3 Backward compatibility (`UD-META-003`)

> Level: **MUST**

Within a major version (V1 → V1.x), removing or strengthening a clause
**MUST NOT** happen. Clauses MAY be added with `SHOULD` or `MAY` levels
and promoted to `MUST` only at the next major version.

### 8.4 Profiles (`UD-META-004`)

> Level: **MAY**

A host MAY declare a **profile** that adjusts non-MUST clauses. Two
profiles are reserved:

- `standard` — the full pipeline (this document)
- `seeai` — competition / time-boxed delivery; alternate phase chain
  `research → docs → docs_confirm → spec → build_fullstack → polish → handoff`

Profiles **MUST NOT** weaken any MUST clause in §3–§6.

## 9. Reference implementation

The UmaDev repository at <https://github.com/umacloud/umadev>
ships a reference injector + orchestrator + verifier for this
specification as a **single pure-Rust binary** (`umadev`). The
workspace is twelve crates:

| Crate | Role |
|---|---|
| `umadev` | The binary — clap CLI + the `tui` subcommand |
| `umadev-spec` | This specification as Rust data (clauses, phases, gates) |
| `umadev-governance` | Every enforceable rule in §3 / §6 — fail-open |
| `umadev-agent` | The team engine — intent router + owned plan DAG + a coordinator seat that schedules the role team (PM, architect, designer, frontend, backend, QA, security, DevOps) step by step + firmware injection, with the full commercial phase chain as its deepest play; gate semantics, role-critic team, trust tiers, runtime/deploy/review evidence |
| `umadev-runtime` | Runtime trait + OfflineRuntime + RuntimeKind (the host drivers impl Runtime; UmaDev owns no HTTP/model endpoint) |
| `umadev-host` | Drives the five supported, already-authenticated base CLIs as subprocesses through vendor-specific and isolated ACP protocol drivers |
| `umadev-process` | Cross-platform process-tree lifecycle primitives, including Windows Job Object ownership |
| `umadev-knowledge` | Structured BM25 + CJK retrieval over the curated `knowledge/` corpus |
| `umadev-contract` | Machine-verifiable frontend↔backend API contract (UD-CODE-003) |
| `umadev-tui` | A ratatui terminal app over the engine event stream |
| `umadev-i18n` | Trilingual (zh-CN / zh-TW / en) string catalogs + locale detection |
| `umadev-state` | Shared safe-persistence primitives and the user-controlled leaf-store capture/recall/retention policy schema |

### 9.1 Execution modes

The reference implementation drives a turn through the team runtime of §9.5
— a coordinator seat routing it, and for a full commercial build expanding the
plan into the §4 phase chain — with one of two runtime implementations. The
choice does **not** weaken which clauses are evaluated, but Offline produces
deterministic placeholders and cannot claim that generative coding or a
commercial delivery occurred:

| Mode | Selector | Needs an API key |
|---|---|---|
| **Base CLI** | `--backend <one of umadev_host::BACKEND_IDS>` | No UmaDev API key — drives the user's already-authenticated base CLI, reusing its own model + reasoning effort (UmaDev imposes neither) |
| **Offline** | internal `OfflineRuntime` / diagnostic no-base fallback | No — deterministic templates, not real coding |

UmaDev owns no model endpoint and connects no third-party API itself: a base
that the user has pointed at a third-party / local model simply runs with that
model. Where the installed base exposes documented configuration or session
metadata, UmaDev reads and displays the model and reasoning effort without
overriding either. Missing data remains unknown; the reference driver does not
guess a context window or model identity from a name table.

### 9.2 Governance hook entry

All four enforceable layers converge on one command surface —
`umadev hook <name>` — invoked by the host's pre/post-write hooks:
`check-emoji` (`UD-CODE-001`), `check-color` (`UD-CODE-002`),
`audit-api` (`UD-CODE-003` + `UD-EVID-001`), `tool-audit`
(`UD-EVID-002`), `inject-context` (`UD-FLOW-006`).

The reference implementation is one realization. Hosts MAY implement
this spec independently; conformance is judged by the spec, not by use
of the reference code.

### 9.3 Continuous-session driving model

The reference implementation exposes one **logical continuous writer
session** for a run rather than intentionally starting cold on every phase.
The physical subprocess may be resumed, reattached, or safely recreated when
the vendor capability and failure path require it; exact transcript recovery
is claimed only where the base proves it. This is a property of the *reference driver*, not a
new normative clause: every clause in §3–§6 fires identically whether the
phases are driven over a persistent session or over per-phase single
shots. The phase chain (`UD-FLOW-001`), the confirmation gates
(`UD-FLOW-002` / `UD-FLOW-003`), session continuity (`UD-FLOW-006`), and
the evidence chain (§6) are all defined on the *order of phases and the
artifacts they leave on disk* — never on the wire mechanism underneath.

In the reference driver the model is this:

- **One logical writer session = the team's shared working context.** A run opens a
  single base session, hands it the `research` directive, and then drives
  the *same* session through `docs`, `spec`, `frontend`, `backend`,
  `quality`, and `delivery`. The base keeps the accumulated context across
  phases instead of being re-primed from cold nine times. The confirmation
  gates (`UD-FLOW-002` / `UD-FLOW-003`) are natural pause points: after a
  turn completes the driver does not send the next directive until the
  applicable trust/gate policy allows progress. When exact native resume is
  unavailable, bounded owned project state can be replayed, but this is not
  described as restoration of the vendor's full transcript.
- **The base does all cognition; the reference shell does only tooling.**
  Reasoning, research, design, writing code, and review are the base's
  work, observed as a stream of tool-call and text events. The shell layer
  is deterministic orchestration: advancing phases, staging gates,
  enforcing governance at the tool-call boundary, writing the audit chain
  (`UD-EVID-002`), applying the hard reversibility floor (`UD-FLOW-008`),
  and running the role-critic team (`UD-FLOW-007`). A run's *truth* is the
  set of tool calls and the files they leave on disk, not the prose the
  base narrates — the "no chat-only completion" rule (`UD-ART-006`) is
  enforced against the filesystem, and runtime evidence (`UD-EVID-006`)
  proves the result actually boots.
- **All intents share one logical conversation surface, with physical
  sessions separated by authority.** The configured base model classifies an
  ordinary natural-language turn before the writer acts. Intent triage runs
  on a fresh read-only child; a healthy child may also answer Chat/Explain,
  while a mutating turn is handed only to the single writer under the run
  lock. In the typed route contract, only the exact, legal authorization
  value `mutating` grants that hand-off; a missing, blank, unknown, or
  malformed authorization fails closed to a read-only route. `plan` mode is
  an independent ceiling and remains read-only even if the model proposes a
  mutating class. This fail-closed rule applies to the model verdict itself;
  the independent deterministic availability fallback may choose only an
  unmistakable, explicitly scoped current-user request on the resident lane
  and never treats a malformed model field as fallback authorization. A full
  pipeline still keeps one writable main session
  across its phases. This preserves conversational follow-ups without giving
  a read-only or malformed decision a write-capable execution surface.
- **Single-shot is a fail-open fallback.** A base-specific non-interactive
  one-shot path is retained where the base exposes one, only as a degradation
  route: when the continuous session cannot start, when the brain is the
  offline runtime, or when an operator explicitly opts out. A driver that
  cannot open a continuous session **MUST** return a bounded, honest
  degradation or actionable incompatibility result rather than wedge; it must
  never silently treat a protocol/authentication failure as successful work.
- **Auxiliary native controls remain capability- and session-scoped.** Queue,
  background-process, model, and mode controls are exposed only when the live
  base negotiated the corresponding operation. A destructive process stop
  requires a fresh server-authoritative list and exact live-session ownership;
  UmaDev never substitutes local PID killing or infers ownership from transcript
  text. Unsupported controls produce a visible typed result rather than a
  simulated success.

### 9.4 The team-of-roles collaboration model

The role-critic team required by `UD-FLOW-007` is realized through bounded
role contracts for product, architecture, UI/UX, frontend, backend, QA,
security, and DevOps over a shared blackboard. These seats are isolated
uses of the borrowed base brain, not independent people, and a proportional
route need not convene all of them. A **coordinator** schedules the selected
seats and owns the deterministic gate decision; a model persona does not
constitute human sign-off. The collaboration model obeys the four
hard invariants of `UD-FLOW-007` and is organized along two axes:

- **Doing roles vs. reviewing roles.** Doing roles (the frontend and
  backend engineers, the PM writing the spec) drive the *main* session
  **serially**, because writers share one workspace and cannot be
  parallelized without their implicit decisions colliding. Reviewing roles
  (PM, architect, designer, QA, security, backend, frontend, DevOps
  critics) each run on their **own fresh Plan-profile child session** and
  therefore review **in parallel**. The child is seeded from the blackboard
  artifacts and acceptance criteria, not the writer's transcript; critics
  are never scheduled as workspace writers. The strongest enforceable
  read-only boundary remains vendor-specific and must not be overstated.
- **Communication is a shared blackboard plus structured verdicts, never
  free-form chat.** Roles do not converse with each other (cross-talk
  amplifies hallucination and never converges). They exchange exactly two
  things: the shared blackboard — the artifact files of §5 and the source
  tree on disk — which doing roles write and reviewing roles read; and the
  structured verdict each reviewer returns to the director (overall accept
  plus a list of blocking findings, advisory notes, and concrete
  evidence). Returned verdicts are appended by the coordinator to the team
  ledger. An unavailable or empty advisory review is recorded/degraded; it
  does not manufacture positive evidence or override the deterministic floor.
- **The director aggregates deterministically and drives bounded rework.**
  At each gate the director collects the reviewers' verdicts together with
  the deterministic floor — FR→task coverage, the frontend↔backend contract
  check (`UD-CODE-003`), the governance scan, the verify/runtime result
  (`UD-EVID-006`), and the always-on hard gates. Loop control is a pure
  function of that deterministic floor: a non-deterministic critic opinion
  is advisory and **MUST NOT** drive termination (`UD-FLOW-007`). Blocking
  findings are folded into a single rework directive injected back into the
  main session, the affected artifacts are revised in place
  (`UD-ART-005` / `UD-FLOW-004`), and the gate is re-staged. Rework is
  **bounded**: a gap counter plus a stall counter terminate the loop
  deterministically rather than asking the base whether the result is "good
  enough." Exhausting that budget is not success: a blocked, active, pending,
  incomplete, or dirty-QC plan settles as `Failed` with bounded blocking
  evidence. Only a mechanically clean terminal plan may settle as `Done`.
- **The team scales with task complexity.** Trivial work (a fast, narrowly
  scoped bugfix or refactor) convenes no team — the deterministic floor stands
  alone. A sufficiently deep greenfield build may convene the full roster. This keeps the cost of the
  cross-review proportional to the risk of the change, and matches the
  lightweight path allowed for simple requirements (the spec profile of
  §8.4 governs the time-boxed `seeai` variant).

### 9.5 Team turn model — route, plan, schedule, deliver (coordinator-scheduled)

This section describes the **canonical shape of the reference
implementation's runtime**. It is **non-normative**: it adds no clause and
changes no clause. It documents *how* the reference driver decides, for any
given turn, whether and how deeply to engage the §4 flow contract — and it
reconciles the everyday product behaviour with the phase chain of
`UD-FLOW-001`.

The mental model is **firmware over a borrowed brain**. UmaDev owns no
model and does not re-implement the base's agentic loop. It borrows the
base brain to **think** (route, plan, judge) and directs the base body to
**work** (write code, run, fix), driving the base deterministically against
**typed artifacts UmaDev owns**. Advisory consults have bounded safe fallback;
authentication, transport, authorization, hard-gate, and verification errors
remain explicit failures/degradation rather than synthetic success. A turn flows through up to five layers:

- **L0 — Firmware injection (every path).** Before any base turn, the
  reference driver supplies the stable identity/language firmware, then
  adds a route-proportional, token-budgeted overlay. Chat stays identity-only;
  Explain gets bounded read context; QuickEdit/Debug receive engineering
  craft; a deliberate Build adds repo-map, just-in-time knowledge, learned
  pitfalls, and the selected team doctrine. The base executes; UmaDev
  declares policy. Governance (§3 / §6) remains the silent floor under it.
- **L1 — Intent router.** The coordinator seat classifies the turn into a
  typed route — its class (chat / explain / quick-edit / debug / build), its
  task kind, depth, write authorization, scope, confidence, clarification,
  and team — by asking the configured base model on a fresh read-only child
  **before** any writer turn. A valid model decision is authoritative in both
  directions: it may recognize that keyword-heavy text is only a question,
  or that terse text is a real build. Deterministic classification is the
  conservative availability fallback, not a competing semantic authority.
  Explicit read-only wording, `plan` mode, the single-writer rule, and hard
  safety/reversibility floors remain ceilings that the model cannot widen.
  A write-capable route additionally requires the exact legal typed
  authorization `mutating`; missing, blank, or invalid authorization is
  reconciled fail-closed to read-only `Explain`, with no writer or team. This
  is the reconciliation of a model verdict; the separately computed
  deterministic fallback retains only its conservative, unmistakably scoped
  resident-lane rule and cannot inherit authority from the invalid verdict.
  A requested clarification pauses before lock acquisition, branch isolation,
  or writer execution. Governed mutating routes are surfaced before/at the
  execution boundary, and an availability fallback emits an explicit fallback
  notice. Pure Chat/Explain may stay visually quiet; the typed route source is
  retained by the dispatcher for that turn rather than falsely implying every
  conversational reply renders a provenance card.

  The router may receive a bounded conversation recap for follow-ups, but
  prior plans, TODOs, run notes, specifications, and base transcripts are
  context only. The final current-request block is the sole text that can
  authorize new work. For a read-only route, the healthy triage child may be
  reused to answer the user, making the semantic verdict an execution-level
  permission boundary rather than a prompt-only promise.

  The route-to-execution mapping is exact in the reference implementation:

  - `Chat` and `Explain` stay on the read-only child; they acquire no writer
    lock and start neither Director nor QC.
  - `QuickEdit` and `Fast` `Debug` use the resident single-writer lane with
    targeted verification; they do not convene a role team or full QC. If
    either path writes code, it may report completion only after a successful,
    observed targeted verification that ran after the last code write. A write
    is evidence of mutation, not evidence of verification, Director admission,
    or full-build completion; missing post-write verification settles the turn
    as `Failed` rather than manufacturing a completion card.
  - Every `Build`, including a `Fast` one, and every `Standard`/`Deep` `Debug`
    enter the Director workflow. The owned plan, gates, team and acceptance
    work remain proportional, so Director admission does not imply that every
    turn expands into the nine-phase greenfield play.
  - Only a healthy typed model decision (`RouteSource::Brain`) may cross that
    Director boundary. `RouteSource::DeterministicFallback` is an availability
    fallback on the resident proportional lane; it cannot by itself start the
    Director, a role team, or full post-build QC.

  `plan` is also an execution boundary for explicit commands, not merely a
  router hint. `/run`, `/goal`, and execution-style resume/continue requests
  settle as a typed non-executed `Planned` outcome before a run lock, isolation
  branch, governance/workflow write, or base writer session is created. They
  MUST NOT render `Done`. Ordinary conversation may still inspect the project
  and produce a read-only plan.

  In the reference drivers, “child/fork” names the orchestration operation,
  not transcript inheritance: Claude starts a new session id in plan mode,
  Codex starts a new read-only thread on a separate app-server, and OpenCode
  creates a new deny-by-default session. None uses the main conversation's
  resume/fork wire form for intent or critic work.
- **Live input separation.** While a writer run is active, natural-language
  input is conservatively separated into a question, an explicit adjustment
  to the current task, or deferred work. Only an unambiguous current-task
  adjustment is injected into the active writer. Questions and later/ambiguous
  tasks are queued FIFO as normal model-routed turns after the run settles; a
  gate question is answered on a separate read-only query while the gate stays
  open and is never silently reinterpreted as a revision. An explicit cancel
  stops the current run, clears the native resume/session hand-back, and writes
  a control boundary into conversation memory so the next model turn cannot
  accidentally continue the cancelled request. Deferred FIFO turns remain
  eligible for their own fresh routing.
  Attachment delivery is independently typed and capability-gated. Ordered
  text/image/file blocks retain their order through the vendor encoder; each
  accepted frame returns a path-free per-block receipt naming the actual
  delivery mode. Receipt strength is explicit: `transport_written` proves only
  a complete flushed frame, while `protocol_acknowledged` requires an exact
  vendor response correlated to that input; neither claims model progress.
  Claude replay acknowledgements, Codex `turn/steer` responses, and successful
  OpenCode `prompt_async` responses can reach the latter boundary. A normal
  Codex `turn/start` and Grok ACP prompt remain transport receipts until their
  separate event streams prove later lifecycle state. Unsupported blocks are
  rejected before local file bytes are
  read and are never rewritten into a hidden `@path` prompt. The reference
  driver treats only Codex's native `turn/steer` as proven same-turn steering;
  opening another turn or queueing future input is reported as such, not called
  steering. Attachment validation is bounded and rejects non-regular files,
  symlinks, content/extension mismatches, and files that change during the
  validation/read boundary.
- **L2 — Owned plan + scheduling.** For every Build and deliberate
  (`Standard`/`Deep`) Debug, the driver asks the brain for a proportional strict
  plan it **parses and owns** as a dependency DAG of steps, each with a
  mechanical acceptance check; the plan is persisted, and rendered as a live,
  steerable checklist. Scheduling obeys the
  single-writer / map-reduce-manage doctrine of §9.4: doing-roles drive the
  main session serially under the run lock; reviewing critics run on
  parallel fresh read-only child sessions (`UD-FLOW-007`).
  Every plan step and every observable base-native child agent is also projected
  into an append-only durable lifecycle journal with explicit parent,
  dependency, access mode, and terminal result. A vendor `Finished` event alone
  is not success: a native child's contribution settles successfully only when
  its parent step passes the deterministic acceptance floor. Vendor task ids are
  hashed before persistence, and a process exit converts active work into an
  explicit resumable interruption rather than silently losing it.
- **L3–L5 — Drive, verify, learn.** The driver walks the plan step by step,
  verifying each step against its acceptance on the deterministic floor
  (coverage / contract / verify / hard gate), self-correcting blocking
  findings with a typed, evidence-bearing rework directive. The reference
  driver classifies blockers as build / contract / coverage / behavior / craft,
  attaches classifier-owned root-cause and repair guidance, and tracks each
  stable fingerprint independently across source-tree snapshots. A second
  unchanged observation must change strategy; a third unchanged observation
  settles as an evidence-bearing escalation, while real source progress resets
  the recurrence count. A blocked/incomplete plan, dirty
  final QC, or residual finding at the round/time budget settles as `Failed`
  with blocking evidence and **MUST NOT** be rewritten to `Done`. Delivery
  artifacts (§5) and the proof pack (§6) are produced once the applicable floor
  is clean. Only eligible, evidence-backed events may update learned memory;
  an arbitrary run episode does not automatically become a lesson.

  The reference memory assets are intentionally separate:

  | Asset | Reference scope | Admission / use boundary |
  |---|---|---|
  | Pitfalls | Project incident ledger | Independent episodes count; repeated stderr lines do not, and generic/unclassified rows are quarantined. |
  | Lessons | Project rules plus privacy-reviewed family-safe global projections | Recurrence may create a pending candidate; validation requires an exact repair attempt followed by the same verifier passing. Raw incidents do not silently become cross-project memory. |
  | Learned skills | Project-local procedural candidates | Only non-trivial clean deliveries can graduate; exact prompt delivery and deterministic pass/fail/unknown evidence settle later use. |
  | Recipes | Project-local prior solutions | Strict stack/kind/shape matching; at most one advisory candidate, never an acceptance gate. |
  | Facts | Project-local stable facts | Extracted after meaningful work, bounded and secret-filtered; stale or contradictory facts are demoted/tombstoned and recall is limited to work turns. |
  | Run notes | Current-run working memory | UmaDev writes one bounded note only after a plan step made progress and passed deterministic acceptance. Failed, blocked, or empty-review steps do not write; the base is forbidden from writing notes directly. |
  | Open decisions | Project-visible unresolved-item register | Prompt recall may be disabled independently; the committed register and its report/count surface remain readable. |

  Run notes are bounded, untrusted history for later steps in the same run.
  They are not a transcript, cross-project memory, current authorization, or
  evidence that a step completed. Installed Skill packages are separately
  managed knowledge/rule/prompt assets, not the learned-skill store above.

  The reference implementation applies memory policy per leaf store and scope.
  Capture controls only new automatic writes; recall controls only historical
  data placed into a base prompt. Neither toggle deletes authoritative data,
  hides inventory/reporting, nor suspends already-created receipt settlement,
  trust/invalidation hygiene, or run-note rotation. Facts, recipes, and recurring-
  pitfall reflection check capture before an optional read-only base consult. A missing policy uses the
  documented defaults; an unreadable or malformed policy conservatively disables
  automatic capture and prompt recall while lifecycle bookkeeping remains active.

**Relation to `UD-FLOW-001`.** The §4 phase chain is the **deepest play the
coordinator routes the team into** — its plan for a full commercial greenfield build
*expands into* `research → docs → docs_confirm → spec → frontend →
preview_confirm → backend → quality → delivery`, with the gates of §4.2 /
§4.3 honored exactly. The chain is therefore canonical for that build, but
it is reached *by routing and planning*, not imposed on every turn: a chat
turn never enters it, a quick edit or fast narrow Debug takes the resident
path, and deeper Debug receives only its proportional Director plan. This is
the same scoping `UD-FLOW-001` now states
in §4.1. The `standard` profile of §8.4 *is* the full chain; the `seeai`
profile is its time-boxed variant; both remain plays the router can select.

## 10. Future work (V2 candidates)

Items considered for the V2 promotion to `MUST`:

- `UD-CODE-005` — accessibility token enforcement (alt text, aria-label, focus order)
- `UD-EVID-009` — model provenance trail (which model + version generated which lines)
- `UD-META-005` — remote audit endpoint (host pushes audit logs to an external evaluator)

These are explicitly **non-normative** in V1.

(The former V2 candidates `UD-FLOW-007` — role-critic team — and the role/
runtime evidence ideas have shipped and are now normative in §4.7 and §6.6.)

## Appendix A — Reserved keywords

The phase identifiers `research`, `docs`, `docs_confirm`, `spec`,
`frontend`, `preview_confirm`, `backend`, `quality`, `delivery`,
`build_fullstack`, `polish`, `handoff` are reserved and **MUST NOT** be
redefined by a conformant host.

The gate identifiers `docs_confirm`, `preview_confirm` are reserved.

The clause-ID prefix space `UD-*` is reserved for this specification.

## Appendix B — Change log

| Version | Date | Notes |
|---|---|---|
| 1.0.0-draft.1 | 2026-05-20 | Initial draft. Layers L1–L4 codified from the in-repo governance core and integration manager. |
| 1.0.0-draft.2 | 2026-05-22 | §7 host map narrowed to the three official SDK families; §9 rewritten for the Rust reference implementation (three execution modes, TUI). No normative clause changed. |
| 1.0.0-draft.3 | 2026-06-22 | Promoted shipped capabilities to normative clauses: `UD-FLOW-007` (role-critic team), `UD-FLOW-008` (trust tiers + irreversibility floor), `UD-ART-007` (PR artifact), `UD-EVID-006/007/008` (runtime / deploy / review-report evidence). §9 crate table updated to the ten-crate workspace; manifest `declared_by` synced to `umadev@1.0.x`. |
| 1.0.0-draft.4 | 2026-06-23 | Added §9.3 (continuous-session driving model) and §9.4 (team-of-roles collaboration model) describing how the reference implementation drives one long-lived base session per run and realizes `UD-FLOW-007` as a director-led team over a shared blackboard. **Non-normative**: no clause added, changed, or renumbered — both sections describe the reference driver and cite only existing clauses. |
| 1.0.0-draft.5 | 2026-06-24 | Added §9.5 (director-driven turn model: route → plan → schedule → deliver) describing how the reference implementation decides whether/how deeply to engage the flow contract per turn, and clarified the **scope** of `UD-FLOW-001` in §4.1: the phase chain is the `standard`-profile *full commercial build* — the deepest play the directing Agent routes to and its plan expands into — not a funnel every turn is forced through. Reconciles the everyday director/router/plan product behaviour with the normative chain. **Non-normative**: no clause added, changed, renumbered, or weakened — `UD-FLOW-001` stays a MUST for the build it governs; §9.5 and the §4.1 scoping paragraph cite only existing clauses. |
| 1.0.0-draft.6 | 2026-07-13 | Added §3.6 `UD-CODE-006` (architecture-fitness floor: god-file gate, architecture-doc layer-dependency rules, added-code clone advisory), matching the shipped `umadev_agent::arch_fitness` gate. The `-005` slot in the `UD-CODE-*` family stays reserved for the §10 accessibility candidate, so the family numbering skips to 006. No existing clause changed, renumbered, or weakened. |
| 1.0.0-draft.7 | 2026-07-15 | Clarified the shipped reference-driver boundaries: typed mutation authorization is exact and fail-closed; `plan` cannot be widened by routing, and explicit execution commands settle as non-executed `Planned` before acquiring execution state; a gate becomes interactive only after the writer boundary and answers questions through an independent read-only query; live input separates correction/question/future work and cancellation clears native resume context while retaining task ownership until real exit; QuickEdit/Fast Debug require strict observed post-write targeted verification; blocked, incomplete, dirty-QC, or budget-exhausted Director runs settle as `Failed`, never `Done`. Expanded the non-normative reference-driver map from three to eight authenticated base CLIs, with capability-negotiated ACP v1 shared by Cursor, CodeBuddy, Droid, Grok Build, and Qwen Code while Claude Code, Codex, and OpenCode retain their native protocols. Recorded that Grok Build headless ACP uses a cached token or `XAI_API_KEY` and never auto-selects OAuth or opens a browser. No clause was added or renumbered. |
| 1.0.0-draft.8 | 2026-07-16 | Deliberately narrowed the reference product to four deeply supported bases: Claude Code, Codex, OpenCode, and Grok Build. Cursor, CodeBuddy, Droid, and Qwen Code were removed from the current driver, command, configuration, and documentation surfaces rather than retained as previews or promotion candidates. Grok Build is the sole ACP-backed base; the other three retain native transports. No normative clause was added or renumbered. |
| 1.0.0-draft.9 | 2026-07-16 | Documented the reference driver's ordered typed text/image/file input contract, path-free per-block delivery receipts, attachment validation boundary, and vendor-specific same-turn steering semantics. Updated Grok Build recovery to prefer negotiated ACP `session/resume` and fall back to advertised `session/load`; a fresh session is used when neither capability exists. Documented the append-only plan/base-native child lifecycle journal and its rule that vendor completion is not acceptance. No normative clause was added or renumbered. |
| 1.0.0-draft.10 | 2026-07-16 | Reconciled non-normative product claims with the shipped reference driver: role seats are bounded base sessions rather than people; advisory fail-soft behavior is distinct from honest execution/gate failure; Offline is an internal deterministic fallback; continuous context and model visibility are capability-specific; and pitfalls, lessons, learned skills, recipes, facts, and run notes now state their separate scope and evidence gates. Corrected the executable spec-vector location. No normative clause was added, changed, or renumbered. |
| 1.0.0-draft.11 | 2026-07-16 | Updated the non-normative reference implementation to the eleven-crate workspace by adding `umadev-state`, and documented leaf-store memory capture/recall policy boundaries: toggles do not delete/report-hide data or block existing settlement/hygiene, capture-off avoids optional fact/recipe/reflection consults, and malformed policy disables automatic capture/recall conservatively. No normative clause was added, changed, or renumbered. |
| 1.0.0-draft.12 | 2026-07-17 | Reconciled Grok Build recovery claims with the fail-closed authority boundary: negotiated ACP resume/load is insufficient without effective-sandbox attestation and the required native pre-start validation, so the shipped product uses an explicit fresh-session handoff today. Documented capability- and session-scoped auxiliary controls, including server-authoritative background-process ownership checks. Clarified the reference Director's typed blocker classes, classifier-owned playbooks, per-fingerprint/source-snapshot strategy change and bounded escalation. No normative clause was added, changed, or renumbered. |
| 1.0.0-draft.13 | 2026-07-17 | Added Kimi Code as the fifth first-class source-audited base. The reference driver pins the official `MoonshotAI/kimi-code` 0.26.0 source/ACP identity, uses `kimi acp`, isolates all Grok private extensions, maps permissions without selecting Kimi auto/yolo, revalidates on-disk login without opening a browser, supports standard resume/load, model/mode configuration, structured input, approval, cancel and streaming, and records the honest upstream child-agent visibility limitation. No normative clause was added, changed, or renumbered. |
