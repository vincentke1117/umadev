use super::{
    combined_git_failure, display_paths, git_commit_blocked, same_permissions, snapshot_blocked,
    BTreeSet, Duration, ExecutionContract, Path, PathBuf, ResidentExecutionBlocked,
    WorkspaceBaseline,
};

mod captured_index;
mod command;
mod config_safety;
mod index;
mod plumbing;
mod policy;
mod process;
mod transaction;
mod validation;

pub(crate) use command::*;
pub(crate) use config_safety::*;
pub(crate) use index::*;
pub(crate) use plumbing::*;
pub(crate) use process::*;
pub(crate) use transaction::*;
pub(crate) use validation::*;

#[derive(Debug)]
pub(crate) struct GitCommitBaseline {
    pub(crate) head: Option<String>,
    pub(crate) symbolic_head: Option<String>,
    pub(crate) dirty_paths: BTreeSet<String>,
    pub(crate) expected_paths: BTreeSet<String>,
    pub(crate) staged_only: bool,
    pub(crate) index: GitIndexSnapshot,
    pub(crate) identity_name: String,
    pub(crate) identity_email: String,
}
