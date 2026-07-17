//! Conservative classification and correlation of streamed tool effects.
//!
//! Verification evidence is accepted only for strict, non-mutating command
//! shapes; arbitrary shell calls remain potential writes.

/// `true` iff a base tool-call NAME mutates the workspace (creates / edits a
/// file) — the signal that turns a chat turn into a build on the light path.
///
/// All five bases normalise their write tools to these names in their stream
/// parsers: three native drivers plus the shared ACP driver emit `Write` for a
/// new file and `Edit` for an in-place change; multi-edit / notebook-edit
/// variants map onto the same family. A `Read` / `Grep` / `Bash` / `Glob` call is NOT a
/// workspace write (a `Bash` may technically write, but the deterministic
/// post-turn git fact-check is the floor for that — we only react to an EXPLICIT
/// file-write tool so a pure read/inspect/answer turn stays light). Case-folded so
/// a base that lower-cases tool names still matches. Pure + cheap.
#[must_use]
pub(super) fn is_workspace_write_tool(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    matches!(
        n.as_str(),
        "write" | "edit" | "multiedit" | "notebookedit" | "create" | "apply_patch" | "applypatch"
    )
}

/// Whether a streamed tool call is a mechanical, targeted verification rather
/// than another edit or a prose claim. This is intentionally conservative: only
/// known test/build/type/lint/check command shapes count. The following successful
/// `ToolResult` is the evidence; merely mentioning a command in final prose never is.
pub(super) fn is_targeted_verification_tool(name: &str, input: &serde_json::Value) -> bool {
    let tool = name.trim().to_ascii_lowercase();
    if ["test", "lint", "typecheck", "check", "build", "compile"].contains(&tool.as_str()) {
        // A structured verifier tool with no shell command is trusted by name.
        // If it does carry a command, apply the same strict parser so a base
        // cannot label `eslint --fix` as `lint` and mint green evidence.
        return input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_none_or(is_strict_verification_command);
    }
    if !matches!(
        tool.as_str(),
        "bash" | "shell" | "exec" | "command" | "powershell"
    ) {
        return false;
    }
    let command = input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    is_strict_verification_command(command)
}

/// Recognise a verifier by its executable + leading subcommand, never by a
/// substring hidden inside arbitrary shell prose. Shell composition/redirection
/// is rejected conservatively: `echo cargo test`, `true # cargo test`, and
/// `cargo test || true` therefore cannot manufacture a green acceptance signal.
pub(super) fn is_strict_verification_command(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty()
        || ["\n", "\r", ";", "&&", "||", "|", "#", "`", "$(`", ">", "<"]
            .iter()
            .any(|needle| command.contains(needle))
    {
        return false;
    }
    let raw: Vec<&str> = command.split_whitespace().collect();
    let mut index = 0usize;
    if raw.first().is_some_and(|token| *token == "env") {
        index += 1;
    }
    while raw
        .get(index)
        .is_some_and(|token| is_shell_assignment(token))
    {
        index += 1;
    }
    let tokens: Vec<String> = raw[index..]
        .iter()
        .map(|token| {
            token
                .trim_matches(['\'', '"'])
                .replace('\\', "/")
                .to_ascii_lowercase()
        })
        .collect();
    let Some(program) = tokens.first().map(String::as_str) else {
        return false;
    };
    if tokens.iter().skip(1).any(|token| {
        verification_flag_matches(token, "--fix")
            || verification_flag_matches(token, "--write")
            || verification_flag_matches(token, "--apply")
            || verification_flag_matches(token, "--update")
            || verification_flag_matches(token, "--update-snapshots")
            || verification_flag_matches(token, "--help")
            || token == "-h"
            || verification_flag_matches(token, "--version")
            || token == "-version"
            || verification_flag_matches(token, "--list")
            || token == "-list"
            || verification_flag_matches(token, "--collect-only")
            || verification_flag_matches(token, "--no-run")
            || verification_flag_matches(token, "--dry-run")
            || verification_flag_matches(token, "--print-config")
            || verification_flag_matches(token, "--show-config")
            || verification_flag_matches(token, "--env-info")
            || verification_flag_matches(token, "--watch")
            || matches!(token.as_str(), "help" | "version" | "list" | "-w")
    }) {
        return false;
    }
    let executable = portable_executable_name(program);
    let arg = |offset: usize| tokens.get(offset).map(String::as_str);
    let package_script = || {
        let script = if arg(1) == Some("run") {
            arg(2)
        } else {
            arg(1)
        };
        script.is_some_and(is_safe_package_verification_script)
    };

    match executable {
        "cargo" => {
            matches!(arg(1), Some("test" | "check" | "clippy" | "build"))
                || (arg(1) == Some("fmt") && arg(2) == Some("--check"))
        }
        "npm" | "pnpm" | "yarn" | "bun" => package_script(),
        "pytest" | "eslint" => true,
        "python" | "python3" | "py" => arg(1) == Some("-m") && arg(2) == Some("pytest"),
        "go" => matches!(arg(1), Some("test" | "vet" | "build")),
        "mvn" | "mvnw" => matches!(arg(1), Some("test" | "verify")),
        "gradle" | "gradlew" => arg(1).is_some_and(|value| value.ends_with("test")),
        "dotnet" | "swift" => arg(1) == Some("test"),
        "tsc" | "vue-tsc" => has_no_emit(&tokens, 1),
        "ruff" | "biome" => arg(1) == Some("check"),
        "prettier" => arg(1) == Some("--check"),
        "git" => arg(1) == Some("diff") && arg(2) == Some("--check"),
        "npx" | "pnpx" | "bunx" => match arg(1).map(portable_executable_name) {
            Some("eslint") => true,
            Some("tsc" | "vue-tsc") => has_no_emit(&tokens, 2),
            Some("ruff" | "biome") => arg(2) == Some("check"),
            Some("prettier") => tokens.iter().skip(2).any(|value| value == "--check"),
            _ => false,
        },
        _ => false,
    }
}

