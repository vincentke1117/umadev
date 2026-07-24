use super::{
    display_paths, git_command_failed, git_commit_blocked, Path, PathBuf, ResidentExecutionBlocked,
};
use std::ffi::OsStr;
use std::process::Command;

const GIT_ENVIRONMENT_OVERRIDES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_SHALLOW_FILE",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_ATTR_NOSYSTEM",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
];

#[cfg(windows)]
const EMPTY_GIT_CONFIG: &str = "NUL";
#[cfg(not(windows))]
const EMPTY_GIT_CONFIG: &str = "/dev/null";
#[cfg(windows)]
const INERT_HOOKS_CONFIG: &str = "core.hooksPath=NUL";
#[cfg(not(windows))]
const INERT_HOOKS_CONFIG: &str = "core.hooksPath=/dev/null";
#[cfg(windows)]
const EMPTY_ATTRIBUTES_CONFIG: &str = "core.attributesFile=NUL";
#[cfg(not(windows))]
const EMPTY_ATTRIBUTES_CONFIG: &str = "core.attributesFile=/dev/null";

fn git_environment_override(key: &OsStr) -> bool {
    let key = key.to_string_lossy();
    GIT_ENVIRONMENT_OVERRIDES
        .iter()
        .any(|candidate| key.eq_ignore_ascii_case(candidate))
        || key.to_ascii_uppercase().starts_with("GIT_CONFIG_KEY_")
        || key.to_ascii_uppercase().starts_with("GIT_CONFIG_VALUE_")
}

fn git_environment_variable(key: &OsStr) -> bool {
    key.to_string_lossy()
        .to_ascii_uppercase()
        .starts_with("GIT_")
}

pub(crate) fn remove_git_environment_overrides(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if git_environment_variable(&key)
            || matches!(
                key.to_str(),
                Some(
                    "GIT_AUTHOR_NAME"
                        | "GIT_AUTHOR_EMAIL"
                        | "GIT_COMMITTER_NAME"
                        | "GIT_COMMITTER_EMAIL"
                        | "EMAIL"
                )
            )
        {
            command.env_remove(key);
        }
    }
}

fn isolate_git_configuration(command: &mut Command) {
    remove_git_environment_overrides(command);
    command
        .env("GIT_CONFIG", EMPTY_GIT_CONFIG)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", EMPTY_GIT_CONFIG)
        .env("GIT_CONFIG_SYSTEM", EMPTY_GIT_CONFIG)
        .env("GIT_ATTR_NOSYSTEM", "1");
}

pub(crate) fn reject_git_environment_redirects() -> Result<(), ResidentExecutionBlocked> {
    let mut overrides = std::env::vars_os()
        .filter_map(|(key, _)| git_environment_override(&key).then(|| key.to_string_lossy().into()))
        .collect::<Vec<String>>();
    overrides.sort();
    overrides.dedup();
    if overrides.is_empty() {
        Ok(())
    } else {
        Err(git_commit_blocked(
            "git-environment-override-blocked",
            &format!(
                "检测到会改变仓库、index、配置或 pathspec 语义的 Git 环境变量: {} / repository-redirecting Git environment variables are not allowed in the host-only transaction",
                display_paths(&overrides)
            ),
        ))
    }
}

pub(crate) fn git_std_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("--literal-pathspecs")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            EMPTY_ATTRIBUTES_CONFIG,
            "-c",
            "commit.gpgSign=false",
            "-c",
            "gc.auto=0",
        ])
        .arg("-C")
        .arg(root);
    isolate_git_configuration(&mut command);
    command
}

pub(crate) fn git_tokio_command(root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("--no-pager")
        .arg("--literal-pathspecs")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            INERT_HOOKS_CONFIG,
            "-c",
            EMPTY_ATTRIBUTES_CONFIG,
            "-c",
            "commit.gpgSign=false",
            "-c",
            "gc.auto=0",
        ])
        .arg("-C")
        .arg(root);
    isolate_git_configuration(command.as_std_mut());
    command
}

