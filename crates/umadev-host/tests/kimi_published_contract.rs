//! Opt-in contract test for the exact Kimi Code package users install.
//!
//! Source markers protect the reviewed implementation. This complementary
//! test launches the published npm executable on every release platform and
//! proves that an isolated, unauthenticated profile reaches the same bounded
//! ACP handshake without opening a browser or a model session.

use std::time::Duration;

use tempfile::TempDir;
use tokio::time::timeout;
use umadev_host::session_bootstrap::SessionOpenPolicy;
use umadev_runtime::BasePermissionProfile;

const OPEN_TIMEOUT: Duration = Duration::from_secs(45);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn published_kimi_package_fails_closed_with_terminal_login_guidance() {
    if std::env::var_os("UMADEV_KIMI_PUBLISHED_CONTRACT").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        eprintln!(
            "skipped: set UMADEV_KIMI_PUBLISHED_CONTRACT=1 in the isolated package-contract job"
        );
        return;
    }

    let workspace = TempDir::new().expect("create isolated Kimi package workspace");
    let before = std::fs::read_dir(workspace.path())
        .expect("read empty package workspace")
        .count();

    let result = timeout(
        OPEN_TIMEOUT,
        umadev_host::session_for_with_policy(
            "kimi-code",
            workspace.path(),
            "",
            BasePermissionProfile::Plan,
            Some("Read-only published-package contract probe."),
            SessionOpenPolicy::NonInteractive,
        ),
    )
    .await
    .expect("published Kimi package handshake exceeded its deadline");

    let error = match result {
        Ok(mut session) => {
            let _ = session.end().await;
            panic!("isolated Kimi home unexpectedly opened an authenticated session")
        }
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("kimi login"),
        "actionable login command: {error}"
    );
    assert!(
        error.contains("never open") || error.contains("does not launch"),
        "the diagnostic must promise that UmaDev will not launch browser login: {error}"
    );
    assert_eq!(
        std::fs::read_dir(workspace.path())
            .expect("read package workspace after handshake")
            .count(),
        before,
        "an unauthenticated ACP handshake must not touch the project"
    );
}
