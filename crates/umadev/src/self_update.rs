//! `umadev update` for installs that **no JS package manager owns** — a
//! `cargo install`, a release asset someone downloaded by hand, a binary copied
//! into `/usr/local/bin`.
//!
//! An npm/pnpm/yarn/bun install is upgraded by the JS shim (`npm/umadev/bin/cli.js`),
//! which intercepts `update` and never launches the binary: the binary lives inside
//! the very tree the package manager is about to replace, and on Windows a running
//! `.exe` cannot be unlinked (EPERM), so the manager leaves the renamed tree behind
//! forever. This module covers everything the shim cannot: it resolves the latest
//! GitHub Release, downloads **this** platform's asset, verifies it, and swaps it
//! over the running binary atomically.
//!
//! **HTTP client:** `reqwest` — already linked into this binary through
//! `umadev-host` (a non-optional dependency there), so using it here adds *zero*
//! new code, no new TLS stack, and no new transitive crates. It is preferred over
//! shelling out to `curl` / `Invoke-WebRequest` because neither is guaranteed to
//! exist (minimal Linux images ship without curl; PowerShell execution policy can
//! block the Windows form), and a subprocess would make the download's progress and
//! failure modes opaque.
//!
//! **Safety contract:** the download is written to a temp file *next to* the
//! current exe (same filesystem, so the final `rename` is atomic) and fully
//! verified — HTTP 200, non-trivial size, correct executable magic for this
//! platform — **before** anything touches the installed binary. Any failure at any
//! step leaves the existing binary byte-for-byte untouched and prints the exact
//! manual command. The user can never be left with a half-written binary.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// GitHub Releases API for the repo the release workflow publishes to
/// (`.github/workflows/release.yml` uploads `dist/*` on every `v*` tag).
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/umacloud/umadev/releases/latest";

/// The releases page, printed whenever an automatic update cannot proceed.
const RELEASES_PAGE: &str = "https://github.com/umacloud/umadev/releases";

/// Smallest plausible size, in bytes, of a real `umadev` release binary. The
/// shipped binary is tens of MB; anything under 1 MiB is a truncated download, an
/// error page, or a redirect stub — never a usable executable.
const MIN_BINARY_BYTES: u64 = 1_048_576;

/// Timeout for the (tiny) GitHub Releases API call.
const API_TIMEOUT: Duration = Duration::from_secs(20);

/// Timeout for the whole asset download. Generous: the binary is tens of MB and a
/// slow link must not be cut off mid-update.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(900);

/// Where this binary lives — which decides how (or whether) it can update itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// Under a `node_modules` tree: a JS package manager owns it, and only that
    /// manager may replace it. Handled by the shim, never by this module.
    PackageManaged,
    /// A build inside a cargo `target/` dir — i.e. someone running this repo.
    /// Overwriting it would fight `cargo build`; print guidance instead.
    DevBuild,
    /// Everything else: `cargo install`, a downloaded release asset, a binary
    /// copied onto `PATH` by hand. This is the case this module actually updates.
    Standalone,
}

/// Classify an installed `umadev` binary by its path alone (pure — no I/O).
pub fn classify_install(exe: &Path) -> InstallKind {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.iter().any(|p| p == "node_modules") {
        return InstallKind::PackageManaged;
    }
    if is_cargo_target_build(&parts) {
        return InstallKind::DevBuild;
    }
    InstallKind::Standalone
}

/// Does this path look like `…/target/<profile>/umadev` or
/// `…/target/<triple>/<profile>/umadev` — i.e. a cargo build output, not an
/// install? A `cargo install` binary lands in `~/.cargo/bin`, which has no
/// `target/` component, so it is correctly *not* matched.
fn is_cargo_target_build(parts: &[String]) -> bool {
    for (i, part) in parts.iter().enumerate() {
        if part != "target" {
            continue;
        }
        // `target/<profile>/bin` and `target/<triple>/<profile>/bin`.
        for offset in [1_usize, 2] {
            let is_profile = parts
                .get(i + offset)
                .is_some_and(|p| p == "debug" || p == "release");
            if is_profile && parts.len() > i + offset + 1 {
                return true;
            }
        }
    }
    false
}

