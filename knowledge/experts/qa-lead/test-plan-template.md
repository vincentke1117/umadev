---
id: test-plan-template
title: QA Lead — Risk-Based Test Plan Template
domain: experts
category: qa-lead
difficulty: intermediate
tags: [acceptance, criteria, defect, experts, qa, risk, test-plan, traceability]
quality_score: 95
last_updated: 2026-06-29
---
# QA Lead — Risk-Based Test Plan Template

> A test plan is not "we will test everything." It is a **risk-prioritized,
> traceable contract** stating what will be tested, to what depth, in which
> environment, and the explicit criteria for declaring a release ready. Effort
> follows risk: the highest-risk flows get the deepest coverage.

## 1. Scope

State precisely, in two lists:
- **In scope** — features, flows, endpoints, platforms, and quality attributes (functional, performance, security, accessibility, compatibility) this plan covers.
- **Out of scope** — what this plan deliberately does **not** cover, and why (covered elsewhere, accepted risk, future iteration). An unstated exclusion is a coverage gap waiting to surprise you.

## 2. Risk-based prioritization

Rank every feature/flow by **risk = likelihood of failure × impact of failure**.
Allocate test depth by risk tier, not uniformly.

| Risk tier | Examples | Depth |
|---|---|---|
| Critical | Auth, payments, data integrity, anything irreversible or money-moving | Exhaustive: happy + all boundaries + all error/abuse paths + concurrency + E2E |
| High | Core user journeys, permission boundaries, data export/import | Happy + key boundaries + main error paths + E2E smoke |
| Medium | Secondary features, admin tools | Happy + obvious errors |
| Low | Cosmetic, rarely used, low blast radius | Smoke / spot check |

Impact axes to weigh: money loss, data loss/corruption, security/privacy breach,
legal/compliance, number of users affected, reversibility, brand/trust.

## 3. Test approach per layer

Map each area to the right test layer (test each layer at the right altitude — see `testing/01-standards/test-strategy-and-layering`):

| Layer | What is verified here | Type |
|---|---|---|
| Domain logic | Business rules, invariants, state machines, calculations | Unit (no IO) |
| Application/service | Use-case orchestration, transactions, error paths (deps mocked) | Unit / integration |
| Persistence | Real reads/writes, mapping, query correctness | Integration (real DB) |
| API/interface | Routing, validation, status codes, error envelope, authz | Integration / contract |
| Cross-cutting | Performance, security, accessibility, compatibility | Specialized suites |
| Critical journeys | Signup → core action → checkout, with key failures | E2E |

## 4. Entry and exit criteria

Make "ready to test" and "ready to ship" objective, not a feeling.

**Entry criteria (testing may begin when):**
- [ ] Requirements have structured, testable acceptance criteria.
- [ ] Build deploys to a test environment and boots cleanly.
- [ ] Test data and accounts are available; dependencies/stubs are up.
- [ ] Smoke test of the critical path passes (no point deep-testing a dead build).

**Exit criteria (release is ready when):**
- [ ] 100% of critical and high acceptance criteria pass.
- [ ] No open critical/blocker defects; high defects triaged with a decision.
- [ ] Coverage thresholds met (variance and diff coverage — see `testing/01-standards/ci-test-gates-and-coverage`).
- [ ] Performance, security, and accessibility checks pass their budgets.
- [ ] No unexplained flaky tests in the required suite.
- [ ] Rollback procedure documented and verified.

## 5. Traceability matrix

Prove coverage end to end — every requirement maps to acceptance criteria to test cases, with no orphans either way.

| Req ID | Acceptance criteria | Test case IDs | Layer | Risk | Status |
|---|---|---|---|---|---|
| FR-012 | AC-012a..d | T-101..104 | API + E2E | Critical | Pass |

Rules:
- Every acceptance criterion has ≥1 test case (no untested requirement).
- Every test case traces to a requirement (no test for a feature nobody asked for).
- Coverage gaps (a requirement with no test) are visible and block exit.

## 6. From acceptance criteria to test cases

Each criterion expands into multiple cases across the standard axes — happy,
boundary, error, permission, concurrency, state. Example for a login criterion:

1. Valid credentials → success + redirect.
2. Wrong password → generic error (no user enumeration).
3. Non-existent user → same generic error.
4. Empty/invalid-format fields → field validation.
5. N failed attempts → lockout + 429.
6. Injection / script in fields → sanitized, no execution.
7. Concurrent duplicate submit → one session, no double effect.
8. Redirect back to originally requested page after login.

## 7. Defect severity and triage

Classify every defect; severity drives whether it blocks release.

| Severity | Definition | Release impact |
|---|---|---|
| Blocker | Core flow unusable, data loss, security hole, money error | Must fix before ship |
| Critical | Major feature broken, no acceptable workaround | Must fix or formally accept |
| Major | Feature impaired, workaround exists | Fix soon; may ship with sign-off |
| Minor | Cosmetic, edge inconvenience | Backlog |

A defect report must be reproducible: steps, expected vs actual, environment,
build/version, evidence (logs/screenshot), and severity. "It doesn't work" is not a defect report.

## 8. Environments and test data

- Test in an environment that mirrors production (config, dependencies, data shape).
- Use isolated, disposable test data per case; never depend on shared mutable state or execution order.
- Anonymize any production-derived data; never test against real personal data.
- Make non-determinism controllable: injectable clock, fixed seeds, stable ordering.

## 9. Test plan checklist (the plan is ready when)

- [ ] In-scope and out-of-scope explicitly listed.
- [ ] Every feature risk-tiered; depth allocated by risk.
- [ ] Each area mapped to the correct test layer.
- [ ] Entry and exit criteria defined and objective.
- [ ] Traceability matrix links requirements ↔ acceptance ↔ tests.
- [ ] Non-functional coverage (perf/security/a11y/compat) planned with budgets.
- [ ] Defect severity scale and triage path agreed.
- [ ] Environments, test data, and rollback verification covered.
