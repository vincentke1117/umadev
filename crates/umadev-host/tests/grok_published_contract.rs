//! Opt-in contract test for the exact Grok Build binary users install.
//!
//! Source markers protect the reviewed implementation. This complementary
//! test launches the official release artifact on every release platform and
//! proves that an isolated, unauthenticated profile reaches the same bounded
//! ACP handshake without starting browser-capable authentication or touching
//! the project.

use std::ffi::{OsStr, OsString};
use std::time::Duration;

use tempfile::TempDir;
use tokio::time::timeout;
use umadev_host::session_bootstrap::{SessionOpenError, SessionOpenPolicy};
use umadev_runtime::BasePermissionProfile;

const OPEN_TIMEOUT: Duration = Duration::from_secs(45);

struct EnvironmentGuard(Vec<(&'static str, Option<OsString>)>);

impl EnvironmentGuard {
    fn isolated(home: &OsStr) -> Self {
        let names = ["HOME", "USERPROFILE", "XAI_API_KEY", "GROK_DEPLOYMENT_KEY"];
        let saved = names
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect();
        std::env::set_var("HOME", home);
        std::env::set_var("USERPROFILE", home);
        std::env::remove_var("XAI_API_KEY");
        std::env::remove_var("GROK_DEPLOYMENT_KEY");
        Self(saved)
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..) {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn published_grok_binary_stops_at_the_typed_auth_offer_without_browser_login() {
    if std::env::var_os("UMADEV_GROK_PUBLISHED_CONTRACT").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        eprintln!(
            "skipped: set UMADEV_GROK_PUBLISHED_CONTRACT=1 in the isolated binary-contract job"
        );
        return;
    }

    let home = TempDir::new().expect("create isolated Grok binary home");
    let _environment = EnvironmentGuard::isolated(home.path().as_os_str());
    let workspace = TempDir::new().expect("create isolated Grok binary workspace");
    let before = std::fs::read_dir(workspace.path())
        .expect("read empty Grok binary workspace")
        .count();

    let result = timeout(
        OPEN_TIMEOUT,
        umadev_host::session_for_with_policy(
            "grok-build",
            workspace.path(),
            "",
            BasePermissionProfile::Plan,
            Some("Read-only published-binary contract probe."),
            SessionOpenPolicy::NonInteractive,
        ),
    )
    .await
    .expect("published Grok binary handshake exceeded its deadline");

    let offer = match result {
        Err(SessionOpenError::AuthRequired(offer)) => offer,
        Err(error) => panic!("isolated Grok home returned an untyped open error: {error}"),
        Ok(mut session) => {
            let _ = session.end().await;
            panic!("isolated Grok home unexpectedly opened an authenticated session")
        }
    };
    assert!(
        offer.may_open_browser,
        "the official unauthenticated release should expose its explicit interactive login choice"
    );
    assert!(
        offer
            .methods
            .iter()
            .all(|method| !method.id.trim().is_empty()),
        "every advertised auth choice must retain a typed method id"
    );
    assert_eq!(
        std::fs::read_dir(workspace.path())
            .expect("read Grok binary workspace after handshake")
            .count(),
        before,
        "an unauthenticated ACP handshake must not touch the project"
    );
}
