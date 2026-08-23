//! sluice-domain — the UI-agnostic business layer (02 §1). Everything here is
//! plain data + functions that can run on a background executor; the views
//! own the resulting snapshots and never block on git.
//!
//! - [`Repo`]: shared handle (gix reader, git CLI, console, watcher wiring)
//! - [`LogSnapshot`]: refs + log + graph layout for a [`LogQuery`]
//! - [`DetailSnapshot`]: one commit's detail + change list
//! - [`diff_commit_file`] / [`diff_work_file`]: `FileDiff`s composed from blobs
//! - [`ChangesSnapshot`]: working-tree status (M2)
//! - [`LogFilter`]: text / regex / author / date / AI-only filtering of a loaded log

pub mod filter;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, Result};
use sluice_backend_cli::GitCli;
use sluice_backend_gix::GixReader;
use sluice_core::diff::{DiffOptions, FileDiff, diff_bytes};
use sluice_core::*;
use sluice_graph::{GraphLayout, Node};

pub use filter::{DateFilter, LogFilter};
pub use sluice_backend_cli::CommitOptions;
pub use sluice_watch::WatchEvent;

/// Shared, thread-safe handle to one open repository.
#[derive(Clone)]
pub struct Repo {
    pub reader: Arc<dyn GitReader>,
    /// None for bare repositories (no working tree → no write path).
    pub cli: Option<Arc<GitCli>>,
    pub console: Console,
    pub info: RepoInfo,
}

impl Repo {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let console = Console::new();
        let reader = GixReader::discover(path.as_ref(), console.clone())?;
        let info = reader.info()?;
        let cli = match &info.workdir {
            Some(wd) => Some(Arc::new(GitCli::new(
                wd.clone(),
                info.git_dir.clone(),
                console.clone(),
            )?)),
            None => None,
        };
        Ok(Repo {
            reader: Arc::new(reader),
            cli,
            console,
            info,
        })
    }

    pub fn capabilities(&self) -> Capabilities {
        self.reader.capabilities()
    }

    pub fn workdir(&self) -> Option<&PathBuf> {
        self.info.workdir.as_ref()
    }

    /// Start the file watcher; events arrive on the returned receiver.
    pub fn watch(&self) -> Result<(sluice_watch::RepoWatcher, async_channel::Receiver<WatchEvent>)> {
        let (tx, rx) = async_channel::bounded(64);
        let w = sluice_watch::watch(self.info.workdir.as_deref(), &self.info.git_dir, tx)?;
        Ok((w, rx))
    }
}

/// Refs + log + graph for one query. Built on a background thread, then owned by the view.
#[derive(Clone)]
pub struct LogSnapshot {
    pub info: RepoInfo,
    pub refs: Vec<Ref>,
    pub commits: Vec<Commit>,
    pub graph: GraphLayout,
    pub query: LogQuery,
    pub load_ms: u128,
    refs_by_commit: HashMap<Oid, Vec<usize>>,
    /// Ref names a commit carries (for text search), lowercased.
    pub authors: Vec<String>,
}

impl LogSnapshot {
    pub fn load(reader: &dyn GitReader, query: &LogQuery) -> Result<Self> {
        let t0 = Instant::now();
        let info = reader.info()?;
        let refs = reader.refs()?;
        let mut refs_by_commit: HashMap<Oid, Vec<usize>> = HashMap::new();
        for (ix, r) in refs.iter().enumerate() {
            refs_by_commit.entry(r.target.clone()).or_default().push(ix);
        }
        let commits = reader.log(query)?;
        let mut snap = LogSnapshot {
            info,
            refs,
            commits,
            graph: GraphLayout::default(),
            query: query.clone(),
            load_ms: 0,
            refs_by_commit,
            authors: Vec::new(),
        };
        snap.graph = sluice_graph::layout(snap.commits.iter().map(|c| Node {
            id: &c.id,
            parents: &c.parents,
            tip_ref: snap.tip_ref_name(&c.id),
        }));
        snap.recolor_head_lane();
        let mut authors: Vec<String> = snap.commits.iter().map(|c| c.author.name.clone()).collect();
        authors.sort();
        authors.dedup();
        snap.authors = authors;
        snap.load_ms = t0.elapsed().as_millis();
        Ok(snap)
    }

    /// Refs pointing at `id`, local branches first, then remote branches, then tags.
    pub fn refs_at(&self, id: &Oid) -> Vec<&Ref> {
        let mut v: Vec<&Ref> = self
            .refs_by_commit
            .get(id)
            .map(|ixs| ixs.iter().map(|&i| &self.refs[i]).collect())
            .unwrap_or_default();
        v.sort_by_key(|r| match r.kind {
            RefKind::LocalBranch => 0,
            RefKind::RemoteBranch { .. } => 1,
            RefKind::Tag => 2,
        });
        v
    }

    fn tip_ref_name(&self, id: &Oid) -> Option<&str> {
        self.refs_by_commit.get(id).and_then(|ixs| {
            let rank = |r: &Ref| match r.kind {
                RefKind::LocalBranch => 0,
                RefKind::RemoteBranch { .. } => 1,
                RefKind::Tag => 2,
            };
            let best = ixs.iter().map(|&i| &self.refs[i]).min_by_key(|r| rank(r))?;
            Some(match &best.kind {
                RefKind::RemoteBranch { remote } => best
                    .short_name
                    .strip_prefix(&format!("{remote}/"))
                    .unwrap_or(&best.short_name),
                _ => best.short_name.as_str(),
            })
        })
    }

