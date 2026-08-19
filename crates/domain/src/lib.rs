//! sluice-domain — the UI-agnostic business layer (02 §1): `RepoStore` owns the
//! loaded repository state (refs, log, graph layout, lazily loaded details) and
//! is the only thing the views talk to. No gpui types may appear here.
//!
//! M0/M1 slice: synchronous loading. The command/event bus, watcher-driven
//! incremental refresh and the background executor arrive with M1 proper.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use sluice_backend_gix::GixReader;
use sluice_core::*;
use sluice_graph::{GraphLayout, Node};

pub struct RepoStore {
    reader: Arc<dyn GitReader>,
    pub info: RepoInfo,
    pub refs: Vec<Ref>,
    pub commits: Vec<Commit>,
    pub graph: GraphLayout,
    pub query: LogQuery,
    /// Wall-clock milliseconds of the last `reload()` (refs + log + layout).
    pub load_ms: u128,
    /// commit id -> indices into `refs`
    refs_by_commit: HashMap<Oid, Vec<usize>>,
    detail_cache: HashMap<Oid, Arc<(CommitDetail, Vec<FileChange>)>>,
}

impl RepoStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let reader: Arc<dyn GitReader> = Arc::new(GixReader::discover(path)?);
        let info = reader.info()?;
        let mut store = RepoStore {
            reader,
            info,
            refs: Vec::new(),
            commits: Vec::new(),
            graph: GraphLayout::default(),
            query: LogQuery::default(),
            load_ms: 0,
            refs_by_commit: HashMap::new(),
            detail_cache: HashMap::new(),
        };
        store.reload()?;
        Ok(store)
    }

    pub fn capabilities(&self) -> Capabilities {
        self.reader.capabilities()
    }

    /// Re-read refs and the log for the current query and lay the graph out again.
    pub fn reload(&mut self) -> Result<()> {
        let t0 = Instant::now();
        self.info = self.reader.info()?;
        self.refs = self.reader.refs()?;
        self.refs_by_commit.clear();
        for (ix, r) in self.refs.iter().enumerate() {
            self.refs_by_commit.entry(r.target.clone()).or_default().push(ix);
        }
        self.commits = self.reader.log(&self.query)?;
        self.graph = sluice_graph::layout(self.commits.iter().map(|c| Node {
            id: &c.id,
            parents: &c.parents,
            tip_ref: self.tip_ref_name(&c.id),
        }));
        self.recolor_head_lane();
        self.load_ms = t0.elapsed().as_millis();
        Ok(())
    }

    /// Stable hashing may hand the current branch any ink; the prototype keeps
    /// HEAD's lane cyan, so swap palette index 0 with whatever HEAD received.
    fn recolor_head_lane(&mut self) {
        let Some(head_id) = self.info.head.id.clone() else {
            return;
        };
        let Some(head_row) = self.commits.iter().position(|c| c.id == head_id) else {
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

    /// Restrict the log to the given tips (empty = everything) and reload.
    pub fn set_tips(&mut self, tips: Vec<Oid>) -> Result<()> {
        self.query.tips = tips;
        self.reload()
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
            // Prefer a local branch name, then remote, then tag — stable colors follow branch names.
            let mut best: Option<&Ref> = None;
            for &i in ixs {
                let r = &self.refs[i];
                let rank = |r: &Ref| match r.kind {
                    RefKind::LocalBranch => 0,
                    RefKind::RemoteBranch { .. } => 1,
                    RefKind::Tag => 2,
                };
                if best.is_none_or(|b| rank(r) < rank(b)) {
                    best = Some(r);
                }
            }
            best.map(|r| {
                // `origin/main` and `main` should share one ink.
                match &r.kind {
                    RefKind::RemoteBranch { remote } => r
                        .short_name
                        .strip_prefix(&format!("{remote}/"))
                        .unwrap_or(&r.short_name),
                    _ => r.short_name.as_str(),
                }
            })
        })
    }

    pub fn is_head(&self, id: &Oid) -> bool {
        self.info.head.id.as_ref() == Some(id)
    }

    /// Full detail + change list for the commit at row `ix` (cached).
    pub fn detail(&mut self, ix: usize) -> Result<Arc<(CommitDetail, Vec<FileChange>)>> {
        let id = self
            .commits
            .get(ix)
            .map(|c| c.id.clone())
            .ok_or_else(|| anyhow::anyhow!("row out of range"))?;
        if let Some(d) = self.detail_cache.get(&id) {
            return Ok(d.clone());
        }
        let detail = self.reader.commit_detail(&id)?;
        let changes = self.reader.commit_changes(&id).unwrap_or_default();
        let entry = Arc::new((detail, changes));
        self.detail_cache.insert(id, entry.clone());
        Ok(entry)
    }
}
