---
id: contract-first-api-design
title: Architect — Contract-First / API-First Workflow
domain: experts
category: architect
difficulty: intermediate
tags: [api, architecture, contract, design, experts, integration, openapi, schema, versioning]
quality_score: 95
last_updated: 2026-06-29
---
# Architect — Contract-First / API-First Workflow

> The API contract is the **single source of truth** that frontend, backend,
> QA, and external integrators all build against. Write the contract **before**
> writing implementation code. A contract authored after the code merely
> documents whatever was built; a contract authored first becomes a design tool,
> a parallelization enabler, and an automated gate.

## 1. Why contract-first

- **Parallel work** — once the schema is agreed, frontend and backend build
  simultaneously against the same definition instead of waiting on each other.
- **One source of truth** — the machine-readable schema (OpenAPI / GraphQL SDL /
  protobuf) is generated from nothing else and nothing is generated without it.
- **Early design feedback** — interface mistakes (chatty endpoints, leaking the
  data model, wrong resource granularity) surface in review of a 1-page schema,
  not after a sprint of code.
- **Automated alignment** — the contract becomes a test fixture: mock servers,
  client/server stubs, request/response validators, and a CI gate all derive
  from it (see `testing/01-standards/contract-testing-and-api-contracts`).

## 2. The workflow

```
1. Model the resources + operations from the requirements (not the DB tables).
2. Write the contract: paths, methods, request/response schemas, error model, auth.
3. Review the contract with consumers (frontend, integrators, QA) — sign off the shape.
4. Generate: a mock server (consumers unblock) + server stubs + typed clients.
5. Implement against the contract; validate every request/response in tests.
6. Evolve only through additive change + explicit versioning + deprecation.
```

The contract is a **reviewable artifact** that lands in the repo
(`openapi.yaml` / `schema.graphql` / `*.proto`) and is diffed on every change.

## 3. Designing the resource model

- Model **resources and operations**, not your storage schema. The API is a
  product surface; the database is an implementation detail. Never expose ORM
  entities or column names directly.
- Use **nouns for resources** (`/orders`), HTTP methods for verbs. Keep nesting
  to ≤2 levels; flatten deeper relationships with query filters or links.
- Choose **granularity by use case**: avoid chatty designs (N calls to render
  one screen) and avoid god-endpoints that return everything. Provide
  field selection / expansion where payloads vary by caller.
- Make **read and write shapes explicit**: a create request is not the same
  schema as the resource it returns. Define request DTOs and response DTOs separately.

## 4. Mandatory contract elements

Every contract must pin down, before code:

| Element | Requirement |
|---|---|
| Resources & paths | Every path, method, and path/query parameter typed. |
| Request schema | Required vs optional fields, types, formats, ranges, enums. |
| Response schema | Success body shape per status code, with examples. |
| Status codes | The full set per operation (200/201/204/400/401/403/404/409/422/429/5xx). |
| Error model | One consistent error envelope across all endpoints (see §5). |
| Auth | Which endpoints require auth and the scheme (bearer token, scopes/roles). |
| Pagination | The pagination contract (cursor preferred) where collections are returned. |
| Idempotency | Idempotency-key header on unsafe operations that may be retried. |
| Rate limits | Documented limits + `429` + `Retry-After` semantics. |
| Versioning | The version strategy and how breaking change is signalled. |

## 5. Standard error envelope (define once, use everywhere)

A single error shape lets every client handle failures uniformly:

```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "Human-readable, safe to display",
    "details": [{ "field": "email", "message": "Invalid email format" }],
    "requestId": "req_abc123"
  }
}
```

- `code` is a stable machine string clients branch on; `message` is for humans.
- Never leak stack traces, SQL, or internal identifiers in error bodies.
- Map every failure mode in the contract to a code, so QA can assert on them.

## 6. Versioning and evolution

- **Additive change is safe**: adding an optional field or a new endpoint does
  not break existing clients. Make every consumer tolerant of unknown fields.
- **Breaking change requires a new version**: removing/renaming a field,
  changing a type, tightening validation, or changing status-code semantics.
- Carry the major version in the URL prefix (`/api/v1/...`) for cache- and
  test-friendliness; increment only on breaking change.
- **Deprecate, don't delete**: announce, mark deprecated in the contract, emit a
  deprecation signal (header), set a sunset date, and keep the old version until
  consumer traffic drains.
- Treat the contract diff as the **breaking-change detector** in CI — a removed
  field or narrowed type fails the build unless the version is bumped.

## 7. Contract as a CI gate

The contract is only worth writing if it is enforced. Wire it up so drift fails the build:

- **Lint the contract**: schema validity, naming consistency, required examples.
- **Validate runtime traffic**: the server's real responses must satisfy the
  schema (response validation in integration tests).
- **Cross-check the frontend**: extract the client's `fetch`/HTTP calls and
  assert every called path/method/shape exists in the contract — no call to an
  undefined endpoint, no endpoint silently changed under a live consumer.
- **Detect breaking diffs**: compare the new contract to the last released one;
  block merge on a breaking change without a version bump.

## 8. Architect checklist (before implementation starts)

- [ ] Contract authored and committed before non-trivial endpoint code.
- [ ] Resources modeled from use cases, not from database tables.
- [ ] Request/response/error schemas fully typed, with examples per operation.
- [ ] One consistent error envelope across all endpoints.
- [ ] Auth, pagination, idempotency, and rate-limit semantics specified.
- [ ] Versioning strategy and deprecation policy stated.
- [ ] Mock server generated so frontend can build in parallel.
- [ ] CI validates server responses and frontend calls against the contract.

## 9. Anti-patterns

1. **Code-first, document-later** — generating docs from finished code, so the API shape was never reviewed.
2. **Exposing the data model** — returning ORM entities / DB column names as the API.
3. **Inconsistent errors** — every endpoint inventing its own error shape.
4. **Silent breaking change** — renaming a field in v1 and hoping clients cope.
5. **Chatty or god endpoints** — N calls to render a screen, or one endpoint returning everything.
6. **Unenforced contract** — a beautiful schema that nothing in CI checks, so code and contract drift apart.
