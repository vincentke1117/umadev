# Contributing to UmaDev | 贡献指南

Thank you for contributing to UmaDev! 感谢贡献。

UmaDev is a **Rust workspace** that ships one native `umadev` binary for
each supported target. npm is a distribution surface: its small JavaScript
launcher selects the matching prebuilt Rust binary. There are no per-host SDK
or provider integrations. Exactly four base CLIs are driven as subprocesses:
Claude Code, Codex, and OpenCode keep vendor-specific transports; Grok Build
uses a dedicated profile over the hardened ACP v1 core. Contributions can target the specification, governance kernel,
team runner, host drivers, knowledge layer, CLI, or TUI.

## How to contribute | 如何贡献

1. Fork [umadev](https://github.com/umacloud/umadev).
2. Create a feature branch: `git checkout -b feat/your-feature`.
3. Make your changes; run the local checks below.
4. Open a Pull Request against `main`.

## Development setup | 开发环境

Required:

- Rust **1.88+** (the workspace MSRV; check with `rustc --version`).
- A working `cargo` (`rustup` is the easiest installer).

The product runtime is Rust and does not vendor a host SDK. The following are
needed only when changing their corresponding delivery surface:

- Node.js **20+** for the npm launcher, npm package smoke tests, or website.
- Python 3 for the release-only embedding-model quantisation script.
- Docker (through `cross`) for reproducing the Linux release builds and their
  glibc 2.31 compatibility floor.

```bash
git clone https://github.com/umacloud/umadev.git
cd umadev
cargo build --workspace
```

## Workspace layout | 工作区结构

```
crates/
├── umadev/             # main binary (clap CLI)
├── umadev-spec/        # UMADEV_HOST_SPEC_V1 as Rust data
├── umadev-governance/  # rules / audit / context / compliance kernel
├── umadev-agent/       # 9-phase runner + gates + state + experts + coach
├── umadev-host/        # 3 vendor-specific drivers + 1 Grok Build ACP v1 profile
├── umadev-runtime/     # Runtime trait + OfflineRuntime (deterministic fallback)
├── umadev-contract/    # typed OpenAPI 3.1 contract layer
├── umadev-knowledge/   # BM25 + optional vector/hybrid retrieval
├── umadev-tui/         # ratatui terminal UI
├── umadev-i18n/        # zh-CN / zh-TW / en UI catalog
└── umadev-state/       # safe persistence + leaf-store memory policy

spec/
└── UMADEV_HOST_SPEC_V1.md   # normative specification
```

## Local checks | 本地校验

Every PR must pass these commands clean:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo test --workspace --all-features --all-targets --no-fail-fast
cargo test --workspace --all-features --doc --no-fail-fast
```

If npm packaging, self-update, or platform-package resolution changed, also run:

```bash
./npm/scripts/smoke.sh
```

Convenient aliases (no installation required):

```bash
cargo fmt --all                       # apply formatting
cargo clippy --workspace --fix        # auto-apply safe lint fixes
cargo test --workspace --all-targets  # unit + integration + doc tests
```

The authenticated base acceptance suite is intentionally ignored in ordinary
CI because it makes real model calls. Before a release, run its three generic
checks against each installed and logged-in base:

```bash
for base in claude-code codex opencode grok-build; do
  UMADEV_LIVE_BASE="$base" cargo test -p umadev-host --test live_base_contract installed_base -- --ignored --nocapture --test-threads=1
done
```

Grok's private queue, owned background-process control, and release-stamped
Folder Trust path have a separate official-binary gate:

```bash
GROK_TEST_VERSION=0.2.101 GROK_FOLDER_TRUST=1 UMADEV_LIVE_BASE=grok-build \
  cargo test -p umadev-host --test live_base_contract installed_grok_ -- --ignored --nocapture --test-threads=1
```

These probes use isolated temporary workspaces, but they exercise the real
authenticated CLIs. Do not put credentials in the repository or enable them on
untrusted pull-request runners.

## Adding a new spec clause | 新增规范条款

Spec changes touch four places that **must stay in sync**:

1. **Markdown** — add the new clause section to `spec/UMADEV_HOST_SPEC_V1.md`.
2. **Rust data** — append a `Clause { id, layer, title, level, section }`
   entry to `crates/umadev-spec/src/lib.rs#CLAUSES`. IDs are
   permanent — never renumber.
3. **Implementation** — if the clause is enforceable, add the
   judgment / audit logic to `crates/umadev-governance/src/{rules,audit,...}.rs`.
4. **Compliance mapping** — if the clause maps to external frameworks,
   extend `framework_for()` in
   `crates/umadev-governance/src/compliance.rs`.

Tests in `crates/umadev-spec/src/lib.rs` pin the clause-table
structure; they will fail if you add a malformed ID. Add a unit test
for the new rule alongside the implementation.

## Base-driver scope | 底座驱动范围

> First-class support is deliberately fixed at **exactly five** bases:
> `claude-code`, `codex`, `opencode`, `grok-build`, and `kimi-code`.
> `umadev_host::BACKEND_IDS` is authoritative
> and tests pin both the contents and length. Wider model coverage belongs in
> the selected base's own provider configuration, not in another UmaDev driver.

A base is an already-installed and already-configured AI coding CLI driven as a
subprocess. UmaDev vendors no Agent SDK and owns no model credential.
`umadev install --base ...` installs a UmaDev governance hook or pre-commit
integration; it is **not** a base installer, updater, login flow, or licence
manager.

The current adapter families have different maintenance rules:

1. **Native transports:** changes to Claude Code (`stream-json`), Codex
   (`app-server`), or OpenCode (HTTP/SSE) stay in their dedicated modules.
   Preserve vendor request ids, permission semantics, session ids, cancellation,
   bounded shutdown, and exact cross-process resume.
2. **Grok Build ACP v1 profile:** changes to Grok Build reuse
   `crates/umadev-host/src/acp.rs`. Keep executable names,
   launch flags, identity/version checks, permission-mode mapping, and verified
   extensions in the vendor profile; do not fork the transport core.
3. **Capability negotiation:** authentication, `session/load`, modes, questions,
   plans, MCP elicitation, and vendor extensions are accepted only when the
   installed agent advertises or sends the corresponding protocol surface.
   Unknown requests never receive automatic authority.
4. **Read-only proof:** Plan must be enforced by a documented process flag or a
   successfully negotiated read-only mode. Grok Build adds its documented
   read-only sandbox, tool allowlist, and subagent fence.
5. **Platform claims:** test native and `.cmd`/`.bat`/PowerShell resolution on
   the platforms the vendor actually supports. UmaDev's own platform support
   never broadens the selected vendor CLI's platform boundary.
6. **No provenance guessing:** protocol and flag compatibility are not evidence
   that one vendor copied another.

Any proposal to replace or expand this fixed list is a product/spec change, not
a routine driver patch. Open an issue first with official non-interactive
protocol documentation, permission and resume evidence, authentication flow,
minimum-version strategy, macOS/Linux/Windows support boundaries, and a test
plan. If accepted, the host registry, CLI enum, TUI command aliases, doctor,
MCP manager, spec, website, and user docs must change atomically. The
`backend_arg_ids_match_host` and `every_backend_arg_has_a_driver` tests catch
only part of that contract.

## Commit conventions | 提交规范

Follow [Conventional Commits](https://www.conventionalcommits.org):

```
feat(scope): description    # new functionality
fix(scope): description     # bug fix
docs: description           # documentation only
test: description           # tests only
refactor(scope): description
chore: description          # tooling, deps
ci: description             # GitHub Actions / release workflow
```

Common scopes: `spec`, `governance`, `agent`, `runtime`, `cli`,
`install`, `plugin`, `coach`.

## PR checklist | PR 自检清单

Before requesting review:

- [ ] `cargo fmt --check` clean
- [ ] `cargo clippy -D warnings` clean
- [ ] workspace target tests and doctests are green
- [ ] `./npm/scripts/smoke.sh` is green when npm/update packaging changed
- [ ] New code has unit tests in the same file (`mod tests { ... }`)
- [ ] If you changed `spec/UMADEV_HOST_SPEC_V1.md`, you also
      changed `crates/umadev-spec/src/lib.rs#CLAUSES` (or vice versa)
- [ ] PR description explains the *why*, not just the *what*
- [ ] CHANGELOG.md updated under `[Unreleased]` for user-visible changes

## Reporting issues | 报告问题

Open issues at https://github.com/umacloud/umadev/issues with:

- `umadev verify` output (paste verbatim)
- Reproduction steps
- Expected vs actual behavior
- OS + `rustc --version` for build issues

## License | 许可

By contributing you agree your code is licensed under the project's
[MIT License](LICENSE).
