//! Permission-tagged holder for a parked continuous Director session.
//!
//! A base process keeps its launch permissions for its lifetime. Tagging the
//! parked value prevents a damaged/missing workflow state plus a TUI mode
//! change from reusing an Auto worker for a Guarded or Plan continuation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use umadev_runtime::{BasePermissionProfile, BaseResumeIdentity, BaseSession};

/// Immutable process-launch identity for a parked resident session.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct SessionIdentity {
    pub(crate) backend: String,
    pub(crate) canonical_workspace: PathBuf,
    pub(crate) permissions: BasePermissionProfile,
}

impl SessionIdentity {
    /// Resolve an identity only when the workspace can be canonicalized. A missing
    /// identity is never reusable: aliases, moved projects, and deleted roots must
    /// open a fresh process rather than guess.
    pub(crate) fn for_launch(
        backend: &str,
        workspace: &Path,
        permissions: BasePermissionProfile,
    ) -> Option<Self> {
        let canonical_workspace = std::fs::canonicalize(workspace).ok()?;
        Some(Self {
            backend: backend.to_string(),
            canonical_workspace,
            permissions,
        })
    }
}

/// Build the persisted authority identity for a newly-opened base session.
///
/// Grok currently exposes no effective sandbox attestation over ACP, so this is
/// intentionally `requested_only`. That makes the saved id ineligible for
/// `session/load` until the native preflight/effective-report seam is implemented.
pub(crate) fn requested_resume_identity(
    backend: &str,
    workspace: &Path,
    permissions: BasePermissionProfile,
) -> Option<BaseResumeIdentity> {
    BaseResumeIdentity::requested_for_launch(backend, workspace, permissions)
}

pub(crate) fn build_host_driver(
    id: &str,
    continue_session: bool,
    session_id: Option<String>,
    project_root: &Path,
    permissions: BasePermissionProfile,
) -> anyhow::Result<Box<dyn umadev_host::HostDriver>> {
    let mut driver = umadev_host::driver_for_with_permissions(id, permissions)
        .ok_or_else(|| anyhow::anyhow!("unknown backend `{id}`"))?;
    driver.set_continue_session(continue_session);
    driver.set_session_id(session_id);
    driver.set_workspace(project_root.to_path_buf());
    Ok(driver)
}

pub(crate) fn build_cold_judge_driver(
    backend: &str,
    root: PathBuf,
) -> Option<Box<dyn umadev_host::HostDriver>> {
    let mut driver =
        umadev_host::driver_for_with_permissions(backend, BasePermissionProfile::Plan)?;
    driver.set_continue_session(false);
    driver.set_session_id(None);
    driver.set_workspace(root);
    Some(driver)
}

/// One parked value and the exact permission profile used to launch it.
pub(crate) struct PermissionedSession<T> {
    value: T,
    identity: SessionIdentity,
}

impl<T> PermissionedSession<T> {
    pub(crate) fn new(value: T, identity: SessionIdentity) -> Self {
        Self { value, identity }
    }

    /// Return the value only when its complete immutable launch identity matches
    /// the next block; otherwise return it as stale so the caller can close it and
    /// reopen. Permission-only matching is insufficient after a backend switch or
    /// workspace move.
    pub(crate) fn into_matching(self, requested: &SessionIdentity) -> Result<T, T> {
        if &self.identity == requested {
            Ok(self.value)
        } else {
            Err(self.value)
        }
    }

    pub(crate) fn into_inner(self) -> T {
        self.value
    }
}

/// The Director's continuous brain, parked only at gate boundaries.
pub(crate) type SessionHolder =
    Arc<tokio::sync::Mutex<Option<PermissionedSession<Box<dyn BaseSession>>>>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_backend_permission_and_switch_matrix_reuses_only_exact_identity() {
        let workspace = tempfile::TempDir::new().unwrap();
        for parked_backend in umadev_host::BACKEND_IDS {
            for requested_backend in umadev_host::BACKEND_IDS {
                for parked_profile in [
                    BasePermissionProfile::Plan,
                    BasePermissionProfile::Guarded,
                    BasePermissionProfile::Auto,
                ] {
                    for requested_profile in [
                        BasePermissionProfile::Plan,
                        BasePermissionProfile::Guarded,
                        BasePermissionProfile::Auto,
                    ] {
                        let parked = SessionIdentity::for_launch(
                            parked_backend,
                            workspace.path(),
                            parked_profile,
                        )
                        .unwrap();
                        let requested = SessionIdentity::for_launch(
                            requested_backend,
                            workspace.path(),
                            requested_profile,
                        )
                        .unwrap();
                        let result =
                            PermissionedSession::new("session", parked).into_matching(&requested);
                        assert_eq!(
                            result.is_ok(),
                            parked_backend == requested_backend
                                && parked_profile == requested_profile,
                            "{parked_backend}/{parked_profile:?} → \
                             {requested_backend}/{requested_profile:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn parked_value_rejects_backend_and_canonical_workspace_mismatch() {
        let first = tempfile::TempDir::new().unwrap();
        let second = tempfile::TempDir::new().unwrap();
        let parked =
            SessionIdentity::for_launch("grok-build", first.path(), BasePermissionProfile::Guarded)
                .unwrap();
        let other_backend =
            SessionIdentity::for_launch("codex", first.path(), BasePermissionProfile::Guarded)
                .unwrap();
        assert!(PermissionedSession::new("session", parked.clone())
            .into_matching(&other_backend)
            .is_err());
        let other_workspace = SessionIdentity::for_launch(
            "grok-build",
            second.path(),
            BasePermissionProfile::Guarded,
        )
        .unwrap();
        assert!(PermissionedSession::new("session", parked)
            .into_matching(&other_workspace)
            .is_err());
    }
}
