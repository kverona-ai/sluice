//! sluice-backend-gix — the read path of the `GitBackend` (02 §2): refs, log,
//! commit details and per-commit change lists via gitoxide. Pure Rust, so it
//! cross-compiles to the mobile shells later.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use gix::bstr::ByteSlice;
use gix::traverse::commit::topo;
use sluice_core::*;

/// Size above which we don't compute line stats for a blob (05 §4).
const MAX_STAT_BLOB_BYTES: u64 = 4 * 1024 * 1024;
/// Bound for ahead/behind counting so a divergent fork can't stall startup.
const MAX_AHEAD_BEHIND_WALK: usize = 10_000;

pub struct GixReader {
    repo: gix::ThreadSafeRepository,
    console: Console,
    vcs: Vcs,
}

impl GixReader {
    /// Open the repository containing `path` (any subdirectory works; worktrees resolve to their common dir).
    pub fn discover(path: impl AsRef<Path>, console: Console) -> Result<Self> {
        let p = path.as_ref();
        // jujutsu (non-colocated): the git store lives in .jj/repo/store/git; open it with the
        // jj workspace as the working directory so blobs / logs work through gix unchanged.
        if p.join(".jj").is_dir() && !p.join(".git").exists() {
            let store = p.join(".jj").join("repo").join("store").join("git");
            if store.is_dir() {
                let mut repo = gix::open(&store).with_context(|| format!("opening jj git store {}", store.display()))?;
                repo.set_workdir(Some(p.to_path_buf()))?;
                return Ok(Self { repo: repo.into_sync(), console, vcs: Vcs::Jujutsu { colocated: false } });
            }
        }
        let repo = gix::discover(p)
            .with_context(|| format!("no git repository found at or above {}", p.display()))?;
        let vcs = match repo.workdir() {
            Some(wd) if wd.join(".jj").is_dir() => Vcs::Jujutsu { colocated: true },
            _ => Vcs::Git,
        };
        Ok(Self {
            repo: repo.into_sync(),
            console,
            vcs,
        })
    }

    pub fn console(&self) -> &Console {
        &self.console
    }

    fn blob_in_tree(
        &self,
        repo: &gix::Repository,
        tree: &gix::Tree<'_>,
        path: &str,
    ) -> Result<Option<Vec<u8>>> {
        let Some(entry) = tree.lookup_entry_by_path(path)? else {
            return Ok(None);
        };
        if !entry.mode().is_blob_or_symlink() {
            return Ok(None);
        }
        Ok(Some(repo.find_blob(entry.object_id())?.data.clone()))
    }

    fn repo(&self) -> gix::Repository {
        self.repo.to_thread_local()
    }

    fn all_tips(&self, repo: &gix::Repository) -> Result<Vec<gix::ObjectId>> {
        let mut seen = HashSet::new();
        let mut tips = Vec::new();
        if let Ok(id) = repo.head_id() {
            let id = id.detach();
            if seen.insert(id) {
                tips.push(id);
            }
        }
        let platform = repo.references()?;
        for r in platform.all()?.flatten() {
            let mut r = r;
            if r.name().category().is_none_or(|c| {
                !matches!(
                    c,
                    gix::refs::Category::LocalBranch
                        | gix::refs::Category::RemoteBranch
                        | gix::refs::Category::Tag
                )
            }) {
                continue;
            }
            if let Ok(id) = r.peel_to_id() {
                let id = id.detach();
                // Only commits can seed a walk (annotated tags peel to commits; tree/blob tags are skipped).
                if seen.insert(id) && repo.find_commit(id).is_ok() {
                    tips.push(id);
                }
            }
        }
        Ok(tips)
    }
}

fn to_chrono(t: gix::date::Time) -> DateTime<FixedOffset> {
    let offset = FixedOffset::east_opt(t.offset).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    offset
        .timestamp_opt(t.seconds, 0)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap().with_timezone(&offset))
}

fn signature(s: gix::actor::SignatureRef<'_>) -> Signature {
    let time = s.time().unwrap_or(gix::date::Time {
        seconds: 0,
        offset: 0,
    });
    Signature {
        name: s.name.to_str_lossy().into_owned(),
        email: s.email.to_str_lossy().into_owned(),
        time: to_chrono(time),
    }
}

fn oid(id: impl std::fmt::Display) -> Oid {
    Oid::new(id.to_string())
}