/// Name of the GitHub Release asset for a given `std::env::consts::OS` /
/// `ARCH` pair, or `None` when we publish no binary for that platform.
///
/// This MUST stay in lockstep with `.github/workflows/release.yml`, which stages
/// each build as `dist/umadev-<target-triple><ext>` and uploads `dist/*`.
pub fn release_asset_name(os: &str, arch: &str) -> Option<String> {
    let target = match (os, arch) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        // Windows on ARM runs x64 binaries under built-in emulation, and we publish
        // no arm64 Windows build — reuse the x64 asset, exactly as the npm shim's
        // PLATFORM_PACKAGES maps `win32-arm64` to `@umacloud/cli-win32-x64`.
        ("windows", "x86_64" | "aarch64") => "x86_64-pc-windows-msvc",
        _ => return None,
    };
    let ext = if os == "windows" { ".exe" } else { "" };
    Some(format!("umadev-{target}{ext}"))
}

/// Parse a `major.minor.patch` version, tolerating a leading `v` and ignoring any
/// pre-release / build suffix. `None` when it is not a plain three-part version.
fn semver_triple(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Is the published `latest` release actually newer than what is installed?
///
/// Equal versions are **not** newer (that is the "already on the latest version"
/// short-circuit), and a *local* build that is ahead of the release (a dev machine
/// mid-release) is not downgraded. If either side is not a plain semver, fall back
/// to string inequality — an unknown-but-different tag is treated as an update.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (semver_triple(latest), semver_triple(current)) {
        (Some(l), Some(c)) => l > c,
        _ => latest.trim().trim_start_matches('v') != current.trim().trim_start_matches('v'),
    }
}

/// Does the downloaded blob actually start like a native executable for `os`?
///
/// The release publishes **no checksum file** (`release.yml` uploads the raw
/// binaries and the embedding-model assets, nothing else), so this magic-byte check
/// plus the size floor is the strongest integrity gate available without inventing
/// a checksum the release does not produce. It reliably rejects the realistic
/// failure: an HTML error / captive-portal page served with a 200.
/// Unknown platforms fail open (`true`) — we never publish for them anyway.
pub fn looks_like_executable(os: &str, head: &[u8]) -> bool {
    match os {
        // PE/COFF (`MZ`).
        "windows" => head.starts_with(b"MZ"),
        // ELF.
        "linux" => head.starts_with(b"\x7fELF"),
        // Mach-O 64-bit (LE/BE) or a universal ("fat") binary.
        "macos" => {
            head.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
                || head.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
                || head.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
                || head.starts_with(&[0xbe, 0xba, 0xfe, 0xca])
        }
        _ => true,
    }
}

/// The path a running binary is renamed **aside** to on Windows: `<exe>.old`
/// (appended, so `umadev.exe` → `umadev.exe.old`).
pub fn backup_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(".old");
    PathBuf::from(name)
}

/// One filesystem move in the binary swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapStep {
    /// Move the running exe out of the way. Windows only: a mapped `.exe` cannot be
    /// *unlinked*, but it **can** be *renamed* — so the new binary can take its
    /// place while the old image keeps running. The leftover is swept next launch.
    RenameAside {
        /// The running binary.
        from: PathBuf,
        /// Its `<exe>.old` parking spot.
        to: PathBuf,
    },
    /// Move the verified download into place. On Unix this single `rename` IS the
    /// update: replacing a running binary is legal, the running process keeps its
    /// already-mapped inode.
    Install {
        /// The verified temp file (same filesystem as the exe).
        from: PathBuf,
        /// The installed binary path.
        to: PathBuf,
    },
}

/// The ordered moves that replace `exe` with the verified download at `tmp`.
///
/// `windows` is a parameter (not `cfg!`) so both plans stay unit-testable from any
/// host.
pub fn swap_plan(exe: &Path, tmp: &Path, windows: bool) -> Vec<SwapStep> {
    let mut steps = Vec::new();
    if windows {
        steps.push(SwapStep::RenameAside {
            from: exe.to_path_buf(),
            to: backup_path(exe),
        });
    }
    steps.push(SwapStep::Install {
        from: tmp.to_path_buf(),
        to: exe.to_path_buf(),
    });
    steps
}

