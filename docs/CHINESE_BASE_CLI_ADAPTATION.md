# Adapting Chinese domestic base CLIs (CodeBuddy / Qoder / Kimi)

Feasibility study + driver blueprint for wiring the Chinese domestic coding CLIs as
UmaDev base backends. Answers the question **"can it be done?"** with evidence, then
gives the exact driver spec for the ones that can.

## The real bar: the bidirectional persistent stream-json SESSION

UmaDev does **not** drive a base with a one-shot `binary -p "<prompt>"` that exits.
Its default (and richest) path is a **long-lived bidirectional stream-json NDJSON
session** — `crates/umadev-host/src/claude_session.rs`:

```
claude --print --input-format stream-json --output-format stream-json --verbose \
       --session-id <uuid> --permission-mode <acceptEdits|default|plan>
```

The process is spawned **once**, stdin stays **open**, and one stream-json `user`
message is written per turn:
`{"type":"user","message":{"role":"user","content":[{"type":"text","text":"…"}]}}`,
reading `system`(init) → `assistant` → `result` NDJSON back across many turns on the
SAME process. Critic forks use `--permission-mode plan` (read-only).

So the make-or-break question for each candidate is **not** "does it have `-p`?" (they
all do) — it is **"does it support the persistent `--input-format stream-json` session +
`--session-id` pinning + a read-only `plan` mode?"**

## Verdict per base

| Base | npm / binary | Bidirectional session | `--session-id` | `plan` mode | Verdict |
|---|---|---|---|---|---|
| **CodeBuddy Code** | `@tencent-ai/codebuddy-code` / `codebuddy` | ✅ documented verbatim | ✅ | ✅ | **First-class driver — reuse `claude_session.rs`** |
| **Qoder CLI** | `@qoder-ai/qodercli` (CN `@qodercn-ai/qoderclicn`) / `qodercli` | ✅ present in the shipped binary | ✅ | ✅ | **First-class driver — reuse `claude_session.rs`** |
| **Kimi Code** | `@moonshot-ai/kimi-code` / `kimi` | ❌ native (uses `kimi acp` = JSON-RPC/ACP, a different wire) | n/a | n/a | **No native claude-style driver; use the config path** |

### CodeBuddy Code — YES (Claude Code fork)

Tencent's CodeBuddy Code is a Claude Code derivative. Its official headless doc states
verbatim that `--input-format stream-json` reads *"a stream of messages provided via
stdin, where each message represents a user turn — this allows for multi-turn
conversations **without restarting the `codebuddy` binary**"*, and ships the literal
two-user-message-into-one-process example. `--session-id <uuid>` is accepted with the
same semantics; `--permission-mode` accepts the exact Claude set
`acceptEdits|bypassPermissions|default|plan`. The user-message JSON shape is
byte-identical to what UmaDev already writes.

- **Reuse:** `claude_session.rs` almost verbatim — swap the binary to `codebuddy`.
- **Non-interactive tools:** `--dangerously-skip-permissions` (alias `-y`) is required
  alongside `-p` for unattended file/command/network ops.
- **Auth:** the user's existing `codebuddy` login (WeChat / Google / GitHub / Enterprise)
  under `~/.codebuddy`; UmaDev injects nothing. Optional env `CODEBUDDY_AUTH_TOKEN`.
- **Smoke test before shipping:** confirm the NDJSON lands on **stdout** (progress on
  stderr) and that writing a 2nd user line after the 1st `result` yields a 2nd response
  on the same PID.

### Qoder CLI — YES (Claude Code lineage; confirmed against the shipped binary)

Alibaba's Qoder CLI (`qodercli`, distinct from the Qoder IDE) is a Claude-Code-lineage
CLI — the npm tarball bundles Claude Code's print/stream-json/SDK layer. Confirmed by
extracting `@qoder-ai/qodercli@1.0.38` and grepping `bundle/qodercli.js`: the binary
defines `--input-format <format>` and, when `inputFormat==="stream-json"`, reads
line-delimited messages from `process.stdin` via `node:readline` (persistent). It also
has `--session-id`, `-r/--resume [id]`, `-c/--continue`, `--fork-session`,
`--output-format stream-json`, `--include-partial-messages`, `--verbose`, and
`--permission-mode` with the full enum `default|acceptEdits|bypassPermissions|dontAsk|plan|auto`
(**including `plan`**), plus `--yolo`. The CLI SDK documents AsyncIterable multi-turn to
one process (the Claude-Agent-SDK shape). CN docs only document a one-shot subset, but
the CN binary shares this bundle.