fn parse_oid(id: &Oid) -> Result<gix::ObjectId> {
    gix::ObjectId::from_hex(id.as_str().as_bytes()).map_err(|e| anyhow!("bad object id {id}: {e}"))
}

fn commit_from(repo: &gix::Repository, id: gix::ObjectId, parents: Vec<gix::ObjectId>) -> Result<Commit> {
    let c = repo.find_commit(id)?;
    let decoded = c.decode()?;
    let author = signature(decoded.author()?);
    let committer = signature(decoded.committer()?);
    let message = decoded.message.to_str_lossy();
    let summary = decoded.message().summary().to_str_lossy().into_owned();
    let agent = Agent::detect(&message, &author.name, &author.email);
    Ok(Commit {
        id: oid(id),
        parents: parents.into_iter().map(oid).collect(),
        author,
        committer,
        summary,
        agent,
    })
}

fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8000).any(|b| *b == 0)
}

fn count_lines(data: &[u8]) -> u32 {
    if data.is_empty() {
        return 0;
    }
    let n = data.iter().filter(|b| **b == b'\n').count();
    (n + usize::from(!data.ends_with(b"\n"))) as u32
}

fn line_stats(
    repo: &gix::Repository,
    old: Option<gix::ObjectId>,
    new: Option<gix::ObjectId>,
) -> (Option<u32>, Option<u32>, bool) {
    let load = |id: Option<gix::ObjectId>| -> Option<Vec<u8>> {
        let id = id?;
        let header = repo.find_header(id).ok()?;
        if header.size() > MAX_STAT_BLOB_BYTES {
            return None;
        }
        repo.find_blob(id).ok().map(|b| b.data.clone())
    };
    let a = load(old);
    let b = load(new);
    match (old, new, a, b) {
        (None, Some(_), _, Some(b)) => {
            if is_binary(&b) {
                (None, None, true)
            } else {
                (Some(count_lines(&b)), Some(0), false)
            }
        }
        (Some(_), None, Some(a), _) => {
            if is_binary(&a) {
                (None, None, true)
            } else {
                (Some(0), Some(count_lines(&a)), false)
            }
        }
        (Some(_), Some(_), Some(a), Some(b)) => {
            if is_binary(&a) || is_binary(&b) {
                return (None, None, true);
            }
            use imara_diff::{Algorithm, diff, intern::InternedInput, sink::Counter};
            let input = InternedInput::new(a.as_slice(), b.as_slice());
            let counter = diff(Algorithm::Histogram, &input, Counter::default());
            (Some(counter.insertions), Some(counter.removals), false)
        }
        _ => (None, None, false),
    }
}

impl GitReader for GixReader {
    fn capabilities(&self) -> Capabilities {
        Capabilities::GIT
    }

    fn info(&self) -> Result<RepoInfo> {
        let repo = self.repo();
        let workdir = repo.workdir().map(|p| p.to_path_buf());
        let git_dir = repo.git_dir().to_path_buf();
        let name = workdir
            .as_ref()
            .and_then(|w| w.file_name())
            .or_else(|| git_dir.parent().and_then(|p| p.file_name()))
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repository".into());

        let mut head = HeadInfo::default();
        if let Ok(h) = repo.head() {
            head.detached = h.is_detached();
            head.branch = h.referent_name().map(|n| n.shorten().to_str_lossy().into_owned());
            head.id = h.id().map(|id| oid(id.detach()));
        }
        if let (Ok(Some(head_ref)), Some(head_id)) = (repo.head_ref(), head.id.clone())
            && let Some(Ok(upstream)) = head_ref.remote_tracking_ref_name(gix::remote::Direction::Fetch)
        {
            head.upstream = Some(upstream.shorten().to_str_lossy().into_owned());
            if let Ok(mut up_ref) = repo.find_reference(upstream.as_ref())
                && let Ok(up_id) = up_ref.peel_to_id()
            {
                let head_oid = parse_oid(&head_id)?;
                let up_oid = up_id.detach();
                head.ahead = count_reachable_hidden(&repo, head_oid, up_oid);
                head.behind = count_reachable_hidden(&repo, up_oid, head_oid);
            }
        }
        Ok(RepoInfo {
            name,
            workdir,
            git_dir,
            is_bare: repo.is_bare(),
            is_shallow: repo.is_shallow(),
            head,
            vcs: self.vcs,
        })
    }