/// Execute a [`swap_plan`]. Every move is a same-filesystem `rename`, so each step
/// is atomic; the download was already verified, so nothing here can produce a
/// half-written binary.
fn apply_swap(steps: &[SwapStep]) -> Result<()> {
    for step in steps {
        match step {
            SwapStep::RenameAside { from, to } => {
                // A `<exe>.old` from a previous update may still exist (the sweep is
                // best-effort). Windows `rename` will not clobber it, so drop it first.
                let _ = std::fs::remove_file(to);
                std::fs::rename(from, to).with_context(|| {
                    format!(
                        "could not move the running binary aside to {}",
                        to.display()
                    )
                })?;
            }
            SwapStep::Install { from, to } => {
                std::fs::rename(from, to).with_context(|| {
                    format!("could not install the new binary at {}", to.display())
                })?;
            }
        }
    }
    Ok(())
}

/// Delete a `<exe>.old` left behind by a Windows self-update. Called once at
/// startup; strictly best-effort and never fails the launch (the old image may
/// still be mapped by a process that is exiting — the next launch retries).
pub fn sweep_stale_backup() {
    if !cfg!(windows) {
        return; // the Unix swap never creates one
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(backup_path(&exe));
    }
}

/// Tag + assets of the latest GitHub Release — only the fields the updater needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestRelease {
    /// The release tag, e.g. `v1.0.41`.
    pub tag: String,
    /// `(asset name, browser_download_url)` for every asset on the release.
    pub assets: Vec<(String, String)>,
}

impl LatestRelease {
    /// The download URL of the asset named `name`, if the release published it.
    pub fn asset_url(&self, name: &str) -> Option<&str> {
        self.assets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, u)| u.as_str())
    }
}

/// Parse the GitHub Releases API `latest` payload. Pure, so the whole
/// release-resolution path is testable without touching the network.
pub fn parse_latest_release(body: &str) -> Result<LatestRelease> {
    let v: serde_json::Value = serde_json::from_str(body)
        .context("the release API returned something that is not JSON")?;
    let tag = v
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .context("the release API returned no tag_name")?
        .to_string();
    let assets = v
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let name = a.get("name")?.as_str()?.to_string();
                    let url = a.get("browser_download_url")?.as_str()?.to_string();
                    Some((name, url))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(LatestRelease { tag, assets })
}

/// Every URL worth trying for one release asset, in order: GitHub itself, then the
/// community GitHub mirrors that make release assets reachable from mainland China
/// (the same fallback chain the npm shim already uses for the embedding model).
pub fn download_urls(browser_url: &str) -> Vec<String> {
    vec![
        browser_url.to_string(),
        format!("https://ghproxy.net/{browser_url}"),
        format!("https://ghfast.top/{browser_url}"),
    ]
}

/// Fetch + parse the latest release from the GitHub API.
async fn fetch_latest_release() -> Result<LatestRelease> {
    let client = reqwest::Client::builder()
        .user_agent("umadev-cli")
        .timeout(API_TIMEOUT)
        .build()
        .context("could not build an HTTP client")?;
    let resp = client
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("could not reach the GitHub release API")?;
    if !resp.status().is_success() {
        bail!("the GitHub release API returned HTTP {}", resp.status());
    }
    let body = resp
        .text()
        .await
        .context("could not read the GitHub release API response")?;
    parse_latest_release(&body)
}

