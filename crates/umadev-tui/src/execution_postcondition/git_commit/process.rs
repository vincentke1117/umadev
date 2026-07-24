use super::{
    git_commit_blocked, git_output, git_tokio_command, Duration, GitTransactionGuard, Path,
    ResidentExecutionBlocked,
};
use std::process::{Command, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) async fn git_mutating_output(
    root: &Path,
    args: &[&str],
    paths: &[&str],
    timeout: Duration,
    timeout_code: &'static str,
    command_label: &str,
    transaction: &mut GitTransactionGuard,
) -> Result<std::process::Output, ResidentExecutionBlocked> {
    git_mutating_output_inner(
        root,
        args,
        paths,
        GitMutationInvocation {
            input: None,
            timeout,
            timeout_code,
            command_label,
        },
        transaction,
    )
    .await
}

pub(crate) async fn git_mutating_output_with_input(
    root: &Path,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
    timeout_code: &'static str,
    command_label: &str,
    transaction: &mut GitTransactionGuard,
) -> Result<std::process::Output, ResidentExecutionBlocked> {
    git_mutating_output_inner(
        root,
        args,
        &[],
        GitMutationInvocation {
            input: Some(input),
            timeout,
            timeout_code,
            command_label,
        },
        transaction,
    )
    .await
}

struct GitMutationInvocation<'a> {
    input: Option<&'a [u8]>,
    timeout: Duration,
    timeout_code: &'static str,
    command_label: &'a str,
}

async fn git_mutating_output_inner(
    root: &Path,
    args: &[&str],
    paths: &[&str],
    invocation: GitMutationInvocation<'_>,
    transaction: &mut GitTransactionGuard,
) -> Result<std::process::Output, ResidentExecutionBlocked> {
    let GitMutationInvocation {
        input,
        timeout,
        timeout_code,
        command_label,
    } = invocation;
    let mut command = git_tokio_command(root);
    command
        .env("GIT_REFLOG_ACTION", transaction.reflog_action())
        .args(args)
        .args(paths)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command.as_std_mut(), 0);
    let mut child = command.spawn().map_err(|error| {
        git_commit_blocked(
            "git-command-unavailable",
            &format!("无法执行 Git 事务命令 / unable to execute Git transaction: {error}"),
        )
    })?;
    transaction.arm_process(child.id());
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| git_commit_blocked("git-output-invalid", "Git stdout pipe 不可用"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| git_commit_blocked("git-output-invalid", "Git stderr pipe 不可用"))?;
    let stdout_task = tokio::spawn(read_bounded_output_tail(stdout));
    let stderr_task = tokio::spawn(read_bounded_output_tail(stderr));
    if let Some(input) = input {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            git_commit_blocked(
                "git-input-invalid",
                "Git stdin pipe 不可用 / Git stdin unavailable",
            )
        })?;
        let write = async {
            stdin.write_all(input).await?;
            stdin.shutdown().await
        };
        match tokio::time::timeout(timeout, write).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                terminate_git_process_tree(&mut child).await;
                stdout_task.abort();
                stderr_task.abort();
                return Err(git_commit_blocked(
                    "git-input-failed",
                    &format!("无法写入 Git stdin / unable to write Git stdin: {error}"),
                ));
            }
            Err(_) => {
                terminate_git_process_tree(&mut child).await;
                stdout_task.abort();
                stderr_task.abort();
                return Err(git_commit_blocked(
                    timeout_code,
                    &format!(
                        "{command_label} stdin 超过 {} 秒并已终止 / stdin timed out and was terminated",
                        timeout.as_secs_f64(),
                    ),
                ));
            }
        }
    }
    let status = if let Ok(result) = tokio::time::timeout(timeout, child.wait()).await {
        transaction.clear_process();
        result.map_err(|error| {
            git_commit_blocked(
                "git-command-unavailable",
                &format!("无法等待 Git 事务命令 / unable to wait for Git transaction: {error}"),
            )
        })?
    } else {
        terminate_git_process_tree(&mut child).await;
        stdout_task.abort();
        stderr_task.abort();
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        return Err(git_commit_blocked(
            timeout_code,
            &format!(
                "{command_label} 超过 {} 秒并已终止 / command timed out and was terminated",
                timeout.as_secs_f64(),
            ),
        ));
    };
    let stdout = stdout_task
        .await
        .map_err(|error| {
            git_commit_blocked(
                "git-output-invalid",
                &format!("读取 Git stdout 任务失败 / Git stdout task failed: {error}"),
            )
        })?
        .map_err(|error| {
            git_commit_blocked(
                "git-output-invalid",
                &format!("读取 Git stdout 失败 / unable to read Git stdout: {error}"),
            )
        })?;
    let stderr = stderr_task
        .await
        .map_err(|error| {
            git_commit_blocked(
                "git-output-invalid",
                &format!("读取 Git stderr 任务失败 / Git stderr task failed: {error}"),
            )
        })?
        .map_err(|error| {
            git_commit_blocked(
                "git-output-invalid",
                &format!("读取 Git stderr 失败 / unable to read Git stderr: {error}"),
            )
        })?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