    fn refs(&self) -> Result<Vec<Ref>> {
        let t0 = std::time::Instant::now();
        let repo = self.repo();
        let head_name = repo.head_name().ok().flatten().map(|n| n.as_bstr().to_string());
        let mut out = Vec::new();
        let platform = repo.references()?;
        for r in platform.all()?.flatten() {
            let mut r = r;
            let full = r.name().as_bstr().to_string();
            let Some((category, short)) = r.name().category_and_short_name() else {
                continue;
            };
            let short = short.to_str_lossy().into_owned();
            let kind = match category {
                gix::refs::Category::LocalBranch => RefKind::LocalBranch,
                // `refs/remotes/origin/HEAD` is a symref to the remote's default branch — not a branch of its own.
                gix::refs::Category::RemoteBranch if short.ends_with("/HEAD") => continue,
                gix::refs::Category::RemoteBranch => {
                    let remote = short.split('/').next().unwrap_or_default().to_string();
                    RefKind::RemoteBranch { remote }
                }
                gix::refs::Category::Tag => RefKind::Tag,
                _ => continue,
            };
            let Ok(target) = r.peel_to_id() else { continue };
            let target = target.detach();
            if repo.find_commit(target).is_err() {
                continue; // tag pointing at a tree/blob
            }
            let is_head = head_name.as_deref() == Some(full.as_str());
            out.push(Ref {
                full_name: full,
                short_name: short,
                kind,
                target: oid(target),
                is_head,
            });
        }
        out.sort_by(|a, b| a.short_name.cmp(&b.short_name));
        self.console
            .read("git for-each-ref", t0.elapsed(), format!("{} refs", out.len()));
        Ok(out)
    }

    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>> {
        let t0 = std::time::Instant::now();
        let repo = self.repo();
        let tips: Vec<gix::ObjectId> = if query.tips.is_empty() {
            self.all_tips(&repo)?
        } else {
            query.tips.iter().map(parse_oid).collect::<Result<_>>()?
        };
        if tips.is_empty() {
            return Ok(Vec::new());
        }
        let sorting = match query.order {
            LogOrder::DateOrder => topo::Sorting::DateOrder,
            LogOrder::TopoOrder => topo::Sorting::TopoOrder,
        };
        let walk = topo::Builder::from_iters(&repo.objects, tips, None::<Vec<gix::ObjectId>>)
            .sorting(sorting)
            .build()?;
        let mut out = Vec::with_capacity(query.limit.min(10_000));
        for info in walk.take(query.limit) {
            let info = info?;
            out.push(commit_from(
                &repo,
                info.id,
                info.parent_ids.iter().copied().collect(),
            )?);
        }
        let order = match query.order {
            LogOrder::DateOrder => "--date-order",
            LogOrder::TopoOrder => "--topo-order",
        };
        let scope = if query.tips.is_empty() {
            "--all".to_string()
        } else {
            format!("{} tips", query.tips.len())
        };
        self.console.read(
            format!("git log {order} --max-count={} {scope}", query.limit),
            t0.elapsed(),
            format!("{} commits", out.len()),
        );
        Ok(out)
    }

