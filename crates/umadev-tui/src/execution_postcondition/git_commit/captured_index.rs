use super::{git_commit_blocked, GitIndexSnapshot, PathBuf, ResidentExecutionBlocked};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

/// A byte-for-byte copy of the index captured at authorization time.
///
/// It lives beside the real index so split-index shared files retain their
/// relative lookup semantics while `GIT_INDEX_FILE` isolates tree reads from
/// concurrent `git add` or IDE index refreshes.
#[derive(Debug)]
pub(super) struct CapturedGitIndex {
    pub(super) path: PathBuf,
}

impl CapturedGitIndex {
    pub(super) fn materialize(
        snapshot: &GitIndexSnapshot,
    ) -> Result<Self, ResidentExecutionBlocked> {
        static SNAPSHOT_ID: AtomicU64 = AtomicU64::new(1);
        let bytes = snapshot.bytes.as_ref().ok_or_else(|| {
            git_commit_blocked(
                "git-expected-tree-unverifiable",
                "捕获时 Git index 不存在,无法冻结 staged tree / the captured Git index does not exist",
            )
        })?;
        let parent = snapshot.path.parent().ok_or_else(|| {
            git_commit_blocked(
                "git-expected-tree-unverifiable",
                "Git index 没有可用父目录 / Git index has no usable parent directory",
            )
        })?;
        for _ in 0..64 {
            let id = SNAPSHOT_ID.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".umadev-captured-index-{}-{id}",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                        let _ = std::fs::remove_file(&path);
                        return Err(git_commit_blocked(
                            "git-expected-tree-unverifiable",
                            &format!(
                                "无法写入捕获的隔离 Git index / unable to materialize the captured Git index: {error}"
                            ),
                        ));
                    }
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(git_commit_blocked(
                        "git-expected-tree-unverifiable",
                        &format!(
                            "无法创建捕获的隔离 Git index / unable to create the captured Git index: {error}"
                        ),
                    ));
                }
            }
        }
        Err(git_commit_blocked(
            "git-expected-tree-unverifiable",
            "无法分配捕获的隔离 Git index / unable to allocate the captured Git index",
        ))
    }
}

impl Drop for CapturedGitIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
