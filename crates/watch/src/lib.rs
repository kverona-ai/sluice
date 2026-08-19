//! sluice-watch — worktree + `.git` change detection (05 §4): recursive
//! `notify` watch on the working tree (skipping `.git/objects`) and on the
//! git dir's ref / index files. Events are coalesced by the consumer (the UI
//! debounces ~120ms before reloading).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchEvent {
    /// Something under the git dir changed (HEAD, refs, index, MERGE_HEAD …).
    pub git_meta: bool,
    /// A working-tree file changed.
    pub worktree: bool,
}

pub struct RepoWatcher {
    _watcher: RecommendedWatcher,
}

/// Start watching. Events are pushed to `tx` (bounded; dropped when the consumer lags —
/// that's fine, any event just means "reload").
pub fn watch(
    workdir: Option<&Path>,
    git_dir: &Path,
    tx: async_channel::Sender<WatchEvent>,
) -> Result<RepoWatcher> {
    let git_dir_owned: PathBuf = git_dir.to_path_buf();
    let objects = git_dir_owned.join("objects");
    let private = git_dir_owned.join("sluice");
    let logs = git_dir_owned.join("logs");
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else { return };
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
        ) {
            return;
        }
        let mut git_meta = false;
        let mut worktree = false;
        for p in &event.paths {
            if p.starts_with(&objects) || p.starts_with(&private) || p.starts_with(&logs) {
                continue;
            }
            if p.starts_with(&git_dir_owned) {
                // `index.lock` churn is noisy but harmless; the debounce absorbs it.
                git_meta = true;
            } else {
                worktree = true;
            }
        }
        if git_meta || worktree {
            let _ = tx.try_send(WatchEvent { git_meta, worktree });
        }
    })
    .context("creating file watcher")?;
    if let Some(wd) = workdir {
        watcher
            .watch(wd, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", wd.display()))?;
    }
    // The git dir may live outside the worktree (worktrees, GIT_DIR): watch it explicitly.
    if workdir.is_none_or(|wd| !git_dir.starts_with(wd)) {
        watcher
            .watch(git_dir, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", git_dir.display()))?;
    }
    Ok(RepoWatcher { _watcher: watcher })
}