pub(crate) fn git_identity_config(
    root: &Path,
) -> Result<(String, String), ResidentExecutionBlocked> {
    let value = |key: &str| -> Result<String, ResidentExecutionBlocked> {
        let mut command = Command::new("git");
        command
            .arg("--literal-pathspecs")
            .arg("-C")
            .arg(root)
            .args(["config", "--get", key]);
        remove_git_environment_overrides(&mut command);
        let output = command.output().map_err(|error| {
            git_commit_blocked(
                "git-identity-unverifiable",
                &format!("无法读取 Git 身份配置 / unable to read Git identity: {error}"),
            )
        })?;
        if !output.status.success() {
            return Err(git_commit_blocked(
                "git-identity-missing",
                "Git user.name 和 user.email 必须已配置 / Git user.name and user.email must be configured",
            ));
        }
        let value = String::from_utf8(output.stdout)
            .map_err(|_| {
                git_commit_blocked(
                    "git-identity-invalid",
                    "Git 身份配置不是有效 UTF-8 / Git identity is not valid UTF-8",
                )
            })?
            .trim()
            .to_string();
        if value.is_empty()
            || value.len() > 512
            || value.chars().any(char::is_control)
            || value.contains('<')
            || value.contains('>')
        {
            return Err(git_commit_blocked(
                "git-identity-invalid",
                "Git 身份配置包含不安全字符 / Git identity contains unsafe characters",
            ));
        }
        Ok(value)
    };
    Ok((value("user.name")?, value("user.email")?))
}

pub(crate) fn configured_git_path(
    root: &Path,
    key: &str,
) -> Result<Option<PathBuf>, ResidentExecutionBlocked> {
    let mut command = Command::new("git");
    command
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(root)
        .args(["config", "--path", "--get", key]);
    remove_git_environment_overrides(&mut command);
    let output = command.output().map_err(|error| {
        git_commit_blocked(
            "git-config-unverifiable",
            &format!("无法读取 Git 路径配置 / unable to read Git path configuration: {error}"),
        )
    })?;
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(git_command_failed(
            "git-config-unverifiable",
            "git config --path --get",
            &output,
        ));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| {
            git_commit_blocked(
                "git-config-invalid",
                "Git 路径配置不是有效 UTF-8 / Git path configuration is not valid UTF-8",
            )
        })?
        .trim()
        .to_string();
    if value.is_empty() || value.len() > 4_096 || value.chars().any(char::is_control) {
        return Err(git_commit_blocked(
            "git-config-invalid",
            "Git 路径配置包含不安全字符 / Git path configuration contains unsafe characters",
        ));
    }
    Ok(Some(PathBuf::from(value)))
}

pub(crate) fn git_output(
    root: &Path,
    args: &[&str],
) -> Result<std::process::Output, ResidentExecutionBlocked> {
    git_std_command(root).args(args).output().map_err(|error| {
        git_commit_blocked(
            "git-command-unavailable",
            &format!("无法执行 Git 验证命令 / unable to execute Git verification: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        git_environment_override, git_environment_variable, git_std_command, EMPTY_GIT_CONFIG,
    };
    use std::ffi::OsStr;
    use std::path::Path;

    #[test]
    fn repository_index_and_pathspec_overrides_are_all_fail_closed() {
        for key in [
            "GIT_DIR",
            "GIT_INDEX_FILE",
            "GIT_LITERAL_PATHSPECS",
            "GIT_GLOB_PATHSPECS",
            "GIT_NOGLOB_PATHSPECS",
            "GIT_ICASE_PATHSPECS",
        ] {
            assert!(
                git_environment_override(OsStr::new(key)),
                "{key} must never influence a host-owned commit"
            );
        }
    }

    #[test]
    fn ancestry_discovery_and_dynamic_config_overrides_are_fail_closed() {
        for key in [
            "GIT_SHALLOW_FILE",
            "GIT_GRAFT_FILE",
            "GIT_REPLACE_REF_BASE",
            "GIT_NO_REPLACE_OBJECTS",
            "GIT_CEILING_DIRECTORIES",
            "GIT_DISCOVERY_ACROSS_FILESYSTEM",
            "GIT_CONFIG_KEY_0",
            "git_config_value_17",
        ] {
            assert!(
                git_environment_override(OsStr::new(key)),
                "{key} must never alter repository interpretation"
            );
        }
        assert!(!git_environment_override(OsStr::new("LANG")));
    }

    #[test]
    fn every_git_environment_variable_is_removed_from_children() {
        for key in [
            "GIT_TRACE",
            "GIT_TRACE2_EVENT",
            "GIT_EXTERNAL_DIFF",
            "GIT_PAGER",
            "GIT_ASKPASS",
            "GIT_OPTIONAL_LOCKS",
        ] {
            assert!(
                git_environment_variable(OsStr::new(key)),
                "{key} must not leak into host-owned Git children"
            );
        }
        assert!(!git_environment_variable(OsStr::new("LANG")));
    }

    #[test]
    fn every_git_child_uses_an_empty_config_instead_of_repo_or_global_programs() {
        let command = git_std_command(Path::new("."));
        let config = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == OsStr::new("GIT_CONFIG")).then(|| value.map(OsStr::to_owned))
            })
            .flatten();
        assert_eq!(config.as_deref(), Some(OsStr::new(EMPTY_GIT_CONFIG)));
    }
}
