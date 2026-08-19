use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::agent::Agent;

/// A git object id as lowercase hex. Kept as a string for now; the graph
/// skeleton only stores one `Oid` per commit plus parents (02 §4 budget).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Oid(pub String);

impl Oid {
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// First `n` hex characters (never panics).
    pub fn short(&self, n: usize) -> &str {
        &self.0[..n.min(self.0.len())]
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.short(10))
    }
}

/// Author / committer identity with the signature time in its original offset.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub time: DateTime<FixedOffset>,
}

/// The per-row commit record used by the log (summary only; the full message
/// is loaded lazily through [`CommitDetail`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Commit {
    pub id: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    /// First line of the message.
    pub summary: String,
    /// Best-effort provenance (trailer / co-author heuristics; see 03 §6).
    pub agent: Agent,
}

impl Commit {
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitDetail {
    pub commit: Commit,
    /// Full message (subject + body) as UTF-8 (lossy).
    pub message: String,
    /// Parsed trailers such as `Co-authored-by`, `Sluice-Agent`, `Sluice-Session`.
    pub trailers: Vec<(String, String)>,
    /// Whether the commit carries a GPG/SSH signature header (not verified yet).
    pub has_signature: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefKind {
    LocalBranch,
    RemoteBranch { remote: String },
    Tag,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ref {
    /// e.g. `refs/heads/main`
    pub full_name: String,
    /// e.g. `main`, `origin/main`, `v0.2`
    pub short_name: String,
    pub kind: RefKind,
    /// Peeled commit id.
    pub target: Oid,
    /// True if HEAD currently points at this (local) branch.
    pub is_head: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HeadInfo {
    /// Short branch name when HEAD is attached.
    pub branch: Option<String>,
    pub id: Option<Oid>,
    pub detached: bool,
    /// Upstream short name (e.g. `origin/main`).
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoInfo {
    pub name: String,
    pub workdir: Option<PathBuf>,
    pub git_dir: PathBuf,
    pub is_bare: bool,
    pub is_shallow: bool,
    pub head: HeadInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

impl ChangeKind {
    pub fn mark(&self) -> &'static str {
        match self {
            ChangeKind::Added => "A",
            ChangeKind::Modified => "M",
            ChangeKind::Deleted => "D",
            ChangeKind::Renamed => "R",
            ChangeKind::Copied => "C",
            ChangeKind::TypeChanged => "T",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileChange {
    /// Repository-relative path (new path for renames).
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
}

impl FileChange {
    /// `dir/` and `name` split for the two-tone file rows of the prototype.
    pub fn split_dir_name(&self) -> (&str, &str) {
        match self.path.rfind('/') {
            Some(i) => (&self.path[..=i], &self.path[i + 1..]),
            None => ("", &self.path),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogOrder {
    /// `git rev-list --date-order`: commit-time order, parents never before children (default, 05 §4).
    #[default]
    DateOrder,
    /// `git rev-list --topo-order`.
    TopoOrder,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogQuery {
    /// Tips to start from. Empty = HEAD + every branch and tag (IDEA default, 05 §4).
    pub tips: Vec<Oid>,
    pub order: LogOrder,
    pub limit: usize,
}

impl Default for LogQuery {
    fn default() -> Self {
        Self {
            tips: Vec::new(),
            order: LogOrder::DateOrder,
            limit: 5_000,
        }
    }
}
