use super::{
    git_command_failed, git_commit_blocked, git_std_command, remove_git_environment_overrides,
    BTreeSet, Path, ResidentExecutionBlocked,
};
use std::process::Command;

pub(crate) fn git_output_without_filter_programs(
    root: &Path,
    args: &[&str],
) -> Result<std::process::Output, ResidentExecutionBlocked> {
    let overrides = configured_filter_overrides(root)?;
    let mut command = git_std_command(root);
    for value in &overrides {
        command.arg("-c").arg(value);
    }
    command.args(args).output().map_err(|error| {
        git_commit_blocked(
            "git-command-unavailable",
            &format!("无法执行隔离的 Git 验证命令 / unable to execute isolated Git verification: {error}"),
        )
    })
}

fn configured_filter_overrides(root: &Path) -> Result<Vec<String>, ResidentExecutionBlocked> {
    let mut command = Command::new("git");
    command
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(root)
        .args(["config", "--null", "--list", "--includes"]);
    remove_git_environment_overrides(&mut command);
    let output = command.output().map_err(|error| {
        git_commit_blocked(
            "git-config-unverifiable",
            &format!(
                "无法枚举 Git filter 配置 / unable to enumerate Git filter configuration: {error}"
            ),
        )
    })?;
    if !output.status.success() {
        return Err(git_command_failed(
            "git-config-unverifiable",
            "git config --list",
            &output,
        ));
    }
    if output.stdout.len() > 512 * 1024 {
        return Err(git_commit_blocked(
            "git-config-invalid",
            "Git config 过大,无法安全枚举 filter / Git configuration is too large to inspect safely",
        ));
    }
    let mut drivers = BTreeSet::new();
    for record in output.stdout.split(|byte| *byte == 0) {
        let Some(separator) = record.iter().position(|byte| *byte == b'\n') else {
            continue;
        };
        let key = std::str::from_utf8(&record[..separator]).map_err(|_| {
            git_commit_blocked(
                "git-config-invalid",
                "Git config key 不是 UTF-8 / Git config key is not UTF-8",
            )
        })?;
        let lower = key.to_ascii_lowercase();
        let Some(lower_rest) = lower.strip_prefix("filter.") else {
            continue;
        };
        let rest = &key["filter.".len()..];
        for suffix in [".clean", ".smudge", ".process", ".required"] {
            if lower_rest.ends_with(suffix) {
                let driver = &rest[..rest.len() - suffix.len()];
                if driver.is_empty()
                    || driver.len() > 256
                    || driver.contains('=')
                    || driver.chars().any(char::is_control)
                {
                    return Err(git_commit_blocked(
                        "git-config-invalid",
                        "Git filter 名称无效 / invalid Git filter driver name",
                    ));
                }
                drivers.insert(driver.to_string());
                if drivers.len() > 256 {
                    return Err(git_commit_blocked(
                        "git-config-invalid",
                        "Git filter 数量过多 / too many Git filter drivers",
                    ));
                }
            }
        }
    }
    let mut overrides = Vec::with_capacity(drivers.len().saturating_mul(4));
    for driver in drivers {
        overrides.push(format!("filter.{driver}.clean="));
        overrides.push(format!("filter.{driver}.smudge="));
        overrides.push(format!("filter.{driver}.process="));
        overrides.push(format!("filter.{driver}.required=false"));
    }
    Ok(overrides)
}