/// Stream one URL to `dest`, with a coarse progress line on stderr. Returns the
/// number of bytes written. The caller verifies the result *before* installing it.
async fn download_to(client: &reqwest::Client, url: &str, dest: &Path) -> Result<u64> {
    use std::io::Write as _;

    let mut resp = client
        .get(url)
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;
    if !resp.status().is_success() {
        bail!("HTTP {} from {url}", resp.status());
    }
    let total = resp.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(dest).with_context(|| {
        format!(
            "could not write next to the installed binary ({}) — is the install dir writable?",
            dest.display()
        )
    })?;
    let mut got: u64 = 0;
    let mut last_report: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .with_context(|| format!("the download from {url} was interrupted"))?
    {
        file.write_all(&chunk)
            .with_context(|| format!("could not write to {}", dest.display()))?;
        got += chunk.len() as u64;
        // One line per ~4 MiB — enough to show life on a slow link, quiet enough for
        // a piped/CI log. Integer math throughout: a byte count is exact in u64 and
        // an approximate percentage needs no float.
        if got - last_report >= 4 * 1_048_576 {
            last_report = got;
            let mb = |b: u64| b / 1_048_576;
            // `checked_div` is None exactly when the server sent no Content-Length
            // (total == 0) — then report bytes only, with no percentage.
            if let Some(pct) = got.saturating_mul(100).checked_div(total) {
                eprint!("\r  downloading  {pct}%  ({}/{} MB)   ", mb(got), mb(total));
            } else {
                eprint!("\r  downloading  {} MB   ", mb(got));
            }
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
    }
    file.flush()
        .with_context(|| format!("could not flush {}", dest.display()))?;
    drop(file);
    if last_report > 0 {
        eprintln!();
    }
    Ok(got)
}

/// Size + magic-byte gate on a finished download. Runs BEFORE any rename, so a
/// rejected file never reaches the installed path.
fn verify_download(path: &Path, os: &str) -> Result<()> {
    let size = std::fs::metadata(path)
        .with_context(|| format!("the download vanished: {}", path.display()))?
        .len();
    if size < MIN_BINARY_BYTES {
        bail!(
            "the download is only {size} bytes — that is not a umadev binary (a truncated \
             transfer or an error page served with HTTP 200)"
        );
    }
    let head = {
        use std::io::Read as _;
        let mut f = std::fs::File::open(path)
            .with_context(|| format!("could not re-open {}", path.display()))?;
        let mut buf = [0_u8; 8];
        let n = f.read(&mut buf).unwrap_or(0);
        buf[..n].to_vec()
    };
    if !looks_like_executable(os, &head) {
        bail!("the download does not look like a native executable for this platform");
    }
    Ok(())
}

/// Make the staged file executable before it is renamed into place, so the binary
/// is runnable the instant it appears at the installed path (no window where
/// `umadev` exists but is not `+x`).
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("could not chmod +x {}", path.display()))
}

/// No-op on Windows: executability is decided by the `.exe` extension.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// The manual fallback, printed on every failure path so the user is never stuck.
fn print_manual_instructions() {
    println!(
        "\nUpdate manually:\n  \
         cargo install --git https://github.com/umacloud/umadev umadev --force\n  \
         or download this platform's binary from: {RELEASES_PAGE}"
    );
}

/// `umadev update` for a [`InstallKind::Standalone`] install: resolve the latest
/// release, download + verify this platform's asset, and swap it over `exe`.
///
/// `confirm` is the caller's `y/N` prompt (skipped when `yes`). `force` reinstalls
/// even when already on the latest version. On ANY failure the installed binary is
/// left untouched and the manual command is printed.
pub async fn run(
    exe: &Path,
    yes: bool,
    force: bool,
    confirm: impl FnOnce(&str) -> bool,
) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let Some(asset) = release_asset_name(os, arch) else {
        println!("No prebuilt binary is published for {os}-{arch}.");
        print_manual_instructions();
        bail!("unsupported platform: {os}-{arch}");
    };

    println!("Checking for a newer release…");
    let release = match fetch_latest_release().await {
        Ok(r) => r,
        Err(e) => {
            println!("Could not check for updates: {e:#}");
            print_manual_instructions();
            return Err(e);
        }
    };

    if !is_newer(&release.tag, current) && !force {
        println!("Already on the latest version ({current}). Nothing to do.");
        return Ok(());
    }

    let Some(url) = release.asset_url(&asset).map(str::to_string) else {
        println!("Release {} publishes no `{asset}` asset.", release.tag);
        print_manual_instructions();
        bail!("no asset `{asset}` in release {}", release.tag);
    };

    if is_newer(&release.tag, current) {
        println!(
            "A newer release is available: {} (you have {current}).",
            release.tag
        );
    } else {
        println!("Reinstalling {} (--force).", release.tag);
    }
    if !yes && !confirm(&format!("Download and install {} now?", release.tag)) {
        println!("Aborted.");
        return Ok(());
    }

    // Stage NEXT TO the exe: same filesystem, so the final rename is atomic (a temp
    // dir is often a different mount, where `rename` fails with EXDEV).
    let mut tmp = exe.as_os_str().to_os_string();
    tmp.push(format!(".new-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);

    let result = install(&tmp, exe, &url, os).await;
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp); // never leave a partial download behind
        print_manual_instructions();
    }
    result?;

    println!(
        "[ok] UmaDev updated to {}. Run `umadev --version` to confirm.",
        release.tag
    );
    Ok(())
}