- **Reuse:** `claude_session.rs` almost verbatim — binary `qodercli` (intl) /
  `qoderclicn` (CN); non-interactive permission `--permission-mode bypassPermissions`
  (or `--yolo`); resume flag is `-r/--resume [id]` (bracketed optional).
- **Auth:** `QODER_PERSONAL_ACCESS_TOKEN`, or `/login` → `~/.qoder/config.json`; UmaDev
  injects nothing. **Backend is NOT Anthropic** (Qoder's own router: Qwen / DeepSeek /
  GLM / Kimi / MiniMax); it does not honor `ANTHROPIC_BASE_URL`.
- **Smoke test before shipping:** the study confirmed the flag surface by static grep
  but did not execute the binary — run one live `printf '<two stream-json user lines>' |
  qodercli --print --input-format stream-json --output-format stream-json --verbose
  --session-id <uuid> --permission-mode bypassPermissions` to lock the exact output
  field names (`type:"system",subtype:"init",session_id`) before wiring.

### Kimi Code — native NO, config path YES

The current `@moonshot-ai/kimi-code` (TypeScript successor to the wound-down Python
`kimi-cli`) **dropped `--input-format`**. Its `--prompt` is one-shot-per-process and
`--output-format stream-json` is output-only. Its only persistent bidirectional stdio
session is `kimi acp` — **Agent Client Protocol over JSON-RPC**, a different schema from
Claude's NDJSON. So `claude_session.rs` cannot drive native `kimi`; a native live driver
would mean writing a **new ACP/JSON-RPC driver** (codex/opencode-scale work).

**But Kimi K2 runs perfectly under UmaDev's existing `claude-code` driver via Moonshot's
Anthropic-compatible endpoint** — zero new code:

```
ANTHROPIC_BASE_URL=https://api.moonshot.ai/anthropic   # .cn for China
ANTHROPIC_AUTH_TOKEN=<Moonshot API key>                # platform.kimi.ai
ANTHROPIC_MODEL=kimi-k2.7-code
```

`ANTHROPIC_BASE_URL` only changes the HTTPS endpoint the `claude` binary calls
downstream; UmaDev's full bidirectional session (`claude_session.rs`) is preserved
because it is a client-side protocol between UmaDev and the `claude` binary, orthogonal
to which model endpoint the binary talks to. This matches UmaDev's "inject NOTHING; the
base's own config is the brain" contract. **This is the recommended Kimi integration.**

## Implementation blueprint (CodeBuddy + Qoder)

Both are Claude-Code-lineage → parameterize the existing claude driver family rather
than write new protocol code:

1. **Parameterize `ClaudeCodeDriver` + `ClaudeSession`** on an identity/profile:
   `backend_id`, `display_name`, binary resolver + env override, config dir, install /
   login hints, auth-probe file/env, and the non-interactive permission flag. The
   default profile stays `claude-code`; add `codebuddy` and `qoder` profiles. The
   stream-json session logic (`claude_session.rs`) is reused unchanged.
2. **Register the ids:** `umadev_host::BACKEND_IDS` grows from 3 → 5 (`claude-code`,
   `codex`, `opencode`, `codebuddy`, `qoder`); add `BackendArg` variants; update the
   locking tests (`BACKEND_IDS.len()`, `backend_arg_ids_match_host`). Per the repo
   anti-rules, both `BACKEND_IDS` and `BackendArg` must move together.
3. **`driver_for` + `open_session`** route the two new ids to the parameterized
   claude driver / session with the right binary + profile.
4. **i18n** (zh-CN / zh-TW / en) display names + install/login hints; **first-run
   picker** lists the two new bases; **Kimi** shown as a claude-code config note, not a
   separate id.
5. **Smoke tests** (real installs, per §verdict) lock the last-mile before enabling.

Non-goals: a native Kimi ACP driver (deferred — the config path covers Kimi); silently
folding these under `claude-code` (they own their auth + models, so they are distinct
registered ids).

## Sources

CodeBuddy: codebuddy.ai/docs/cli/{headless,reference,cli-reference,sdk-sessions}; npm
`@tencent-ai/codebuddy-code`. Qoder: docs.qoder.com/en/cli/*, help.aliyun.com Lingma
qoderclicn guide, npm `@qoder-ai/qodercli` (v1.0.38 binary grep), QoderAI/qoder-action.
Kimi: moonshotai.github.io/kimi-code command reference, MoonshotAI/kimi-code +
kimi-agent-sdk, platform.kimi.ai/docs/guide/agent-support.
