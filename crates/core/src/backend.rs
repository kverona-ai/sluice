use anyhow::Result;

use crate::types::*;

/// Backend capability declaration (02 §2): the domain layer switches features on
/// and off from this instead of assuming every backend has, say, an index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub staging_area: bool,
    pub operation_log: bool,
    pub change_ids: bool,
    pub first_class_conflicts: bool,
}

impl Capabilities {
    pub const GIT: Capabilities = Capabilities {
        staging_area: true,
        operation_log: false,
        change_ids: false,
        first_class_conflicts: false,
    };
}

/// Read path of the `GitBackend` trait (02 §2). Implemented by `sluice-backend-gix`.
/// Synchronous for now; the domain layer runs it on a background executor.
pub trait GitReader: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn info(&self) -> Result<RepoInfo>;
    fn refs(&self) -> Result<Vec<Ref>>;
    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>>;
    fn commit_detail(&self, id: &Oid) -> Result<CommitDetail>;
    /// Changes of `id` against its first parent (or the empty tree for root commits).
    fn commit_changes(&self, id: &Oid) -> Result<Vec<FileChange>>;
    /// Raw content of `path` at `rev` (None when the path does not exist there). The
    /// domain layer composes diffs from pairs of these with `sluice_core::diff`.
    fn blob(&self, rev: &BlobRev, path: &str) -> Result<Option<Vec<u8>>>;
}

/// Where to read a file from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobRev {
    /// The tree of a commit.
    Commit(Oid),
    /// The first parent of a commit (empty tree for roots).
    ParentOf(Oid),
    /// HEAD's tree.
    Head,
    /// The index (staging area).
    Index,
    /// The working tree file on disk.
    Worktree,
}
