use std::sync::Arc;

use umadev_agent::{ChannelSink, EngineEvent, EventSink};

/// Split a worker-recorded run command like `cd web && npm run dev` into
/// (`working_dir`, `program`, `args`), ready to feed a raw
/// `tokio::process::Command::new(program).args(args)`.
///
/// Windows-aware (mirrors `deploy.rs` / `verify.rs` / `runtime_proof.rs`): the
/// `cd X && <prog> ...` shape routes the bare program through
/// [`umadev_host::spawn_parts`], so a Windows npm/pnpm `.cmd` shim runs via
/// `cmd /c <prog>.cmd ...` instead of failing `CreateProcess` with os error 193;
/// the catch-all fallback shells out via `cmd /c` on Windows and `sh -c` on Unix
/// (Windows has no `sh`). Without this the preview dev-server never booted on
/// Windows — `npm run dev` spawned a non-existent `sh`, and `cd web && npm run
/// dev` spawned a bare `npm` that `CreateProcess` can't find.
pub(super) fn parse_run_command(
    command: &str,
    project_root: &std::path::Path,
) -> (std::path::PathBuf, String, Vec<String>) {
    // Strip a leading `cd <dir> &&` and resolve it relative to the workspace.
    if let Some(after_cd) = command.trim().strip_prefix("cd ") {
        if let Some((dir, rest)) = after_cd.split_once("&&") {
            let dir = dir.trim().trim_matches(|c| c == '\'' || c == '"');
            let resolved = if std::path::Path::new(dir).is_absolute() {
                std::path::PathBuf::from(dir)
            } else {
                project_root.join(dir)
            };
            let rest = rest.trim();
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if let Some((prog, args)) = parts.split_first() {
                // Route the bare program through `spawn_parts` (resolves the real
                // binary + routes a Windows `.cmd`/`.bat` shim through `cmd /c`),
                // then append the original args after whatever lead it produced.
                let (program, mut spawn_args) = umadev_host::spawn_parts(prog);
                spawn_args.extend(args.iter().map(std::string::ToString::to_string));
                return (resolved, program, spawn_args);
            }
        }
    }
    // Fallback: shell out via `cmd /c` (Windows) / `sh -c` (Unix) in the
    // workspace root, so the whole multi-token command runs as written.
    let (shell, shell_arg) = if cfg!(windows) {
        ("cmd", "/c")
    } else {
        ("sh", "-c")
    };
    (
        project_root.to_path_buf(),
        shell.to_string(),
        vec![shell_arg.to_string(), command.to_string()],
    )
}

/// Extract the host:port from a `http://host:port/...` URL, returning None
/// when parsing fails. Used by [`wait_for_port`] so we only open the browser
/// after the dev server is actually accepting connections — not 0ms after
/// spawn, when Vite is still compiling and the page would 404.
pub(super) fn url_host_port(url: &str) -> Option<String> {
    let after_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host_port = after_scheme.split('/').next()?;
    Some(host_port.to_string())
}