/// Download → verify → swap. Split out of [`run`] so every failure funnels through
/// one cleanup path (delete the temp file, keep the installed binary).
async fn install(tmp: &Path, exe: &Path, url: &str, os: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("umadev-cli")
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("could not build an HTTP client")?;

    let mut last: Option<anyhow::Error> = None;
    let mut downloaded = false;
    for candidate in download_urls(url) {
        match download_to(&client, &candidate, tmp).await {
            Ok(_) => {
                downloaded = true;
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    if !downloaded {
        return Err(last.unwrap_or_else(|| anyhow::anyhow!("no download source was reachable")));
    }

    verify_download(tmp, os)?;
    make_executable(tmp)?;
    apply_swap(&swap_plan(exe, tmp, cfg!(windows)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_match_the_release_workflow() {
        // These strings are the contract with `.github/workflows/release.yml`, which
        // stages `dist/umadev-<target><ext>`.
        assert_eq!(
            release_asset_name("macos", "aarch64").unwrap(),
            "umadev-aarch64-apple-darwin"
        );
        assert_eq!(
            release_asset_name("macos", "x86_64").unwrap(),
            "umadev-x86_64-apple-darwin"
        );
        assert_eq!(
            release_asset_name("linux", "x86_64").unwrap(),
            "umadev-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            release_asset_name("linux", "aarch64").unwrap(),
            "umadev-aarch64-unknown-linux-gnu"
        );
        assert_eq!(
            release_asset_name("windows", "x86_64").unwrap(),
            "umadev-x86_64-pc-windows-msvc.exe"
        );
    }

    #[test]
    fn windows_on_arm_reuses_the_x64_asset() {
        assert_eq!(
            release_asset_name("windows", "aarch64").unwrap(),
            "umadev-x86_64-pc-windows-msvc.exe"
        );
    }

    #[test]
    fn unpublished_platforms_have_no_asset() {
        assert!(release_asset_name("freebsd", "x86_64").is_none());
        assert!(release_asset_name("linux", "riscv64").is_none());
    }

    #[test]
    fn the_running_platform_always_has_an_asset() {
        // Every platform we can actually build+test on must be self-updatable.
        assert!(release_asset_name(std::env::consts::OS, std::env::consts::ARCH).is_some());
    }

    #[test]
    fn already_latest_is_not_newer() {
        assert!(!is_newer("v1.0.40", "1.0.40"));
        assert!(!is_newer("1.0.40", "1.0.40"));
    }

    #[test]
    fn a_higher_release_is_newer() {
        assert!(is_newer("v1.0.41", "1.0.40"));
        assert!(is_newer("v1.1.0", "1.0.99"));
        assert!(is_newer("v2.0.0", "1.9.9"));
    }

    #[test]
    fn a_local_build_ahead_of_the_release_is_not_downgraded() {
        assert!(!is_newer("v1.0.39", "1.0.40"));
    }

    #[test]
    fn a_non_semver_tag_falls_back_to_string_inequality() {
        assert!(is_newer("nightly", "1.0.40"));
        assert!(!is_newer("weird", "weird"));
    }

    #[test]
    fn swap_plan_on_unix_is_one_atomic_rename() {
        let exe = Path::new("/usr/local/bin/umadev");
        let tmp = Path::new("/usr/local/bin/umadev.new-1");
        assert_eq!(
            swap_plan(exe, tmp, false),
            vec![SwapStep::Install {
                from: tmp.to_path_buf(),
                to: exe.to_path_buf(),
            }]
        );
    }

    #[test]
    fn swap_plan_on_windows_parks_the_running_exe_first() {
        let exe = Path::new(r"C:\bin\umadev.exe");
        let tmp = Path::new(r"C:\bin\umadev.exe.new-1");
        assert_eq!(
            swap_plan(exe, tmp, true),
            vec![
                SwapStep::RenameAside {
                    from: exe.to_path_buf(),
                    to: PathBuf::from(r"C:\bin\umadev.exe.old"),
                },
                SwapStep::Install {
                    from: tmp.to_path_buf(),
                    to: exe.to_path_buf(),
                },
            ]
        );
    }

    #[test]
    fn backup_path_appends_dot_old() {
        assert_eq!(
            backup_path(Path::new("/usr/local/bin/umadev")),
            PathBuf::from("/usr/local/bin/umadev.old")
        );
        assert_eq!(
            backup_path(Path::new(r"C:\bin\umadev.exe")),
            PathBuf::from(r"C:\bin\umadev.exe.old")
        );
    }

    #[test]
    fn an_npm_install_is_package_managed() {
        assert_eq!(
            classify_install(Path::new(
                "/usr/local/lib/node_modules/@umacloud/cli-linux-x64/bin/umadev"
            )),
            InstallKind::PackageManaged
        );
        // pnpm's content-addressed layout still lives under node_modules.
        assert_eq!(
            classify_install(Path::new(
                "/home/u/Library/pnpm/global/5/.pnpm/@umacloud+cli-linux-x64@1.0.0/node_modules/@umacloud/cli-linux-x64/bin/umadev"
            )),
            InstallKind::PackageManaged
        );
    }

    #[test]
    fn a_cargo_target_build_is_a_dev_build() {
        assert_eq!(
            classify_install(Path::new("/home/u/UmaDev/target/debug/umadev")),
            InstallKind::DevBuild
        );
        assert_eq!(
            classify_install(Path::new("/home/u/UmaDev/target/release/umadev")),
            InstallKind::DevBuild
        );
        assert_eq!(
            classify_install(Path::new(
                "/home/u/UmaDev/target/aarch64-apple-darwin/release/umadev"
            )),
            InstallKind::DevBuild
        );
    }

    #[test]
    fn a_cargo_install_or_manual_copy_is_standalone() {
        assert_eq!(
            classify_install(Path::new("/home/u/.cargo/bin/umadev")),
            InstallKind::Standalone
        );
        assert_eq!(
            classify_install(Path::new("/usr/local/bin/umadev")),
            InstallKind::Standalone
        );
        assert_eq!(
            classify_install(Path::new(r"C:\Users\u\bin\umadev.exe")),
            InstallKind::Standalone
        );
        // A directory literally named `target` that is NOT a cargo build output.
        assert_eq!(
            classify_install(Path::new("/opt/target/umadev")),
            InstallKind::Standalone
        );
    }

    #[test]
    fn executable_magic_rejects_an_html_error_page() {
        let html = b"<!DOCTYPE html><html>";
        assert!(!looks_like_executable("linux", html));
        assert!(!looks_like_executable("macos", html));
        assert!(!looks_like_executable("windows", html));
    }

    #[test]
    fn executable_magic_accepts_each_native_format() {
        assert!(looks_like_executable("linux", b"\x7fELF\x02\x01\x01\x00"));
        assert!(looks_like_executable(
            "macos",
            &[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01]
        ));
        assert!(looks_like_executable(
            "macos",
            &[0xca, 0xfe, 0xba, 0xbe, 0x00, 0x00, 0x00, 0x02]
        ));
        assert!(looks_like_executable("windows", b"MZ\x90\x00"));
        // An unpublished platform fails open rather than blocking an update.
        assert!(looks_like_executable("freebsd", b"anything"));
    }

    #[test]
    fn parses_a_release_payload() {
        let body = r#"{
          "tag_name": "v1.0.41",
          "assets": [
            {"name": "umadev-aarch64-apple-darwin",
             "browser_download_url": "https://github.com/umacloud/umadev/releases/download/v1.0.41/umadev-aarch64-apple-darwin"},
            {"name": "model.safetensors",
             "browser_download_url": "https://example.invalid/model"}
          ]
        }"#;
        let r = parse_latest_release(body).unwrap();
        assert_eq!(r.tag, "v1.0.41");
        assert_eq!(r.assets.len(), 2);
        assert_eq!(
            r.asset_url("umadev-aarch64-apple-darwin").unwrap(),
            "https://github.com/umacloud/umadev/releases/download/v1.0.41/umadev-aarch64-apple-darwin"
        );
        assert!(r.asset_url("umadev-x86_64-pc-windows-msvc.exe").is_none());
    }

    #[test]
    fn a_release_without_assets_still_parses() {
        let r = parse_latest_release(r#"{"tag_name":"v9.9.9"}"#).unwrap();
        assert_eq!(r.tag, "v9.9.9");
        assert!(r.assets.is_empty());
    }

    #[test]
    fn junk_from_the_release_api_is_an_error_not_a_panic() {
        assert!(parse_latest_release("<html>rate limited</html>").is_err());
        assert!(parse_latest_release(r#"{"message":"Not Found"}"#).is_err());
    }

    #[test]
    fn download_urls_try_github_first_then_the_mirrors() {
        let urls = download_urls("https://github.com/umacloud/umadev/releases/download/v1/x");
        assert_eq!(urls.len(), 3);
        assert!(urls[0].starts_with("https://github.com/"));
        assert!(urls[1].contains("ghproxy.net"));
        assert!(urls[2].contains("ghfast.top"));
    }
}
