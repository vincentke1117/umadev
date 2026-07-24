use super::{Duration, Path, WorkspaceSnapshotError};

pub(crate) const MAX_FACT_PATHS: usize = 20;

/// A blocking inability to prove the resident turn satisfied its contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResidentExecutionBlocked {
    pub(crate) note: String,
}

impl ResidentExecutionBlocked {
    /// User-visible terminal failure note.
    pub(crate) fn into_note(self) -> String {
        self.note
    }
}

pub(crate) fn git_commit_blocked(code: &'static str, detail: &str) -> ResidentExecutionBlocked {
    ResidentExecutionBlocked {
        note: format!(
            "[blocked] Git 仅提交契约未通过 [{code}]: {detail}; this turn cannot be marked successful"
        ),
    }
}

pub(crate) fn combined_git_failure(
    code: &'static str,
    primary: &ResidentExecutionBlocked,
    recovery: &ResidentExecutionBlocked,
    detail: &str,
) -> ResidentExecutionBlocked {
    ResidentExecutionBlocked {
        note: format!(
            "[blocked] Git 仅提交契约未通过 [{code}]: {detail}\n原始失败: {}\n恢复失败: {}",
            primary.note, recovery.note
        ),
    }
}

pub(crate) fn git_mutation_timeout() -> Duration {
    std::env::var("UMADEV_GIT_COMMIT_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| seconds.clamp(1, 600))
        .map_or_else(|| Duration::from_secs(120), Duration::from_secs)
}

#[cfg(unix)]
pub(crate) fn same_permissions(left: &std::fs::Permissions, right: &std::fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;
    left.mode() == right.mode()
}

#[cfg(not(unix))]
pub(crate) fn same_permissions(left: &std::fs::Permissions, right: &std::fs::Permissions) -> bool {
    left.readonly() == right.readonly()
}

pub(crate) fn safe_display_path(path: &str) -> String {
    const MAX_PATH_CHARS: usize = 320;
    let mut output = String::new();
    for (index, character) in umadev_agent::base_error::strip_ansi(path)
        .chars()
        .enumerate()
    {
        if index >= MAX_PATH_CHARS {
            output.push('…');
            break;
        }
        output.push(if character.is_control() {
            '�'
        } else {
            character
        });
    }
    output
}

pub(crate) fn display_paths(paths: &[String]) -> String {
    let mut shown = paths
        .iter()
        .take(MAX_FACT_PATHS)
        .map(|path| safe_display_path(path))
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > MAX_FACT_PATHS {
        shown.push_str(&format!(" ... (+{})", paths.len() - MAX_FACT_PATHS));
    }
    shown
}

pub(crate) fn snapshot_blocked(error: WorkspaceSnapshotError) -> ResidentExecutionBlocked {
    ResidentExecutionBlocked {
        note: format!(
            "[blocked] 无法完整核对本轮工作区内容指纹,因此不能标记成功 / unable to \
             verify the complete workspace content fingerprint; this turn cannot be marked \
             successful: {error}"
        ),
    }
}

/// Snapshot the working tree as `git status --porcelain` for legacy reality
/// prompt/fact rendering. Execution-contract enforcement uses the stronger
/// content-fingerprint baseline above.
pub(crate) fn git_status_porcelain(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) fn porcelain_path(line: &str) -> Option<String> {
    let trimmed = line.strip_prefix('\u{feff}').unwrap_or(line);
    if trimmed.trim().is_empty() {
        return None;
    }
    let rest = trimmed.get(3..).unwrap_or("").trim();
    if rest.is_empty() {
        return None;
    }
    let path = rest
        .rsplit(" -> ")
        .next()
        .unwrap_or(rest)
        .trim()
        .trim_matches('"');
    (!path.is_empty()).then(|| path.to_string())
}

/// Diff two legacy porcelain snapshots for transcript fact rendering.
pub(crate) fn changed_files_between(before: &str, after: &str) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};

    let parse = |snapshot: &str| -> BTreeMap<String, String> {
        snapshot
            .lines()
            .filter_map(|line| porcelain_path(line).map(|path| (path, line.trim_end().to_string())))
            .collect()
    };
    let before = parse(before);
    let after = parse(after);
    let mut changed = BTreeSet::new();
    for (path, line) in &after {
        if before.get(path).map(String::as_str) != Some(line.as_str()) {
            changed.insert(path.clone());
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            changed.insert(path.clone());
        }
    }
    changed.into_iter().collect()
}

/// Build the reality-anchored fact line shown after an agentic turn.
pub(crate) fn agentic_fact_line(changed: Option<&[String]>, claimed: bool) -> Option<String> {
    let changed = changed?;
    if changed.is_empty() {
        return Some(if claimed {
            "[note] 本轮无文件变更\n[warn] 底座报告了改动,但工作区没有实际文件变更 —— \
             可能未真正落盘或为复述,请核对 / base reported changes but the working \
             tree is unchanged — verify before trusting"
                .to_string()
        } else {
            "[note] 本轮无文件变更 / no file changes this turn".to_string()
        });
    }
    let mut list = changed
        .iter()
        .take(MAX_FACT_PATHS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if changed.len() > MAX_FACT_PATHS {
        list.push_str(&format!(" ... (+{})", changed.len() - MAX_FACT_PATHS));
    }
    Some(format!("[note] 本轮实际文件变更: {list}"))
}
