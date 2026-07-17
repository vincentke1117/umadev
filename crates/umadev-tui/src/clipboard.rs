//! Native clipboard routing and line-ending normalization.
//!
//! The transcript selection layer decides *what* to copy; this module owns the
//! platform boundary for local native commands and the remote/tmux routing
//! signals. All process failures are best-effort and never block the TUI.

/// Whether this is a remote session where a native clipboard command would
/// target the far host instead of the user's terminal.
pub(crate) fn clipboard_is_remote() -> bool {
    clipboard_remote_from_env(
        std::env::var_os("SSH_CONNECTION").is_some(),
        std::env::var_os("SSH_TTY").is_some(),
    )
}

pub(crate) fn clipboard_remote_from_env(ssh_connection: bool, _ssh_tty: bool) -> bool {
    // `SSH_CONNECTION` is authoritative. A locally reattached tmux pane may
    // retain stale `SSH_TTY`, which must not disable the native clipboard.
    ssh_connection
}

/// Whether OSC 52 needs tmux DCS passthrough to reach the outer terminal.
pub(crate) fn clipboard_in_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

/// Copy `text` with the native OS command. Intended for a blocking worker so a
/// wedged platform command can never stall input or rendering.
pub(crate) fn copy_to_clipboard_native(text: &str) -> bool {
    let plan = native_clipboard_plan(std::env::consts::OS);
    let text = normalize_clipboard_newlines(text, matches!(plan, NativeClipboardPlan::Windows));
    match plan {
        NativeClipboardPlan::Windows => copy_to_clipboard_windows(&text),
        NativeClipboardPlan::Macos => try_native_clipboard("pbcopy", &[], &text),
        NativeClipboardPlan::UnixLike => {
            try_native_clipboard("wl-copy", &[], &text)
                || try_native_clipboard("xclip", &["-selection", "clipboard"], &text)
                || try_native_clipboard("xsel", &["--clipboard", "--input"], &text)
        }
    }
}

/// Normalize the internal LF representation at the native-OS boundary. Bare CR
/// is treated as a line break and an existing CRLF pair is never doubled.
pub(crate) fn normalize_clipboard_newlines(text: &str, windows: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                if windows {
                    out.push_str("\r\n");
                } else {
                    out.push('\n');
                }
            }
            '\n' => {
                if windows {
                    out.push_str("\r\n");
                } else {
                    out.push('\n');
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeClipboardPlan {
    Windows,
    Macos,
    UnixLike,
}

pub(crate) fn native_clipboard_plan(os: &str) -> NativeClipboardPlan {
    match os {
        "windows" => NativeClipboardPlan::Windows,
        "macos" => NativeClipboardPlan::Macos,
        _ => NativeClipboardPlan::UnixLike,
    }
}

fn try_native_clipboard(cmd: &str, args: &[&str], text: &str) -> bool {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let Ok(mut child) = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn clipboard_temp_path() -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "umadev-clipboard-{}-{stamp}.txt",
        std::process::id()
    ))
}

#[cfg(windows)]
fn copy_to_clipboard_windows(text: &str) -> bool {
    use std::process::{Command, Stdio};

    // `clip.exe` uses the active console code page and can corrupt CJK. Prefer
    // PowerShell reading an explicit UTF-8 file, with clip.exe as fallback.
    let path = clipboard_temp_path();
    if std::fs::write(&path, text.as_bytes()).is_ok() {
        let ok = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Set-Clipboard -Value (Get-Content -LiteralPath $args[0] -Raw -Encoding UTF8)",
            ])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        let _ = std::fs::remove_file(&path);
        if ok {
            return true;
        }
    }
    try_native_clipboard("clip.exe", &[], text)
}

#[cfg(not(windows))]
fn copy_to_clipboard_windows(_text: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_plan_never_routes_windows_to_unix_commands() {
        assert_eq!(
            native_clipboard_plan("windows"),
            NativeClipboardPlan::Windows
        );
        assert_eq!(native_clipboard_plan("macos"), NativeClipboardPlan::Macos);
        assert_eq!(
            native_clipboard_plan("linux"),
            NativeClipboardPlan::UnixLike
        );
        assert_eq!(
            native_clipboard_plan("freebsd"),
            NativeClipboardPlan::UnixLike
        );
    }

    #[test]
    fn newlines_match_each_platform_without_doubling_crlf() {
        let mixed = "第一行\r\nsecond\rthird\n";
        assert_eq!(
            normalize_clipboard_newlines(mixed, false),
            "第一行\nsecond\nthird\n"
        );
        assert_eq!(
            normalize_clipboard_newlines(mixed, true),
            "第一行\r\nsecond\r\nthird\r\n"
        );
    }

    #[test]
    fn remote_detection_ignores_a_stale_ssh_tty() {
        assert!(clipboard_remote_from_env(true, true));
        assert!(clipboard_remote_from_env(true, false));
        assert!(!clipboard_remote_from_env(false, true));
        assert!(!clipboard_remote_from_env(false, false));
    }
}