    pub fn is_head(&self, id: &Oid) -> bool {
        self.info.head.id.as_ref() == Some(id)
    }

    pub fn row_of(&self, id: &Oid) -> Option<usize> {
        self.commits.iter().position(|c| &c.id == id)
    }

    /// Stable hashing may hand the current branch any ink; the prototype keeps
    /// HEAD's lane cyan, so swap palette index 0 with whatever HEAD received.
    fn recolor_head_lane(&mut self) {
        let Some(head_id) = self.info.head.id.clone() else {
            return;
        };
        let Some(head_row) = self.row_of(&head_id) else {
            return;
        };
        let head_color = self.graph.rows[head_row].color;
        if head_color == 0 {
            return;
        }
        let swap = |c: &mut u16| {
            if *c == 0 {
                *c = head_color;
            } else if *c == head_color {
                *c = 0;
            }
        };
        for row in &mut self.graph.rows {
            swap(&mut row.color);
            for e in &mut row.out_edges {
                swap(&mut e.color);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct DetailSnapshot {
    pub detail: CommitDetail,
    pub changes: Vec<FileChange>,
}

impl DetailSnapshot {
    pub fn load(reader: &dyn GitReader, id: &Oid) -> Result<Self> {
        let detail = reader.commit_detail(id)?;
        let changes = reader.commit_changes(id).unwrap_or_default();
        Ok(Self { detail, changes })
    }
}

/// Diff of `path` between a commit and its first parent (renames: `old_path` for the parent side).
pub fn diff_commit_file(
    reader: &dyn GitReader,
    commit: &Oid,
    path: &str,
    old_path: Option<&str>,
    opts: &DiffOptions,
) -> Result<FileDiff> {
    let old = reader
        .blob(&BlobRev::ParentOf(commit.clone()), old_path.unwrap_or(path))?
        .unwrap_or_default();
    let new = reader
        .blob(&BlobRev::Commit(commit.clone()), path)?
        .unwrap_or_default();
    let mut d = diff_bytes(&old, &new, opts);
    d.old_path = Some(old_path.unwrap_or(path).to_string());
    d.new_path = Some(path.to_string());
    attach_syntax(&mut d, path, &old, &new);
    Ok(d)
}

/// Syntax spans for both sides when a grammar matches (05 §4); large files stay plain.
fn attach_syntax(d: &mut FileDiff, path: &str, old: &[u8], new: &[u8]) {
    if d.binary {
        return;
    }
    let new_text = std::str::from_utf8(new).ok();
    let old_text = std::str::from_utf8(old).ok();
    let first = new_text.or(old_text).and_then(|t| t.lines().next()).unwrap_or("");
    let Some(lang) = sluice_syntax::detect(path, first) else {
        return;
    };
    const MAX: usize = 2 * 1024 * 1024;
    if let Some(t) = new_text {
        d.syntax_new = sluice_syntax::highlight_lines(lang, t, MAX);
    }
    if let Some(t) = old_text {
        d.syntax_old = sluice_syntax::highlight_lines(lang, t, MAX);
    }
}

/// Diff of one path between two arbitrary commits (PR review: merge-base .. head).
pub fn diff_range_file(
    reader: &dyn GitReader,
    base: &Oid,
    head: &Oid,
    path: &str,
    old_path: Option<&str>,
    opts: &DiffOptions,
) -> Result<FileDiff> {
    let old = reader
        .blob(&BlobRev::Commit(base.clone()), old_path.unwrap_or(path))?
        .unwrap_or_default();
    let new = reader
        .blob(&BlobRev::Commit(head.clone()), path)?
        .unwrap_or_default();
    let mut d = diff_bytes(&old, &new, opts);
    d.old_path = Some(old_path.unwrap_or(path).to_string());
    d.new_path = Some(path.to_string());
    attach_syntax(&mut d, path, &old, &new);
    Ok(d)
}

/// Diff of a working-tree path: `staged` = index vs HEAD, otherwise worktree vs index.
pub fn diff_work_file(
    reader: &dyn GitReader,
    path: &str,
    old_path: Option<&str>,
    staged: bool,
    opts: &DiffOptions,
) -> Result<FileDiff> {
    let (old_rev, new_rev) = if staged {
        (BlobRev::Head, BlobRev::Index)
    } else {
        (BlobRev::Index, BlobRev::Worktree)
    };
    let old = reader
        .blob(&old_rev, old_path.unwrap_or(path))?
        .unwrap_or_default();
    let new = reader.blob(&new_rev, path)?.unwrap_or_default();
    let mut d = diff_bytes(&old, &new, opts);
    d.old_path = Some(old_path.unwrap_or(path).to_string());
    d.new_path = Some(path.to_string());
    attach_syntax(&mut d, path, &old, &new);
    Ok(d)
}

/// Working-tree status (M2). Loaded through the git CLI (`status --porcelain=v2`).
#[derive(Clone, Debug, Default)]
pub struct ChangesSnapshot {
    pub status: WorkStatus,
    pub load_ms: u128,
}

impl ChangesSnapshot {
    pub fn load(cli: &GitCli) -> Result<Self> {
        let t0 = Instant::now();
        let status = cli.status().context("reading working-tree status")?;
        Ok(Self {
            status,
            load_ms: t0.elapsed().as_millis(),
        })
    }
}