/// Poll a `host:port` with a TCP connect until it succeeds or `timeout`
/// elapses. Returns Ok(()) when the dev server is reachable. Mirrors what a
/// browser does — so opening the URL after this returns won't hit a 404 from
/// a half-started server. Runs in the async task so it never blocks the TUI.
pub(super) async fn wait_for_port(url: &str, timeout: std::time::Duration) -> bool {
    let Some(addr) = url_host_port(url) else {
        return false;
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Check whether the port in `url` is currently FREE (nothing listening). We
/// bind to it briefly — if binding fails the port is occupied (by the user's
/// other Vite/Node service), so spawning our dev server would either fail or
/// silently bind a different port while we open the wrong URL. Returning
/// false here tells the caller to NOT spawn and instead hint to the user.
pub(super) fn port_is_free(url: &str) -> bool {
    let Some(addr) = url_host_port(url) else {
        return false; // can't parse → assume not free (conservative)
    };
    std::net::TcpListener::bind(&addr).is_ok()
}

/// Cross-platform best-effort browser open (sync variant for the event loop).
pub(super) fn open_url(url: &str) -> std::io::Result<()> {
    // REAP the launcher on a detached thread. The OS URL-launcher (`open` /
    // `xdg-open` / `cmd start`) hands off to the browser and exits within ms;
    // dropping the `Child` without `wait()` leaves a defunct (zombie) process on
    // Unix that accumulates over every `/preview` / auto-open (P1).
    #[allow(dead_code)]
    fn reap(child: std::process::Child) {
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
    }
    #[cfg(target_os = "macos")]
    {
        reap(std::process::Command::new("open").arg(url).spawn()?);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        reap(std::process::Command::new("xdg-open").arg(url).spawn()?);
    }
    #[cfg(target_os = "windows")]
    {
        reap(
            std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn()?,
        );
    }
    Ok(())
}

/// Start a preview dev server in the background and optionally open its URL once
/// the port is up. Shared by the manual `/preview` ([`Action::StartPreview`]) path
/// and the automatic post-build preview, so both behave identically: the
/// port-conflict guard, the background `wait_for_port` + browser-open, and the
/// `preview_server` child handle (parked for exit-cleanup) are defined exactly
/// once here.
///
/// **Fail-open / non-blocking by contract**: spawning the dev server is
/// best-effort and never blocks the TUI — `wait_for_port` runs in a detached
/// task, a spawn failure only emits a hint, and a busy port opens what is
/// already running instead of starting a second server. The child is stored in
/// `preview_server` so the run-exit cleanup (`run()`) kills it and no process
/// leaks. `open_browser` controls whether the URL is auto-opened in a browser
/// (the manual `/preview` opens it; the automatic post-build preview does NOT —
/// it only surfaces the clickable URL so the build flow never steals focus).
pub(super) fn start_preview_server(
    preview_server: &std::sync::Arc<std::sync::Mutex<Option<tokio::process::Child>>>,
    sink: &Arc<ChannelSink>,
    url: &str,
    command: &str,
    project_root: &std::path::Path,
    open_browser: bool,
) {
    let (dir, prog, args) = parse_run_command(command, project_root);
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(&args)
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    // Detach the preview server into its OWN session (no controlling terminal)
    // so its — or a descendant's — direct /dev/tty writes can't paint over the
    // alt-screen. The unsafe `setsid`/`pre_exec` seam lives in `umadev-agent`
    // because this crate is `#![forbid(unsafe_code)]`. Safe: all three stdio
    // streams are null above. Fail-open.
    umadev_agent::detach_from_controlling_terminal(&mut cmd);
    // Port-conflict guard: if the port is already bound (the user's own
    // Vite/Next/Express), DON'T spawn a second server — it would either fail or
    // bind a different port while we open the wrong URL. Open / surface what's
    // already running instead.
    if port_is_free(url) {
        match cmd.spawn() {
            Ok(child) => {
                if let Ok(mut g) = preview_server.lock() {
                    *g = Some(child);
                }
                sink.emit(EngineEvent::Note(
                    umadev_i18n::tl("preview.dev_starting").into(),
                ));
                let url2 = url.to_string();
                tokio::spawn(async move {
                    let up = wait_for_port(&url2, std::time::Duration::from_secs(15)).await;
                    if up && open_browser {
                        let _ = open_url(&url2);
                    }
                    // Do not append a readiness Note from this detached task. It
                    // can finish after `/clear` or `/resume` and would then write
                    // old-preview output into the replacement conversation. The
                    // synchronous starting note and completion-card URL already
                    // make the preview discoverable without crossing that boundary.
                });
            }
            Err(e) => {
                sink.emit(EngineEvent::Note(umadev_i18n::tlf(
                    "preview.dev_spawn_failed",
                    &[command, &e.to_string(), url],
                )));
            }
        }
    } else {
        if open_browser {
            let _ = open_url(url);
        }
        sink.emit(EngineEvent::Note(umadev_i18n::tlf(
            "preview.port_busy",
            &[url],
        )));
    }
}