/// Match a safety-sensitive CLI flag in either bare or `--flag=value` form.
/// Prefix matching is intentional for families such as `--fix-dry-run` and
/// `--watch-all`: neither is accepted as terminal verification evidence.
pub(super) fn verification_flag_matches(token: &str, flag: &str) -> bool {
    token == flag
        || token
            .strip_prefix(flag)
            .is_some_and(|suffix| suffix.starts_with('=') || suffix.starts_with('-'))
}

/// Package-manager scripts may encode behavior in the script name rather than
/// flags (`lint:fix`, `test:update`, `test:watch`). Only an exact verifier base
/// or a benign qualified variant such as `test:unit` / `lint:ci` counts.
pub(super) fn is_safe_package_verification_script(name: &str) -> bool {
    let (base, suffix) = name.split_once(':').unwrap_or((name, ""));
    if !matches!(
        base,
        "test" | "lint" | "build" | "check" | "typecheck" | "type-check"
    ) {
        return false;
    }
    !suffix
        .split([':', '-', '_', '/', '.'])
        .filter(|part| !part.is_empty())
        .any(|part| {
            [
                "fix", "write", "update", "watch", "snapshot", "snap", "bless",
            ]
            .iter()
            .any(|unsafe_stem| part.starts_with(unsafe_stem))
        })
}

/// Require an explicitly enabled no-emit mode. `--noEmit false` and
/// `--noEmit=false` are not verification-only because they permit writes.
pub(super) fn has_no_emit(tokens: &[String], start: usize) -> bool {
    tokens.iter().enumerate().skip(start).any(|(index, token)| {
        (token == "--noemit"
            && tokens
                .get(index + 1)
                .is_none_or(|next| next.as_str() != "false"))
            || token == "--noemit=true"
    })
}

pub(super) fn is_shell_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
}

/// Return a normalized executable basename across Unix and Windows command
/// spellings. Windows launchers commonly surface `cargo.exe`, `npm.cmd`, or
/// `gradlew.bat`; treating those as different programs would make the same green
/// verification count on macOS/Linux but fail on Windows.
pub(super) fn portable_executable_name(program: &str) -> &str {
    let basename = program.rsplit('/').next().unwrap_or(program);
    [".exe", ".cmd", ".bat", ".ps1"]
        .iter()
        .find_map(|suffix| basename.strip_suffix(suffix))
        .unwrap_or(basename)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObservedToolEffect {
    Verification,
    PotentialWrite,
    Neutral,
}

#[derive(Debug, Default)]
pub(super) struct ToolEffectTracker {
    legacy: std::collections::VecDeque<ObservedToolEffect>,
    correlated: std::collections::HashMap<String, ObservedToolEffect>,
}

impl ToolEffectTracker {
    pub(super) fn start(&mut self, call_id: Option<&str>, effect: ObservedToolEffect) {
        if let Some(call_id) = call_id {
            self.correlated.insert(call_id.to_owned(), effect);
        } else {
            self.legacy.push_back(effect);
        }
    }

    pub(super) fn finish(&mut self, call_id: Option<&str>) -> Option<ObservedToolEffect> {
        match call_id {
            Some(call_id) => self.correlated.remove(call_id),
            None => self.legacy.pop_front(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.legacy.clear();
        self.correlated.clear();
    }
}

pub(super) fn observed_tool_effect(name: &str, input: &serde_json::Value) -> ObservedToolEffect {
    if is_targeted_verification_tool(name, input) {
        return ObservedToolEffect::Verification;
    }
    if is_workspace_write_tool(name) {
        return ObservedToolEffect::PotentialWrite;
    }
    let tool = name.trim().to_ascii_lowercase();
    if !matches!(
        tool.as_str(),
        "bash" | "shell" | "exec" | "command" | "powershell"
    ) {
        return ObservedToolEffect::Neutral;
    }
    let command = input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if is_strict_read_only_command(command) {
        ObservedToolEffect::Neutral
    } else {
        // With no call id or filesystem journal, an arbitrary shell command must
        // be treated as possibly mutating. This is conservative by design.
        ObservedToolEffect::PotentialWrite
    }
}

pub(super) fn is_strict_read_only_command(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty()
        || ["\n", "\r", ";", "&&", "||", "|", "#", "`", "$(`", ">", "<"]
            .iter()
            .any(|needle| command.contains(needle))
    {
        return false;
    }
    let tokens: Vec<String> = command
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches(['\'', '"'])
                .replace('\\', "/")
                .to_ascii_lowercase()
        })
        .collect();
    let Some(program) = tokens.first().map(String::as_str) else {
        return false;
    };
    let executable = portable_executable_name(program);
    match executable {
        "pwd" | "ls" | "dir" | "cat" | "head" | "tail" | "wc" | "which" | "where" | "rg"
        | "grep" => true,
        "git" => matches!(
            tokens.get(1).map(String::as_str),
            Some("status" | "diff" | "log" | "show")
        ),
        _ => false,
    }
}
