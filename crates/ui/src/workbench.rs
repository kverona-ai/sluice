//! The root view: window chrome, tool rail, and the active workspace
//! (Log / Local Changes / Console). All git work runs on the background
//! executor; the view only owns snapshots.

use crate::i18n::tr;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::WindowControlArea;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollStrategy, StatefulInteractiveElement, Styled, Task, UniformListScrollHandle,
    Window, actions, div, px,
};
use gpui_component::input::{InputEvent, InputState};
use sluice_core::diff::{DiffOptions, FileDiff};
use sluice_core::*;
use sluice_domain::{ChangesSnapshot, DetailSnapshot, LogFilter, LogSnapshot, Repo};
use sluice_watch::RepoWatcher;

use crate::diff_view::DiffView;
use crate::file_view::FileView;
use crate::icons::{icon, icon_b};
use crate::overlays::{CtxMenu, Overlay};
use crate::theme::{FONT_BODY, FONT_HEADING, Theme};
use gpui_component::tooltip::Tooltip;
use sluice_backend_cli::{SnapshotEntry, StashEntry};

actions!(
    workbench,
    [
        MoveUp,
        MoveDown,
        SelectFirst,
        SelectLast,
        PageUp,
        PageDown,
        ShowLog,
        ShowChanges,
        ShowConsole,
        Refresh,
        FocusSearch,
        Escape,
        NextHunk,
        PrevHunk,
        ToggleSideBySide,
        CommitAction,
        FocusCommit,
        OpenBranches,
        OpenStash,
        OpenSnapshots,
        OpenSettings,
        OpenPush,
        ToggleTheme,
        ToggleLang,
        OpenUserFilter,
        OpenDateFilter,
        OpenPathFilter,
        OpenMessageHistory,
        OpenWorktrees,
        OpenFileHistory,
        OpenBlame,
        OpenAiConnect,
        OpenProposals,
        OpenRepository,
        OpenRecent,
        RebaseMoveUp,
        RebaseMoveDown,
        RebaseFromSelection,
        ConflictOurs,
        ConflictTheirs,
        ConflictBoth,
        ConflictResolve,
        ProposalAccept,
        ProposalReject,
        StageAll,
        UnstageAll,
        ToggleSelected,
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Log,
    Console,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Changes => "Local Changes",
            Tab::Log => "Log",
            Tab::Console => "Console",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popup {
    Users,
    Date,
    Paths,
    /// Recent commit messages (commit panel ⟲).
    Messages,
}

/// Which working-tree file the Changes tab is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkFile {
    pub path: String,
    pub old_path: Option<String>,
    pub staged: bool,
}

pub struct Workbench {
    pub repo: Repo,
    pub theme: Theme,
    pub tab: Tab,
    pub focus: FocusHandle,
    pub status: Option<String>,
    // ----- log -----
    pub query: LogQuery,
    pub log: Option<Arc<LogSnapshot>>,
    pub log_loading: bool,
    pub log_error: Option<String>,
    pub filter: LogFilter,
    pub visible: Vec<usize>,
    /// Index into `visible`.
    pub selected: usize,
    pub detail: Option<Arc<DetailSnapshot>>,
    pub detail_for: Option<Oid>,
    pub selected_ref: Option<String>,
    pub search: Entity<InputState>,
    pub popup: Option<(Popup, gpui::Point<gpui::Pixels>)>,
    pub scroll: UniformListScrollHandle,
    /// Diff of a file of the selected commit, shown in the log center.
    pub commit_diff: Option<DiffView>,
    pub diff_opts: DiffOptions,
    pub side_by_side: bool,
    // ----- changes (M2) -----
    pub changes: Option<Arc<ChangesSnapshot>>,
    pub changes_loading: bool,
    pub changes_error: Option<String>,
    pub work_file: Option<WorkFile>,
    pub work_diff: Option<DiffView>,
    /// hunk/line selection for partial staging: (hunk, line) → deselected
    pub deselected: HashMap<(usize, usize), ()>,
    pub commit_msg: Entity<InputState>,
    pub author_input: Entity<InputState>,
    pub amend: bool,
    pub signoff: bool,
    pub no_verify: bool,
    pub commit_busy: bool,
    pub ai_busy: bool,
    pub ai_tool: Option<(String, String)>,
    pub pending_ai_message: Option<String>,
    pub clear_msg_pending: bool,
    // ----- console -----
    pub console_filter: Option<ConsoleKind>,
    pub console_verbose: bool,
    pub sidebar_hidden: bool,
    pub rail_expanded: bool,
    pub overlay: Option<Overlay>,
    pub file_view: Option<FileView>,
    pub ctx_menu: Option<CtxMenu>,
    pub branch_filter: Entity<InputState>,
    pub new_branch_name: Entity<InputState>,
    pub stash_msg: Entity<InputState>,
    pub stash_untracked: bool,
    pub stashes: Vec<StashEntry>,
    pub snapshots: Vec<SnapshotEntry>,
    pub push_lease: bool,
    pub push_upstream: bool,
    pub telemetry: bool,
    pub ai_status: Vec<sluice_bridge::connect::ToolStatus>,
    pub ai_report: Option<String>,
    pub ai_busy_connect: bool,
    pub proposals: Vec<crate::proposals::PendingProposal>,
    pub recent: Vec<crate::recent::RecentRepo>,
    pub rebase: Option<crate::rebase::RebaseDraft>,
    pub conflict: Option<crate::conflict::ConflictView>,
    pub paths_input: Entity<InputState>,
    pub worktrees: Vec<sluice_backend_cli::WorktreeEntry>,
    pub worktree_branch: Entity<InputState>,
    pub settings: crate::recent::Settings,
    pub fetch_busy: bool,
    pub rebase_msg: Entity<InputState>,
    pub provenance_cache: Option<(Oid, Vec<sluice_bridge::provenance::SessionMatch>)>,
    pub pending_askpass_prompt: Option<(String, std::sync::mpsc::Sender<Option<String>>)>,
    pub askpass_input: Entity<InputState>,
    /// Selection history for the ◀ ▶ title-bar buttons (commit ids).
    pub history: Vec<Oid>,
    pub history_ix: usize,
    // ----- infra -----
    _watcher: Option<RepoWatcher>,
    _watch_task: Option<Task<()>>,
    load_gen: u64,
}

impl Workbench {
    pub fn new(repo: Repo, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = crate::recent::load_settings();
        crate::i18n::set_lang(if settings.lang == "en" {
            crate::i18n::Lang::En
        } else {
            crate::i18n::Lang::Zh
        });
        let focus = cx.focus_handle();
        focus.focus(window);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Text or hash"));
        let commit_msg = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(5)
                .placeholder(tr("在此撰写提交信息，或让已登录的 AI CLI 生成（零 API key）"))
        });
        let author_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("Name <email>（留空沿用 git 配置）")));
        let branch_filter = cx.new(|cx| InputState::new(window, cx).placeholder(tr("搜索分支 / 标签")));
        let new_branch_name = cx.new(|cx| InputState::new(window, cx).placeholder("feat/my-branch"));
        let stash_msg = cx.new(|cx| InputState::new(window, cx).placeholder(tr("stash 说明（可选）")));
        let paths_input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("src/ 或 README.md")));
        cx.subscribe(&paths_input, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                let text = this.paths_input.read(cx).value().trim().to_string();
                if !text.is_empty() && !this.filter.paths.contains(&text) {
                    this.filter.paths.push(text);
                    this.recompute_path_filter(cx);
                }
            }
        })
        .detach();
        let worktree_branch =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("新分支名（如 feat/agent-2）")));
        let rebase_msg = cx.new(|cx| InputState::new(window, cx).placeholder(tr("新的提交信息")));
        cx.subscribe(&rebase_msg, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                let text = this.rebase_msg.read(cx).value().to_string();
                if let Some(d) = this.rebase.as_mut()
                    && let Some(it) = d.items.get_mut(d.selected)
                {
                    it.message = if text.trim().is_empty() { None } else { Some(text) };
                }
                cx.notify();
            }
        })
        .detach();
        let askpass_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(tr("密码 / token / passphrase"))
        });
        cx.subscribe(&branch_filter, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&search, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change | InputEvent::PressEnter { .. }) {
                this.filter.text = this.search.read(cx).value().to_string();
                this.apply_filter(cx);
            }
        })
        .detach();
        // ⌘↩ inside the message editor commits (the editor owns `secondary-enter`).
        cx.subscribe(&commit_msg, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::PressEnter { secondary: true }) {
                this.commit_from_editor(cx);
            }
        })
        .detach();

        let mut this = Workbench {
            repo,
            theme: Theme::light(),
            tab: Tab::Log,
            focus,
            status: None,
            query: LogQuery::default(),
            log: None,
            log_loading: false,
            log_error: None,
            filter: LogFilter::default(),
            visible: Vec::new(),
            selected: 0,
            detail: None,
            detail_for: None,
            selected_ref: None,
            search,
            popup: None,
            scroll: UniformListScrollHandle::new(),
            commit_diff: None,
            diff_opts: DiffOptions::default(),
            side_by_side: true,
            changes: None,
            changes_loading: false,
            changes_error: None,
            work_file: None,
            work_diff: None,
            deselected: HashMap::new(),
            commit_msg,
            author_input,
            amend: false,
            signoff: false,
            no_verify: false,
            commit_busy: false,
            ai_busy: false,
            ai_tool: crate::ai::detect_tool(),
            pending_ai_message: None,
            clear_msg_pending: false,
            console_filter: None,
            console_verbose: false,
            sidebar_hidden: false,
            rail_expanded: false,
            overlay: None,
            file_view: None,
            ctx_menu: None,
            branch_filter,
            new_branch_name,
            stash_msg,
            stash_untracked: true,
            stashes: Vec::new(),
            snapshots: Vec::new(),
            push_lease: false,
            push_upstream: false,
            telemetry: false,
            ai_status: Vec::new(),
            ai_report: None,
            ai_busy_connect: false,
            proposals: Vec::new(),
            recent: Vec::new(),
            rebase: None,
            conflict: None,
            paths_input,
            worktrees: Vec::new(),
            worktree_branch,
            settings: settings.clone(),
            fetch_busy: false,
            rebase_msg,
            provenance_cache: None,
            pending_askpass_prompt: None,
            askpass_input,
            history: Vec::new(),
            history_ix: 0,
            _watcher: None,
            _watch_task: None,
            load_gen: 0,
        };
        // Apply persisted settings.
        if this.settings.dark {
            this.theme = Theme::dark();
            crate::sync_component_theme(cx, &this.theme);
        }
        this.telemetry = this.settings.telemetry;
        this.rail_expanded = this.settings.rail_expanded;
        crate::i18n::set_lang(if this.settings.lang == "en" {
            crate::i18n::Lang::En
        } else {
            crate::i18n::Lang::Zh
        });
        this.start_watcher(cx);
        this.reload_log(cx);
        this.reload_changes(cx);
        this.start_background_fetch(cx);
        this
    }

    pub fn toggle_lang(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let en = crate::i18n::lang() == crate::i18n::Lang::Zh;
        crate::i18n::set_lang(if en {
            crate::i18n::Lang::En
        } else {
            crate::i18n::Lang::Zh
        });
        self.settings.lang = if en { "en".into() } else { "zh".into() };
        self.save_settings();
        // Placeholders are captured at construction; refresh them for the new language.
        self.commit_msg.update(cx, |st, cx| {
            st.set_placeholder(
                tr("在此撰写提交信息，或让已登录的 AI CLI 生成（零 API key）"),
                window,
                cx,
            )
        });
        self.author_input.update(cx, |st, cx| {
            st.set_placeholder(tr("Name <email>（留空沿用 git 配置）"), window, cx)
        });
        self.branch_filter
            .update(cx, |st, cx| st.set_placeholder(tr("搜索分支 / 标签"), window, cx));
        self.stash_msg.update(cx, |st, cx| {
            st.set_placeholder(tr("stash 说明（可选）"), window, cx)
        });
        self.paths_input.update(cx, |st, cx| {
            st.set_placeholder(tr("src/ 或 README.md"), window, cx)
        });
        self.rebase_msg
            .update(cx, |st, cx| st.set_placeholder(tr("新的提交信息"), window, cx));
        self.toast(
            if en {
                "Language: English"
            } else {
                "语言：中文"
            },
            cx,
        );
        cx.notify();
    }

    pub fn save_settings(&mut self) {
        self.settings.dark = self.theme.is_dark;
        self.settings.telemetry = self.telemetry;
        self.settings.rail_expanded = self.rail_expanded;
        crate::recent::save_settings(&self.settings);
    }

    /// Recompute the commit id set for the Paths filter (`git log -- <paths>`) off-thread.
    pub fn recompute_path_filter(&mut self, cx: &mut Context<Self>) {
        self.popup = self.popup.filter(|(p, _)| *p == Popup::Paths);
        let paths = self.filter.paths.clone();
        if paths.is_empty() {
            self.filter.path_ids = None;
            self.apply_filter(cx);
            return;
        }
        let Some(cli) = self.repo.cli.clone() else { return };
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let mut args: Vec<String> = vec!["log".into(), "--format=%H".into(), "--".into()];
                    args.extend(paths.iter().cloned());
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let out = cli.run_read(&refs)?;
                    let set: std::collections::HashSet<Oid> =
                        out.stdout_str().lines().map(|l| Oid::new(l.trim())).collect();
                    anyhow::Ok(set)
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(set) => this.filter.path_ids = Some(set),
                    Err(e) => this.toast(tf!("路径过滤失败：{}", format!("{e:#}")), cx),
                }
                this.apply_filter(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Periodic `git fetch --prune` (05 §4 background fetch); interval from settings.
    pub fn start_background_fetch(&mut self, cx: &mut Context<Self>) {
        let minutes = self.settings.fetch_minutes;
        if minutes == 0 {
            return;
        }
        let Some(cli) = self.repo.cli.clone() else { return };
        if self.repo.info.head.upstream.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(minutes as u64 * 60))
                    .await;
                let still = this
                    .update(cx, |this, _| !this.fetch_busy && !this.commit_busy)
                    .unwrap_or(false);
                if !still {
                    continue;
                }
                this.update(cx, |this, _| this.fetch_busy = true).ok();
                let cli2 = cli.clone();
                let res = cx.background_spawn(async move { cli2.fetch(None, true) }).await;
                if this
                    .update(cx, |this, cx| {
                        this.fetch_busy = false;
                        if let Err(e) = res {
                            tracing::debug!("background fetch: {e:#}");
                        } else {
                            this.refresh(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    // ----- helpers -----------------------------------------------------------

    pub fn title(&self) -> String {
        let branch = self
            .repo
            .info
            .head
            .branch
            .clone()
            .unwrap_or_else(|| "detached HEAD".into());
        format!("{} — {}", self.repo.info.name, branch)
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        let log = self.log.as_ref()?;
        let ix = *self.visible.get(self.selected)?;
        log.commits.get(ix)
    }

    pub fn toast(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        self.status = Some(msg.into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            this.update(cx, |this, cx| {
                this.status = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- loading ------------------------------------------------------------

    fn start_watcher(&mut self, cx: &mut Context<Self>) {
        match self.repo.watch() {
            Ok((watcher, rx)) => {
                self._watcher = Some(watcher);
                let task = cx.spawn(async move |this, cx| {
                    while let Ok(first) = rx.recv().await {
                        // debounce: absorb the burst, then reload once
                        cx.background_executor().timer(Duration::from_millis(150)).await;
                        let mut ev = first;
                        while let Ok(more) = rx.try_recv() {
                            ev.git_meta |= more.git_meta;
                            ev.worktree |= more.worktree;
                        }
                        if this
                            .update(cx, |this, cx| {
                                if ev.git_meta {
                                    this.reload_log(cx);
                                }
                                this.reload_changes(cx);
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                self._watch_task = Some(task);
            }
            Err(e) => self.status = Some(tf!("watcher 未启动：{}", format!("{e:#}"))),
        }
    }

    pub fn reload_log(&mut self, cx: &mut Context<Self>) {
        self.load_gen += 1;
        let generation = self.load_gen;
        self.log_loading = true;
        cx.notify();
        let reader = self.repo.reader.clone();
        let query = self.query.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { LogSnapshot::load(&*reader, &query) })
                .await;
            this.update(cx, |this, cx| {
                if generation != this.load_gen {
                    return;
                }
                this.log_loading = false;
                match res {
                    Ok(snap) => {
                        this.repo.info = snap.info.clone();
                        this.log = Some(Arc::new(snap));
                        this.log_error = None;
                        this.apply_filter(cx);
                    }
                    Err(e) => {
                        this.log_error = Some(format!("{e:#}"));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn reload_changes(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else { return };
        self.changes_loading = true;
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { ChangesSnapshot::load(&cli) })
                .await;
            this.update(cx, |this, cx| {
                this.changes_loading = false;
                match res {
                    Ok(snap) => {
                        this.changes = Some(Arc::new(snap));
                        this.changes_error = None;
                        // keep the open work diff fresh
                        if let Some(wf) = this.work_file.clone() {
                            let still_there = this.changes.as_ref().is_some_and(|c| {
                                c.status.entries.iter().any(|e| {
                                    e.path == wf.path
                                        && if wf.staged { e.is_staged() } else { e.is_unstaged() }
                                })
                            });
                            if still_there {
                                this.open_work_file(wf, cx);
                            } else {
                                this.work_file = None;
                                this.work_diff = None;
                            }
                        }
                    }
                    Err(e) => this.changes_error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Re-run the filter over the loaded log and keep the selection sensible.
    pub fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let prev_id = self.selected_commit().map(|c| c.id.clone());
        if let Some(log) = &self.log {
            self.visible = self.filter.apply(&log.commits);
        } else {
            self.visible.clear();
        }
        self.selected = prev_id
            .and_then(|id| self.log.as_ref()?.row_of(&id))
            .and_then(|row| self.visible.iter().position(|v| *v == row))
            .unwrap_or(0);
        self.ensure_detail(cx);
        cx.notify();
    }

    fn ensure_detail(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.selected_commit().cloned() else {
            self.detail = None;
            self.detail_for = None;
            return;
        };
        if self.detail_for.as_ref() == Some(&c.id) && self.detail.is_some() {
            return;
        }
        self.detail_for = Some(c.id.clone());
        self.detail = None;
        self.commit_diff = None;
        let reader = self.repo.reader.clone();
        let id = c.id.clone();
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let res = cx
                .background_spawn(async move { DetailSnapshot::load(&*reader, &id2) })
                .await;
            this.update(cx, |this, cx| {
                if this.detail_for.as_ref() == Some(&id) {
                    match res {
                        Ok(d) => this.detail = Some(Arc::new(d)),
                        Err(e) => this.status = Some(tf!("读取提交详情失败：{}", format!("{e:#}"))),
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ----- selection & navigation ---------------------------------------------

    pub fn select(&mut self, visible_ix: usize, cx: &mut Context<Self>) {
        if self.visible.is_empty() {
            return;
        }
        let ix = visible_ix.min(self.visible.len() - 1);
        self.selected = ix;
        self.scroll_into_view(ix);
        self.ensure_detail(cx);
        if let Some(c) = self.selected_commit() {
            let id = c.id.clone();
            if self.history.get(self.history_ix) != Some(&id) {
                self.history.truncate(self.history_ix + 1);
                self.history.push(id);
                self.history_ix = self.history.len() - 1;
            }
        }
        cx.notify();
    }

    fn history_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.history.is_empty() {
            return;
        }
        let next = (self.history_ix as isize + delta).clamp(0, self.history.len() as isize - 1) as usize;
        if next == self.history_ix {
            return;
        }
        self.history_ix = next;
        let id = self.history[next].clone();
        let Some(row) = self.log.as_ref().and_then(|l| l.row_of(&id)) else {
            return;
        };
        if let Some(vix) = self.visible.iter().position(|v| *v == row) {
            self.selected = vix;
            self.scroll_into_view(vix);
            self.ensure_detail(cx);
            self.tab = Tab::Log;
            cx.notify();
        }
    }

    /// Record an app-level note in the Console (not a git command).
    pub fn console_note(&mut self, what: &str, detail: &str) {
        self.repo.console.note(what, detail);
    }

    /// Oid of the commit currently selected in the Log (None when nothing/empty).
    pub fn selected_commit_id(&self) -> Option<Oid> {
        let log = self.log.as_ref()?;
        let ix = *self.visible.get(self.selected)?;
        log.commits.get(ix).map(|c| c.id.clone())
    }

    /// Keyboard entry for file history / blame: acts on the file currently open or
    /// selected (work file in Changes, open diff or first changed file in Log).
    pub fn file_action(&mut self, mode: crate::file_view::FileViewMode, cx: &mut Context<Self>) {
        let (path, rev) = match self.tab {
            Tab::Changes => (self.work_file.as_ref().map(|w| w.path.clone()), None),
            _ => {
                let sha = self.detail.as_ref().map(|d| d.detail.commit.id.to_string());
                let path = self
                    .commit_diff
                    .as_ref()
                    .map(|dv| dv.change.path.clone())
                    .or_else(|| {
                        self.detail
                            .as_ref()
                            .and_then(|d| d.changes.first().map(|c| c.path.clone()))
                    });
                (path, sha)
            }
        };
        match path {
            Some(p) => self.open_file_view(p, rev, mode, cx),
            None => self.toast("先选中一个文件（提交详情里的文件或本地变更）", cx),
        }
    }

    /// Switch between the light and dark Broadsheet palettes (prototype THEMES).
    pub fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.theme = if self.theme.is_dark {
            Theme::light()
        } else {
            Theme::dark()
        };
        crate::sync_component_theme(cx, &self.theme);
        self.toast(
            if self.theme.is_dark {
                tr("已切换到深色主题")
            } else {
                tr("已切换到浅色主题")
            },
            cx,
        );
        cx.notify();
    }

    /// Toast for controls whose feature lands in a later milestone — never a dead click.
    pub fn not_yet(&mut self, what: &str, when: &str, cx: &mut Context<Self>) {
        self.toast(tf!("{} —— {} 提供", what, when), cx);
    }

    fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.tab == Tab::Changes {
            self.move_work_file(delta, cx);
            return;
        }
        let n = self.visible.len();
        if n == 0 {
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, n as isize - 1) as usize;
        self.select(next, cx);
    }

    /// `ScrollStrategy` has no "nearest": scroll only when the row is outside the viewport.
    fn scroll_into_view(&self, ix: usize) {
        let state = self.scroll.0.borrow();
        let Some(item) = state.last_item_size else {
            drop(state);
            self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
            return;
        };
        let item_h = item.item.height;
        let viewport_h = state.base_handle.bounds().size.height;
        let offset_y = -state.base_handle.offset().y;
        drop(state);
        if item_h <= px(0.) || viewport_h <= px(0.) {
            return;
        }
        let first = (offset_y / item_h).floor() as usize;
        let visible = ((viewport_h / item_h).floor() as usize).max(1);
        let last = first + visible - 1;
        if ix < first {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        } else if ix > last {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Bottom);
        }
    }

    pub fn pick_ref(&mut self, full_name: Option<String>, cx: &mut Context<Self>) {
        let tips = match (&full_name, &self.log) {
            (Some(name), Some(log)) => log
                .refs
                .iter()
                .filter(|r| &r.full_name == name)
                .map(|r| r.target.clone())
                .collect(),
            _ => Vec::new(),
        };
        self.selected_ref = full_name;
        self.query.tips = tips;
        self.selected = 0;
        self.detail = None;
        self.detail_for = None;
        self.commit_diff = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.reload_log(cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload_log(cx);
        self.reload_changes(cx);
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.popup = None;
        cx.notify();
    }

    /// Open the diff of `path` of the currently selected commit in the log center.
    pub fn open_commit_file(&mut self, change: FileChange, cx: &mut Context<Self>) {
        let Some(c) = self.selected_commit().cloned() else {
            return;
        };
        let reader = self.repo.reader.clone();
        let opts = self.diff_opts;
        let id = c.id.clone();
        let title = change.path.clone();
        self.commit_diff = Some(DiffView::loading(title.clone(), change.clone()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = change.path.clone();
            let old_path = change.old_path.clone();
            let id2 = id.clone();
            let res = cx
                .background_spawn(async move {
                    sluice_domain::diff_commit_file(&*reader, &id2, &path, old_path.as_deref(), &opts)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(dv) = &mut this.commit_diff
                    && dv.title == title
                {
                    dv.set_result(res.map_err(|e| format!("{e:#}")));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn close_diff(&mut self, cx: &mut Context<Self>) {
        if self.conflict.is_some() {
            self.conflict = None;
            cx.notify();
            return;
        }
        if self.file_view.is_some() {
            self.file_view = None;
            self.popup = None;
            cx.notify();
            return;
        }
        match self.tab {
            Tab::Log => self.commit_diff = None,
            Tab::Changes => {
                self.work_file = None;
                self.work_diff = None;
            }
            Tab::Console => {}
        }
        self.popup = None;
        cx.notify();
    }

    fn rediff(&mut self, cx: &mut Context<Self>) {
        if let Some(dv) = &self.commit_diff {
            let change = dv.change.clone();
            self.open_commit_file(change, cx);
        }
        if let Some(wf) = self.work_file.clone() {
            self.open_work_file(wf, cx);
        }
    }

    pub fn set_context(&mut self, ctx: usize, cx: &mut Context<Self>) {
        self.diff_opts.context = ctx;
        self.rediff(cx);
    }

    pub fn toggle_ignore_ws(&mut self, cx: &mut Context<Self>) {
        self.diff_opts.ignore_whitespace = !self.diff_opts.ignore_whitespace;
        self.rediff(cx);
    }

    fn active_diff_mut(&mut self) -> Option<&mut DiffView> {
        match self.tab {
            Tab::Log => self.commit_diff.as_mut(),
            Tab::Changes => self.work_diff.as_mut(),
            Tab::Console => None,
        }
    }

    pub fn jump_hunk(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(dv) = self.active_diff_mut() {
            dv.jump_hunk(delta);
            cx.notify();
        }
    }

    // ----- chrome ---------------------------------------------------------------

    fn render_titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let repo_name = self.repo.info.name.clone();
        let branch = self
            .repo
            .info
            .head
            .branch
            .clone()
            .unwrap_or_else(|| "detached HEAD".into());
        let tabs = [Tab::Changes, Tab::Log, Tab::Console];
        let active = self.tab;
        let is_mac = cfg!(target_os = "macos");
        let pending = self.changes.as_ref().map(|c| c.status.entries.len()).unwrap_or(0);
        div()
            .id("titlebar")
            .relative()
            .h(px(40.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.line_soft)
            .when(is_mac, |d| {
                d.on_click(|ev: &ClickEvent, window, _| {
                    if ev.click_count() == 2 {
                        window.titlebar_double_click();
                    }
                })
            })
            .when(!is_mac, |d| d.window_control_area(WindowControlArea::Drag))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_family(FONT_HEADING)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(13.5))
                                    .child("sluice"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(t.muted)
                                    .child(format!("{repo_name} — {branch}")),
                            ),
                    ),
            )
            .when(is_mac, |d| d.child(div().w(px(64.)).flex_none()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .flex_none()
                    .child(
                        chrome_button(
                            "tb-sidebar",
                            &t,
                            "sidebar-simple",
                            tr("显示 / 隐藏侧栏"),
                            self.sidebar_hidden,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar_hidden = !this.sidebar_hidden;
                            cx.notify();
                        })),
                    )
                    .child(
                        chrome_button("tb-back", &t, "caret-left", tr("上一个选中的提交"), false)
                            .when(self.history_ix == 0, |d| d.opacity(0.4))
                            .on_click(cx.listener(|this, _, _, cx| this.history_step(-1, cx))),
                    )
                    .child(
                        chrome_button("tb-fwd", &t, "caret-right", tr("下一个选中的提交"), false)
                            .when(self.history_ix + 1 >= self.history.len().max(1), |d| {
                                d.opacity(0.4)
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.history_step(1, cx))),
                    ),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .flex_none()
                    .child(
                        div()
                            .flex()
                            .gap(px(2.))
                            .p(px(2.))
                            .rounded(px(8.))
                            .bg(t.ink_08)
                            .children(tabs.into_iter().enumerate().map(|(ix, tab)| {
                                let on = tab == active;
                                div()
                                    .id(("tab", ix))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .px(px(12.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .text_size(px(12.))
                                    .text_color(t.ink)
                                    .cursor_pointer()
                                    .when(on, |d| {
                                        d.bg(t.surface)
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .shadow_sm()
                                    })
                                    .when(!on, |d| d.hover(move |st| st.bg(t.ink_05)))
                                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                                    .child(tab.label())
                                    .when(tab == Tab::Changes && pending > 0, |d| {
                                        d.child(
                                            div()
                                                .px(px(5.))
                                                .rounded(px(7.))
                                                .bg(t.cyan)
                                                .text_color(t.surface)
                                                .text_size(px(10.))
                                                .line_height(px(14.))
                                                .child(pending.to_string()),
                                        )
                                    })
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                chrome_button("tb-ai", &t, "sparkle", tr("AI 工具接入（一键 MCP）"), false)
                                    .on_click(cx.listener(|this, _, _, cx| this.open_ai_connect(cx))),
                            )
                            .child(
                                chrome_button("tb-refresh", &t, "arrow-clockwise", tr("刷新 ⌥⌘Y"), false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh(cx);
                                        this.toast("已刷新", cx);
                                    })),
                            )
                            .child(
                                chrome_button("tb-search", &t, "magnifying-glass", tr("搜索提交 ⌘F"), false)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.focus_search(window, cx)),
                                    ),
                            )
                            .child(
                                chrome_button("tb-more", &t, "dots-three-circle", tr("更多操作"), false)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.not_yet("更多操作菜单", "M2", cx)),
                                    ),
                            ),
                    )
                    .children(self.render_win_caption_buttons()),
            )
    }

    /// Windows caption buttons — drawn by us, wired to the OS via
    /// `window_control_area` (gpui 0.2.2 has no native-caption path on Windows).
    fn render_win_caption_buttons(&self) -> Option<impl IntoElement> {
        if cfg!(target_os = "macos") {
            return None;
        }
        let t = self.theme;
        let btn = |area: WindowControlArea, name: &'static str, danger: bool| {
            div()
                .w(px(44.))
                .h(px(40.))
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(area)
                .hover(move |s| {
                    if danger {
                        s.bg(gpui::rgb(0xc42b1c))
                    } else {
                        s.bg(t.ink_08)
                    }
                })
                .child(icon_b(name, px(12.), t.muted))
        };
        Some(
            div()
                .flex()
                .items_center()
                .ml(px(6.))
                .mr(px(-18.))
                .child(btn(WindowControlArea::Min, "minus", false))
                .child(btn(WindowControlArea::Max, "square", false))
                .child(btn(WindowControlArea::Close, "x", true)),
        )
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = Tab::Log;
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let tab = self.tab;
        let expanded = self.rail_expanded;
        let item = |id: &'static str,
                    icon: &'static str,
                    label: &'static str,
                    tip: &'static str,
                    active: bool,
                    tone: Option<gpui::Rgba>| {
            rail_item(id, &t, icon, label, tip, active, expanded, tone)
        };
        div()
            .w(px(if expanded { 118. } else { 34. }))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .pt(px(6.))
            .pb(px(2.))
            .bg(t.ink_05)
            .border_r_1()
            .border_color(t.line_55)
            .child(
                rail_item(
                    "rail-expand",
                    &t,
                    if expanded { "caret-left" } else { "caret-right" },
                    tr("收起"),
                    if expanded {
                        tr("收起工具栏")
                    } else {
                        tr("展开工具栏（图标旁显示说明）")
                    },
                    false,
                    expanded,
                    None,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.rail_expanded = !this.rail_expanded;
                    cx.notify();
                })),
            )
            .child(
                item(
                    "rail-log",
                    "git-branch",
                    tr("日志"),
                    tr("日志 / 提交图 ⌘9"),
                    tab == Tab::Log,
                    None,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Log, cx))),
            )
            .child(
                item(
                    "rail-changes",
                    "git-commit",
                    tr("变更"),
                    tr("本地变更 / 提交 ⌘0"),
                    tab == Tab::Changes,
                    None,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Changes, cx))),
            )
            .child(
                item(
                    "rail-branches",
                    "git-merge",
                    tr("分支"),
                    tr("分支面板 ⌃⇧`"),
                    false,
                    None,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_branches(cx))),
            )
            .child(
                item(
                    "rail-pull",
                    "arrow-line-down",
                    tr("拉取"),
                    tr("拉取（git pull）"),
                    false,
                    None,
                )
                .on_click(cx.listener(|this, _, _, cx| this.git_pull(cx))),
            )
            .child(
                item(
                    "rail-push",
                    "arrow-line-up",
                    tr("推送"),
                    tr("推送对话框 ⌘⇧K"),
                    false,
                    None,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_push(cx))),
            )
            .child(
                item("rail-stash", "tray", "Stash", tr("Stash 列表"), false, None)
                    .on_click(cx.listener(|this, _, _, cx| this.open_stashes(cx))),
            )
            .child(
                item(
                    "rail-time",
                    "clock-counter-clockwise",
                    tr("时光机"),
                    tr("时光机 / 快照（M3 完整版）"),
                    false,
                    None,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_snapshots(cx))),
            )
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .relative()
                            .child(
                                item(
                                    "rail-ai",
                                    "sparkle",
                                    "AI",
                                    tr("AI 工具接入 ⌘⇧I / 待确认队列 ⌘⇧P"),
                                    !self.proposals.is_empty(),
                                    Some(t.mag),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.proposals.is_empty() {
                                        this.open_ai_connect(cx)
                                    } else {
                                        this.open_proposals(cx)
                                    }
                                })),
                            )
                            .when(!self.proposals.is_empty(), |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(1.))
                                        .right(px(1.))
                                        .min_w(px(14.))
                                        .h(px(14.))
                                        .px(px(3.))
                                        .rounded(px(7.))
                                        .bg(t.mag)
                                        .text_color(t.surface)
                                        .text_size(px(9.5))
                                        .line_height(px(14.))
                                        .text_center()
                                        .child(self.proposals.len().to_string()),
                                )
                            }),
                    )
                    .child(
                        item(
                            "rail-console",
                            "terminal-window",
                            "Console",
                            "Console ⌘6",
                            tab == Tab::Console,
                            None,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Console, cx))),
                    )
                    .child(
                        item(
                            "rail-settings",
                            "gear",
                            tr("设置"),
                            tr("设置 / Keymap"),
                            false,
                            None,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                    ),
            )
    }

    fn render_toast(&self) -> Option<impl IntoElement> {
        let t = self.theme;
        let msg = self.status.clone()?;
        Some(
            div()
                .absolute()
                .bottom(px(36.))
                .left(px(60.))
                .max_w(px(720.))
                .px(px(14.))
                .py(px(8.))
                .rounded(px(7.))
                .bg(t.ink)
                .text_color(t.paper)
                .text_size(px(12.5))
                .shadow_md()
                .child(msg),
        )
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let body = match self.tab {
            Tab::Log => self.render_log(window, cx).into_any_element(),
            Tab::Changes => self.render_changes(window, cx).into_any_element(),
            Tab::Console => self.render_console(cx).into_any_element(),
        };
        div()
            .key_context("Workbench")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| {
                if this.overlay == Some(crate::overlays::Overlay::Rebase) {
                    this.rebase_move_selection(-1, cx)
                } else {
                    this.move_by(-1, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| {
                if this.overlay == Some(crate::overlays::Overlay::Rebase) {
                    this.rebase_move_selection(1, cx)
                } else {
                    this.move_by(1, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ConflictOurs, _, cx| {
                this.conflict_choose_current(crate::conflict::Choice::Ours, cx)
            }))
            .on_action(cx.listener(|this, _: &ConflictTheirs, _, cx| {
                this.conflict_choose_current(crate::conflict::Choice::Theirs, cx)
            }))
            .on_action(cx.listener(|this, _: &ConflictBoth, _, cx| {
                this.conflict_choose_current(crate::conflict::Choice::Both, cx)
            }))
            .on_action(cx.listener(|this, _: &ConflictResolve, _, cx| this.conflict_save(true, cx)))
            .on_action(cx.listener(|this, _: &RebaseMoveUp, _, cx| this.rebase_reorder(-1, cx)))
            .on_action(cx.listener(|this, _: &RebaseMoveDown, _, cx| this.rebase_reorder(1, cx)))
            .on_action(cx.listener(|this, _: &RebaseFromSelection, _, cx| {
                if let Some(id) = this.selected_commit_id() {
                    this.open_rebase_from(id, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.move_by(-20, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.move_by(20, cx)))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.select(0, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| {
                let n = this.visible.len();
                this.select(n.saturating_sub(1), cx)
            }))
            .on_action(cx.listener(|this, _: &ShowLog, _, cx| this.set_tab(Tab::Log, cx)))
            .on_action(cx.listener(|this, _: &ShowChanges, _, cx| this.set_tab(Tab::Changes, cx)))
            .on_action(cx.listener(|this, _: &ShowConsole, _, cx| this.set_tab(Tab::Console, cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)))
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if !this.dismiss_top(cx) {
                    this.close_diff(cx);
                }
                this.focus.focus(window);
            }))
            .on_action(cx.listener(|this, _: &OpenBranches, _, cx| this.open_branches(cx)))
            .on_action(cx.listener(|this, _: &OpenStash, _, cx| this.open_stashes(cx)))
            .on_action(cx.listener(|this, _: &OpenSnapshots, _, cx| this.open_snapshots(cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_settings(cx)))
            .on_action(cx.listener(|this, _: &OpenPush, _, cx| this.open_push(cx)))
            .on_action(cx.listener(|this, _: &ToggleTheme, _, cx| this.toggle_theme(cx)))
            .on_action(cx.listener(|this, _: &ToggleLang, window, cx| this.toggle_lang(window, cx)))
            .on_action(cx.listener(|this, _: &OpenAiConnect, _, cx| this.open_ai_connect(cx)))
            .on_action(cx.listener(|this, _: &OpenProposals, _, cx| this.open_proposals(cx)))
            .on_action(cx.listener(|this, _: &OpenRepository, window, cx| this.pick_repository(window, cx)))
            .on_action(cx.listener(|this, _: &OpenRecent, _, cx| this.open_recent(cx)))
            .on_action(cx.listener(|this, _: &ProposalAccept, window, cx| {
                if this.overlay == Some(crate::overlays::Overlay::Proposals) {
                    this.decide_proposal(0, true, cx);
                } else if this.overlay == Some(crate::overlays::Overlay::Rebase) {
                    this.rebase_start(cx);
                } else if this.overlay == Some(crate::overlays::Overlay::Recent) {
                    let current = this.repo.cli.as_ref().map(|c| c.workdir().to_path_buf());
                    if let Some(r) = this
                        .recent
                        .iter()
                        .find(|r| Some(&r.path) != current.as_ref())
                        .cloned()
                    {
                        this.overlay = None;
                        this.switch_repository(r.path, window, cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &ProposalReject, _, cx| {
                if this.overlay == Some(crate::overlays::Overlay::Proposals) {
                    this.decide_proposal(0, false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenFileHistory, _, cx| {
                this.file_action(crate::file_view::FileViewMode::History, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenBlame, _, cx| {
                this.file_action(crate::file_view::FileViewMode::Blame, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenUserFilter, _, cx| {
                this.tab = Tab::Log;
                this.popup = Some((Popup::Users, gpui::point(px(620.), px(64.))));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenWorktrees, _, cx| this.open_worktrees(cx)))
            .on_action(cx.listener(|this, _: &OpenMessageHistory, _, cx| {
                this.tab = Tab::Changes;
                this.popup = Some((Popup::Messages, gpui::point(px(60.), px(420.))));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenPathFilter, window, cx| {
                this.tab = Tab::Log;
                this.popup = Some((Popup::Paths, gpui::point(px(730.), px(64.))));
                let fh = gpui::Focusable::focus_handle(this.paths_input.read(cx), cx);
                window.focus(&fh);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenDateFilter, _, cx| {
                this.tab = Tab::Log;
                this.popup = Some((Popup::Date, gpui::point(px(690.), px(64.))));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NextHunk, _, cx| this.jump_hunk(1, cx)))
            .on_action(cx.listener(|this, _: &PrevHunk, _, cx| this.jump_hunk(-1, cx)))
            .on_action(cx.listener(|this, _: &ToggleSideBySide, _, cx| {
                this.side_by_side = !this.side_by_side;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CommitAction, window, cx| this.commit_action(window, cx)))
            .on_action(cx.listener(|this, _: &FocusCommit, window, cx| this.focus_commit(window, cx)))
            .on_action(cx.listener(|this, _: &StageAll, _, cx| this.stage_all(cx)))
            .on_action(cx.listener(|this, _: &UnstageAll, _, cx| this.unstage_all(cx)))
            .on_action(cx.listener(|this, _: &ToggleSelected, _, cx| {
                if this.overlay == Some(crate::overlays::Overlay::Rebase) {
                    this.rebase_cycle_action(cx)
                } else {
                    this.toggle_selected_work_file(cx)
                }
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(t.paper)
            .text_color(t.ink)
            .font_family(FONT_BODY)
            .text_size(px(13.))
            .child(self.render_titlebar(cx))
            .children(self.render_win_menu(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_rail(cx))
                    .child(body),
            )
            .children(self.render_toast())
            .children(self.render_popup(cx))
            .children(self.render_overlay(window, cx))
            .children(self.render_ctx_menu(window, cx))
    }
}

impl Workbench {
    /// Windows-only mnemonic menu row (prototype win variant). Menus arrive with M4.
    fn render_win_menu(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if cfg!(target_os = "macos") {
            return None;
        }
        let t = self.theme;
        let entry = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |s| s.bg(t.ink_08))
                .child(label)
        };
        Some(
            div()
                .h(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .bg(t.paper)
                .border_b_1()
                .border_color(t.line_soft)
                .text_size(px(12.))
                .text_color(t.muted)
                .child(
                    entry("wm-file", tr("文件(F)"))
                        .on_click(cx.listener(|this, _, _, cx| this.not_yet("应用菜单", "M4", cx))),
                )
                .child(
                    entry("wm-edit", tr("编辑(E)"))
                        .on_click(cx.listener(|this, _, _, cx| this.not_yet("应用菜单", "M4", cx))),
                )
                .child(
                    entry("wm-view", tr("视图(V)"))
                        .on_click(cx.listener(|this, _, _, cx| this.not_yet("应用菜单", "M4", cx))),
                )
                .child(div().text_color(t.cyan).child(
                    entry("wm-git", "Git(G)").on_click(cx.listener(|this, _, _, cx| this.open_branches(cx))),
                ))
                .child(
                    entry("wm-ai", tr("AI 工具(A)"))
                        .on_click(cx.listener(|this, _, _, cx| this.not_yet("AI 工具菜单", "M4", cx))),
                )
                .child(
                    entry("wm-help", tr("帮助(H)"))
                        .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                )
                .child(
                    div()
                        .ml_auto()
                        .font_family(crate::theme::FONT_MONO)
                        .text_size(px(11.))
                        .text_color(t.faint)
                        .child(tr("Ctrl+K 提交 · Ctrl+Shift+K 推送")),
                ),
        )
    }
}

/// Small helper used by workspaces: an uppercase section label (11px, faint).
pub fn section_label(t: &Theme, text: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .text_color(t.faint)
        .child(text.into().to_uppercase())
}

/// Agent badge: 15×15 box with a one-glyph mark in the agent tone.
pub fn agent_badge(t: &Theme, agent: Agent) -> impl IntoElement {
    let tone = t.agent_tone(agent);
    div()
        .flex_none()
        .w(px(15.))
        .h(px(15.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(tone)
        .text_color(tone)
        .font_family(crate::theme::FONT_MONO)
        .text_size(px(9.5))
        .line_height(px(13.))
        .child(agent.mark())
}

/// 13×13 checkbox in the prototype's style (cyan when on, `–` when mixed).
pub fn checkbox(t: &Theme, on: bool, mixed: bool) -> impl IntoElement {
    let filled = on || mixed;
    div()
        .flex_none()
        .w(px(13.))
        .h(px(13.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if filled { t.cyan } else { t.line })
        .bg(if filled { t.cyan } else { t.surface })
        .text_color(t.surface)
        .font_family(crate::theme::FONT_MONO)
        .text_size(px(10.))
        .line_height(px(11.))
        .child(if mixed {
            "–"
        } else if on {
            "✓"
        } else {
            ""
        })
}

pub fn tone_icon(name: &'static str, size: f32, color: gpui::Rgba) -> impl IntoElement {
    icon(name, px(size), color)
}

/// 26×26 icon button for the title bar (hover wash, tooltip, pressed state).
pub fn chrome_button(
    id: &'static str,
    t: &Theme,
    name: &'static str,
    tip: &'static str,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    let color = if active { t.cyan_deep } else { t.muted };
    let t2 = *t;
    div()
        .id(id)
        .w(px(26.))
        .h(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .when(active, |d| d.bg(t2.cyan_16))
        .hover(move |s| s.bg(t2.ink_08))
        .active(move |s| s.bg(t2.ink_13))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(icon_b(name, px(15.), color))
}

/// Tool-rail entry: a 28×28 icon button when collapsed, icon + label row when expanded.
#[allow(clippy::too_many_arguments)]
pub fn rail_item(
    id: &'static str,
    t: &Theme,
    name: &'static str,
    label: &'static str,
    tip: &'static str,
    active: bool,
    expanded: bool,
    tone: Option<gpui::Rgba>,
) -> gpui::Stateful<gpui::Div> {
    let t2 = *t;
    let color = tone.unwrap_or(if active { t.cyan } else { t.muted });
    let base = div()
        .id(id)
        .rounded(px(7.))
        .cursor_pointer()
        .when(active, |d| d.bg(t2.cyan_16))
        .hover(move |s| s.bg(t2.ink_08))
        .active(move |s| s.bg(t2.ink_13))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx));
    if expanded {
        base.mx(px(4.))
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(6.))
            .child(icon(name, px(16.), color))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(if active { t2.cyan_deep } else { t2.ink })
                    .child(label),
            )
    } else {
        base.mx(px(3.))
            .w(px(28.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(name, px(17.), color))
    }
}

#[allow(dead_code)]
fn _unused(_: &App, _: &FileDiff) {}
