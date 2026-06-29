---
id: prd-template-and-structure
title: Product Manager — PRD Template & Structure (EARS-anchored)
domain: experts
category: product-manager
difficulty: intermediate
tags: [prd, product-requirements, template, ears, acceptance-criteria, scope, traceability, definition-of-done, writing]
quality_score: 95
last_updated: 2026-06-29
---
# Product Manager — PRD Template & Structure (EARS-anchored)

> A PRD is not a pitch deck and not a wish list. It is the **contract between
> intent and implementation**: a document an engineer can build from, a QA can
> test against, and a stakeholder can sign off on — without a meeting to decode
> it. A commercial-grade PRD anchors every requirement in the structured
> acceptance syntax (see `experts/product-manager/requirements-engineering-ears`)
> so that *what we are building* and *how we will know it works* are the same
> artifact, not two documents that drift apart.

## 1. What a PRD must do

A PRD earns its place only if it answers five questions unambiguously:

- **Why** — the problem, who has it, and why now (the case for building at all).
- **What** — the requirements, each atomic, triggered, and testable.
- **What not** — explicit out-of-scope, so scope creep is visible.
- **How we'll know** — acceptance criteria and success metrics with numbers.
- **What it touches** — data, contracts, dependencies, and risks.

A PRD that reads beautifully but cannot be turned into test cases has failed,
no matter how polished the prose.

## 2. The PRD skeleton

Fill every section. "N/A" is an acceptable answer; a blank is not — a blank
means the question was never asked.

| # | Section | Content |
|---|---|---|
| 1 | **Summary** | One paragraph: what this is, for whom, and the outcome. Readable by someone outside the team. |
| 2 | **Problem & context** | The user problem, evidence it's real (data/research, not opinion), and why now. |
| 3 | **Goals & non-goals** | Goals as measurable outcomes; non-goals as explicit exclusions. |
| 4 | **Success metrics** | The numbers that define success + how each is measured (see §5). |
| 5 | **Users & scenarios** | Target users/personas and the key scenarios (jobs to be done). |
| 6 | **Requirements** | The core: structured functional requirements + acceptance criteria (see §3). |
| 7 | **Non-functional requirements** | Performance, availability, security, accessibility, privacy — each with a number and a method. |
| 8 | **Scope** | In-scope this release vs out-of-scope / later, stated explicitly. |
| 9 | **Dependencies & risks** | Upstream/downstream systems, data, teams; risks with mitigation. |
| 10 | **Data & contract impact** | New/changed data, API/contract changes, migration and privacy implications. |
| 11 | **Rollout & success gate** | How it ships (flagged/phased), what metric gates each stage, how it rolls back. |
| 12 | **Open questions** | Unresolved decisions with an owner and a needed-by date. |
| 13 | **Traceability** | The matrix tying requirement → criteria → design/API → tests (see §6). |

## 3. The heart: requirements + acceptance criteria

Every functional requirement is written in one of the five structured patterns
(ubiquitous / event-driven / state-driven / unwanted-behavior / optional-feature)
and immediately followed by its acceptance criteria in Given–When–Then form.
The requirement says *what must hold*; the criteria enumerate *the cases that
prove it*. Use this block, repeated per requirement:

```
### FR-012  Checkout creates exactly one order

Requirement (event-driven):
  When a signed-in user submits the checkout form with a valid card,
  the system shall create one order, charge the card exactly once,
  and return HTTP 201 with the order id.

Acceptance criteria:
  - Given a valid cart and card, when checkout is submitted,
    then an order is created and HTTP 201 with orderId is returned.
  - Given the same idempotency key is replayed, when checkout is submitted twice,
    then exactly one order and one charge exist.
  - Given the card is declined, when checkout is submitted,
    then no order is created and the decline reason is shown (HTTP 402).
  - Given the network drops after charge but before response, when the client retries,
    then no second charge occurs.

Out of scope: saved-card management; partial refunds.
```

Mandatory case coverage per requirement — enumerate criteria across:
**happy path · boundary values · error/unwanted · permission · concurrency ·
state (empty/loading/partial/stale) · idempotency**. A requirement with only a
happy path is not ready.

## 4. Goals and non-goals (kill scope creep early)

| Goal (measurable outcome) | Non-goal (explicit exclusion) |
|---|---|
| Reduce checkout abandonment by 15% in 90 days | Redesigning the cart UI |
| Support guest checkout end to end | Loyalty/points integration (next release) |

Rules: goals are **outcomes** (a metric moves), not outputs ("build a button");
every non-goal prevents a future "but I assumed…" argument.

## 5. Success metrics (numbers, not adjectives)

| Metric | Target | How measured | Guardrail |
|---|---|---|---|
| Checkout completion rate | +15% vs baseline | Funnel analytics, 90-day window | No drop in payment success rate |
| Checkout p95 latency | < 800ms | Server metrics | — |

Every product metric needs a **guardrail metric** — a thing that must *not*
get worse while you optimize the target (e.g., don't raise conversion by
hiding fees). State the baseline; "improve" without a baseline is unfalsifiable.

## 6. Traceability matrix

The PRD ships (or generates) a matrix so coverage gaps are visible at a glance:

| Req ID | Requirement (structured) | Acceptance criteria | Design/API | Tests | Status |
|---|---|---|---|---|---|
| FR-012 | When … shall create one order … | AC-012a..d | `POST /orders` | T-101..104 | Ready |

Rules: every requirement has ≥1 criterion and ≥1 planned test; every screen and
endpoint traces back to a requirement (no orphan features); a requirement with
no test is not shippable, a test with no requirement is scope creep — resolve, don't ignore.

## 7. Definition of Ready (the PRD may enter build only if)

- [ ] Problem stated with evidence; goals are measurable outcomes; non-goals explicit.
- [ ] Every functional requirement in a structured pattern (atomic, triggered, observable).
- [ ] Acceptance criteria enumerated across happy/boundary/error/permission/concurrency/state/idempotency.
- [ ] NFRs carry numbers and a measurement method (no "fast", "secure", "intuitive").
- [ ] Scope: in-scope vs out-of-scope stated; dependencies and risks identified.
- [ ] Data/contract/migration/privacy impact described.
- [ ] Rollout + rollback + success-gate metrics defined.
- [ ] Open questions each have an owner and a needed-by date.
- [ ] Traceability matrix present; no orphan features, no untested requirements.

## 8. Anti-patterns (reject in review)

1. **Solution-as-requirement** — "use a modal / use React" in a requirement; that's design, decided later.
2. **Adjective acceptance** — "should be fast/intuitive" with no observable, measurable outcome.
3. **Happy-path-only** — no error, boundary, permission, or concurrency criteria.
4. **No non-goals** — everything is implicitly in scope, so scope creep is invisible.
5. **Output goals** — "ship a settings page" instead of the outcome it should produce.
6. **Vanity metric, no guardrail** — optimize one number while quietly degrading another.
7. **Orphan feature** — a screen/endpoint with no requirement behind it.
8. **Drift** — acceptance criteria live in a separate doc and fall out of sync with the requirement.
9. **Blank ≠ N/A** — sections silently skipped, so unasked questions surface mid-build.
