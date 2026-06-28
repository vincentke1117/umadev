---
id: requirements-engineering-ears
title: Product Manager — Structured Acceptance Syntax (EARS Patterns)
domain: experts
category: product-manager
difficulty: intermediate
tags: [acceptance, criteria, ears, product, requirements, specification, traceability, writing]
quality_score: 95
last_updated: 2026-06-29
---
# Product Manager — Structured Acceptance Syntax (EARS Patterns)

> Free-text requirements ("the system should be fast and user-friendly") cannot
> be tested, estimated, or signed off. A commercial-grade PRD writes every
> requirement in a **constrained sentence syntax** so it is unambiguous,
> singular, and verifiable. This is the bridge between a product idea and a
> test case.

## 1. Why a constrained syntax

Natural-language requirements fail in four predictable ways: they are
**ambiguous** (multiple readings), **compound** (several requirements in one
sentence), **untestable** (no observable outcome), or **silent on triggers and
preconditions** (when does this apply?). A structured syntax removes all four by
forcing every requirement into one of five fixed shapes, each with an explicit
trigger, an explicit subject (the system), and an explicit, observable response.

A requirement is well-formed only if it is:
- **Atomic** — exactly one testable behavior (no "and"/"also" hiding a second requirement).
- **Triggered** — states the precondition or event under which it applies.
- **Observable** — the response is something a test can assert (status code, value, state, screen).
- **Unconditionally testable** — no vague adjectives ("fast", "intuitive", "robust").

## 2. The five sentence patterns

Use exactly one keyword per pattern. The subject is always the system (or a named component).

| Pattern | Shape | Use when |
|---|---|---|
| **Ubiquitous** | `The <system> shall <response>.` | An always-true invariant, no trigger. |
| **Event-driven** | `When <trigger>, the <system> shall <response>.` | A discrete event causes a response. |
| **State-driven** | `While <state>, the <system> shall <response>.` | A behavior holds for the duration of a state. |
| **Unwanted-behavior** | `If <unwanted condition>, then the <system> shall <response>.` | Error, abuse, or failure handling. |
| **Optional-feature** | `Where <feature is present>, the <system> shall <response>.` | Behavior only when a feature/config is enabled. |
| **Complex** | Combine keywords: `While <state>, when <trigger>, the <system> shall <response>.` | A trigger that only applies in a state. |

### Examples (vague → structured)

| Vague | Structured |
|---|---|
| "Login should be secure." | **If** 5 failed login attempts occur from one account within 10 minutes, **then** the system **shall** lock the account for 15 minutes and return HTTP 429. |
| "The dashboard should load fast." | **When** a signed-in user requests `/dashboard`, the system **shall** return First Contentful Paint under 1.5s on a 4G connection at the 75th percentile. |
| "Handle payment failures gracefully." | **If** the payment provider returns a decline, **then** the system **shall** preserve the cart, display the provider's decline reason, and **shall not** charge the card. |
| "Show notifications." | **Where** browser push is enabled, **when** an order ships, the system **shall** deliver a push notification within 60 seconds. |
| "Search should work." | **When** a user submits a query of 1–256 characters, the system **shall** return matching results ranked by relevance within 300ms p95. |

## 3. From requirement to acceptance criteria

Each structured requirement expands into concrete acceptance criteria in
**Given–When–Then** form — the executable companion of the requirement. The
requirement says *what must hold*; the criteria enumerate the *cases that prove it*.

**Requirement (event-driven):**
> When a user submits the checkout form with a valid card, the system shall create the order, charge the card exactly once, and return HTTP 201 with the order id.

**Acceptance criteria:**
- Given a valid cart and card, when checkout is submitted, then an order is created and HTTP 201 with `orderId` is returned.
- Given the same idempotency key is replayed, when checkout is submitted twice, then exactly one order and one charge exist.
- Given the card is declined, when checkout is submitted, then no order is created and the decline reason is shown (HTTP 402).
- Given the network drops after charge but before response, when the client retries, then no second charge occurs.

Every criterion must be **independently verifiable** and map to at least one
automated test. A requirement with zero acceptance criteria is not done; it is a wish.

## 4. Mandatory case coverage per requirement

For every requirement, the PRD must enumerate criteria across these axes (the
same edge-case discipline applied to specification, not just QA):

- **Happy path** — the normal success case.
- **Boundary values** — 0, 1, max, max+1, empty, negative, oversize.
- **Error / unwanted** — invalid input, missing fields, downstream failure, timeout.
- **Permission** — unauthenticated, authenticated-but-forbidden, owner vs non-owner.
- **Concurrency** — duplicate submit, race, stale read, double-click.
- **State** — empty (no data yet), loading, partial, deleted/stale.
- **Idempotency** — safe to retry; replays produce one effect.

## 5. Non-functional requirements are requirements too

NFRs use the same syntax and must carry a number and a measurement method —
never an adjective.

| Category | Structured NFR | How to verify |
|---|---|---|
| Performance | When a list endpoint is called, the system shall respond within 200ms p95. | Load test + server metrics |
| Availability | The system shall sustain 99.9% monthly uptime. | Uptime monitor + error budget |
| Capacity | While under 1000 concurrent users, the system shall keep error rate below 0.1%. | Load test |
| Security | The system shall reject any request without a valid token with HTTP 401. | Automated auth test |
| Accessibility | The system shall meet AA contrast and full keyboard operability on all interactive elements. | Automated a11y scan + manual keyboard pass |
| Privacy | The system shall encrypt personal data at rest and in transit. | Config audit + scan |

## 6. Traceability matrix

Every requirement is traceable end to end. A commercial PRD ships (or generates)
a matrix so coverage gaps are visible:

| Req ID | Requirement (structured) | Acceptance criteria | Design/API | Test cases | Status |
|---|---|---|---|---|---|
| FR-012 | When … shall create the order … | AC-012a..d | `POST /orders` | T-101..104 | Done |

Rules:
- Every requirement ID has ≥1 acceptance criterion and ≥1 test case.
- Every API endpoint and screen traces back to a requirement (no orphan features).
- A requirement with no test is **not** shippable; a test with no requirement is **scope creep** — resolve, don't ignore.

## 7. Definition of Ready (a requirement may enter a sprint only if)

- [ ] Written in one of the five structured patterns (atomic, triggered, observable).
- [ ] Acceptance criteria enumerated across happy / boundary / error / permission / concurrency.
- [ ] Non-functional targets stated with numbers and a measurement method.
- [ ] Out-of-scope explicitly listed (what this requirement does **not** cover).
- [ ] Dependencies and data/contract impacts identified.
- [ ] No vague adjectives remain ("fast", "easy", "nice", "secure", "robust").

## 8. Anti-patterns (reject in review)

1. **Compound requirement** — one sentence hiding two behaviors joined by "and".
2. **Solutioning** — "use a dropdown" / "use React" in a requirement (that's design, not requirement).
3. **Adjective acceptance** — "should be intuitive" with no observable outcome.
4. **Happy-path only** — no error, boundary, or permission criteria.
5. **Untriggered** — no precondition, so the reader can't tell when it applies.
6. **Unmeasurable NFR** — "should be fast" with no number, percentile, or method.
7. **Orphan feature** — a screen or endpoint with no requirement behind it.