    fn commit_detail(&self, id: &Oid) -> Result<CommitDetail> {
        let repo = self.repo();
        let gid = parse_oid(id)?;
        let c = repo.find_commit(gid)?;
        let parents: Vec<gix::ObjectId> = c.parent_ids().map(|p| p.detach()).collect();
        let commit = commit_from(&repo, gid, parents)?;
        let decoded = c.decode()?;
        let message = decoded.message.to_str_lossy().into_owned();
        let trailers = decoded
            .message()
            .body()
            .map(|b| {
                b.trailers()
                    .map(|t| {
                        (
                            t.token.to_str_lossy().into_owned(),
                            t.value.to_str_lossy().into_owned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let has_signature = decoded.extra_headers().pgp_signature().is_some();
        Ok(CommitDetail {
            commit,
            message,
            trailers,
            has_signature,
        })
    }

    fn commit_changes(&self, id: &Oid) -> Result<Vec<FileChange>> {
        let t0 = std::time::Instant::now();
        let repo = self.repo();
        let gid = parse_oid(id)?;
        let c = repo.find_commit(gid)?;
        let tree = c.tree()?;
        let parent_tree = match c.parent_ids().next() {
            Some(p) => Some(repo.find_commit(p.detach())?.tree()?),
            None => None,
        };
        let changes = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
        let mut out = Vec::with_capacity(changes.len());
        for ch in changes {
            use gix::object::tree::diff::ChangeDetached as C;
            let (path, old_path, kind, old_id, new_id, mode) = match ch {
                C::Addition {
                    location,
                    entry_mode,
                    id,
                    ..
                } => (location, None, ChangeKind::Added, None, Some(id), entry_mode),
                C::Deletion {
                    location,
                    entry_mode,
                    id,
                    ..
                } => (location, None, ChangeKind::Deleted, Some(id), None, entry_mode),
                C::Modification {
                    location,
                    previous_entry_mode,
                    previous_id,
                    entry_mode,
                    id,
                } => {
                    let kind = if previous_entry_mode.is_blob() != entry_mode.is_blob() {
                        ChangeKind::TypeChanged
                    } else {
                        ChangeKind::Modified
                    };
                    (location, None, kind, Some(previous_id), Some(id), entry_mode)
                }
                C::Rewrite {
                    source_location,
                    source_id,
                    entry_mode,
                    id,
                    location,
                    copy,
                    ..
                } => {
                    let kind = if copy {
                        ChangeKind::Copied
                    } else {
                        ChangeKind::Renamed
                    };
                    (
                        location,
                        Some(source_location),
                        kind,
                        Some(source_id),
                        Some(id),
                        entry_mode,
                    )
                }
            };
            if mode.is_tree() {
                continue; // directory-level markers emitted by rewrite tracking — leaves carry the real changes
            }
            let path = path.to_str_lossy().into_owned();
            let old_path = old_path.map(|p| p.to_str_lossy().into_owned());
            let (additions, deletions, binary) = if mode.is_blob() {
                line_stats(&repo, old_id, new_id)
            } else {
                (None, None, false)
            };
            out.push(FileChange {
                path,
                old_path,
                kind,
                additions,
                deletions,
                binary,
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        self.console.read(
            format!("git show --numstat {}", id.short(10)),
            t0.elapsed(),
            format!("{} files", out.len()),
        );
        Ok(out)
    }

    fn blob(&self, rev: &BlobRev, path: &str) -> Result<Option<Vec<u8>>> {
        let repo = self.repo();
        match rev {
            BlobRev::Commit(id) => {
                let tree = repo.find_commit(parse_oid(id)?)?.tree()?;
                self.blob_in_tree(&repo, &tree, path)
            }
            BlobRev::ParentOf(id) => {
                let c = repo.find_commit(parse_oid(id)?)?;
                match c.parent_ids().next() {
                    Some(p) => {
                        let tree = repo.find_commit(p.detach())?.tree()?;
                        self.blob_in_tree(&repo, &tree, path)
                    }
                    None => Ok(None),
                }
            }
            BlobRev::Head => match repo.head_tree() {
                Ok(tree) => self.blob_in_tree(&repo, &tree, path),
                Err(_) => Ok(None), // unborn HEAD
            },
            BlobRev::Index => {
                let index = repo.index_or_empty()?;
                let Some(entry) = index.entry_by_path(path.into()) else {
                    return Ok(None);
                };
                if entry.mode.is_submodule()
                    || entry
                        .mode
                        .to_tree_entry_mode()
                        .is_none_or(|m| !m.is_blob_or_symlink())
                {
                    return Ok(None);
                }
                Ok(Some(repo.find_blob(entry.id)?.data.clone()))
            }
            BlobRev::Worktree => {
                let Some(workdir) = repo.workdir() else {
                    return Ok(None);
                };
                let full = workdir.join(path);
                match std::fs::read(&full) {
                    Ok(data) => Ok(Some(data)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(e).with_context(|| format!("reading {}", full.display())),
                }
            }
        }
    }
}

/// Number of commits reachable from `from` but not from `hidden`, bounded.
fn count_reachable_hidden(repo: &gix::Repository, from: gix::ObjectId, hidden: gix::ObjectId) -> usize {
    repo.rev_walk([from])
        .with_hidden([hidden])
        .all()
        .map(|walk| walk.take(MAX_AHEAD_BEHIND_WALK).filter(|r| r.is_ok()).count())
        .unwrap_or(0)
}