pub(crate) async fn read_bounded_output_tail<R>(mut reader: R) -> Result<Vec<u8>, std::io::Error>
where
    R: tokio::io::AsyncRead + Unpin,
{
    const MAX_RETAINED_BYTES: usize = 64 * 1024;
    let mut retained = Vec::with_capacity(MAX_RETAINED_BYTES);
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if read >= MAX_RETAINED_BYTES {
            retained.clear();
            retained.extend_from_slice(&chunk[read - MAX_RETAINED_BYTES..read]);
            continue;
        }
        let overflow = retained
            .len()
            .saturating_add(read)
            .saturating_sub(MAX_RETAINED_BYTES);
        if overflow > 0 {
            retained.drain(..overflow);
        }
        retained.extend_from_slice(&chunk[..read]);
    }
    Ok(retained)
}

pub(crate) async fn terminate_git_process_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        #[cfg(unix)]
        {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                tokio::process::Command::new("kill")
                    .arg("-KILL")
                    .arg("--")
                    .arg(format!("-{pid}"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status(),
            )
            .await;
        }
        #[cfg(windows)]
        {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                tokio::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status(),
            )
            .await;
        }
    }
    let _ = child.kill().await;
}

pub(crate) fn kill_process_group_sync(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg("--")
            .arg(format!("-{pid}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

pub(crate) fn git_required_text(
    root: &Path,
    args: &[&str],
    code: &'static str,
) -> Result<String, ResidentExecutionBlocked> {
    let output = git_output(root, args)?;
    if !output.status.success() {
        return Err(git_command_failed(code, "git", &output));
    }
    let value = String::from_utf8(output.stdout).map_err(|_| {
        git_commit_blocked(
            code,
            "Git 返回了非 UTF-8 输出 / Git returned non-UTF-8 output",
        )
    })?;
    let value = value.trim();
    if value.is_empty() {
        return Err(git_commit_blocked(
            code,
            "Git 返回了空结果 / Git returned an empty result",
        ));
    }
    Ok(value.to_string())
}

pub(crate) fn git_command_failed(
    code: &'static str,
    command: &str,
    output: &std::process::Output,
) -> ResidentExecutionBlocked {
    let detail = bounded_git_stderr(&output.stderr);
    git_commit_blocked(
        code,
        &format!(
            "{command} 执行失败{}{} / command failed; no automatic retry",
            if detail.is_empty() { "" } else { ": " },
            detail
        ),
    )
}

pub(crate) fn bounded_git_stderr(stderr: &[u8]) -> String {
    const MAX_CHARS: usize = 2_000;
    let decoded = String::from_utf8_lossy(stderr);
    let cleaned = umadev_agent::base_error::strip_ansi(&decoded)
        .chars()
        .map(|character| {
            if character == '\n' || character == '\r' || character == '\t' {
                ' '
            } else if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .collect::<String>();
    let count = cleaned.chars().count();
    let tail = cleaned
        .chars()
        .skip(count.saturating_sub(MAX_CHARS))
        .collect::<String>();
    tail.trim().to_string()
}

pub(crate) fn bounded_text(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    value
        .chars()
        .skip(count.saturating_sub(max_chars))
        .collect()
}
