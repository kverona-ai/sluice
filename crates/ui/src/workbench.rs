//! The root view: window chrome, tool rail, and the active workspace
//! (Log / Local Changes / Console). All git work runs on the background
//! executor; the view only owns snapshots.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, ScrollStrategy, StatefulInteractiveElement, Styled, Task,
    UniformListScrollHandle, Window, actions, div, px,
};
use gpui_component::input::{InputEvent, InputState};
use sluice_core::diff::{DiffOptions, FileDiff};
use sluice_core::*;
use sluice_domain::{ChangesSnapshot, DetailSnapshot, LogFilter, LogSnapshot, Repo};
use sluice_watch::RepoWatcher;

use crate::diff_view::DiffView;
use crate::icons::{icon, icon_b};
use crate::theme::{FONT_BODY, FONT_HEADING, Theme};
use gpui_component::tooltip::Tooltip;

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
    pub popup: Option<Popup>,
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
        let focus = cx.focus_handle();
        focus.focus(window);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Text or hash"));
        let commit_msg = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(5)
                .placeholder("在此撰写提交信息，或让已登录的 AI CLI 生成（零 API key）")
        });
        let author_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Name <email>（留空沿用 git 配置）"));
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
            history: Vec::new(),
            history_ix: 0,
            _watcher: None,
            _watch_task: None,
            load_gen: 0,
        };
        this.start_watcher(cx);
        this.reload_log(cx);
        this.reload_changes(cx);
        this
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
            Err(e) => self.status = Some(format!("watcher 未启动：{e:#}")),
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
                        Err(e) => this.status = Some(format!("读取提交详情失败：{e:#}")),
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

    /// Toast for controls whose feature lands in a later milestone — never a dead click.
    pub fn not_yet(&mut self, what: &str, when: &str, cx: &mut Context<Self>) {
        self.toast(format!("{what} —— {when} 提供"), cx);
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
        let subtitle = self.title();
        let tabs = [Tab::Changes, Tab::Log, Tab::Console];
        let active = self.tab;
        let is_mac = cfg!(target_os = "macos");
        let pending = self.changes.as_ref().map(|c| c.status.entries.len()).unwrap_or(0);
        div()
            .id("titlebar")
            .relative()
            .h(px(if is_mac { 54. } else { 40. }))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(18.))
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.line_soft)
            .when(is_mac, |d| {
                d.on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                    .on_click(|ev: &ClickEvent, window, _| {
                        if ev.click_count() == 2 {
                            window.titlebar_double_click();
                        }
                    })
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .font_family(FONT_HEADING)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(13.5))
                                    .line_height(px(17.))
                                    .child("sluice"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .line_height(px(13.))
                                    .text_color(t.muted)
                                    .child(subtitle),
                            ),
                    ),
            )
            .when(is_mac, |d| d.child(div().w(px(56.)).flex_none()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_none()
                    .child(
                        chrome_button(
                            "tb-sidebar",
                            &t,
                            "sidebar-simple",
                            "显示 / 隐藏侧栏",
                            self.sidebar_hidden,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar_hidden = !this.sidebar_hidden;
                            cx.notify();
                        })),
                    )
                    .child(
                        chrome_button("tb-back", &t, "caret-left", "上一个选中的提交", false)
                            .when(self.history_ix == 0, |d| d.opacity(0.4))
                            .on_click(cx.listener(|this, _, _, cx| this.history_step(-1, cx))),
                    )
                    .child(
                        chrome_button("tb-fwd", &t, "caret-right", "下一个选中的提交", false)
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
                    .gap(px(14.))
                    .flex_none()
                    .child(
                        div()
                            .flex()
                            .gap(px(2.))
                            .p(px(2.))
                            .rounded(px(9.))
                            .bg(t.ink_08)
                            .children(tabs.into_iter().enumerate().map(|(ix, tab)| {
                                let on = tab == active;
                                div()
                                    .id(("tab", ix))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .px(px(14.))
                                    .py(px(3.))
                                    .rounded(px(7.))
                                    .text_size(px(12.5))
                                    .text_color(t.ink)
                                    .cursor_pointer()
                                    .when(on, |d| {
                                        d.bg(t.surface)
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .shadow_sm()
                                    })
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
                            .gap(px(4.))
                            .child(
                                chrome_button("tb-ai", &t, "sparkle", "AI 工具接入向导（M4）", false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.not_yet("AI 零配置接入向导", "M4（03 §2）", cx)
                                    })),
                            )
                            .child(
                                chrome_button("tb-refresh", &t, "arrow-clockwise", "刷新 ⌥⌘Y", false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh(cx);
                                        this.toast("已刷新", cx);
                                    })),
                            )
                            .child(
                                chrome_button("tb-search", &t, "magnifying-glass", "搜索提交 ⌘F", false)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.focus_search(window, cx)),
                                    ),
                            )
                            .child(
                                chrome_button("tb-more", &t, "dots-three-circle", "更多操作", false)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.not_yet("更多操作菜单", "M2", cx)),
                                    ),
                            ),
                    ),
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
        div()
            .w(px(34.))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .py(px(8.))
            .bg(t.ink_05)
            .border_r_1()
            .border_color(t.line_55)
            .child(
                rail_button("rail-log", &t, "git-branch", "日志 / 提交图 ⌘9", tab == Tab::Log)
                    .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Log, cx))),
            )
            .child(
                rail_button(
                    "rail-changes",
                    &t,
                    "git-commit",
                    "本地变更 / 提交 ⌘0",
                    tab == Tab::Changes,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Changes, cx))),
            )
            .child(
                rail_button("rail-merge", &t, "git-merge", "分支 / 合并 / rebase（M3）", false).on_click(
                    cx.listener(|this, _, _, cx| this.not_yet("分支面板与 merge / rebase", "M3", cx)),
                ),
            )
            .child(
                rail_button("rail-pull", &t, "arrow-line-down", "拉取（git pull）", false)
                    .on_click(cx.listener(|this, _, _, cx| this.git_pull(cx))),
            )
            .child(
                rail_button("rail-push", &t, "arrow-line-up", "推送（git push）", false)
                    .on_click(cx.listener(|this, _, _, cx| this.git_push(cx))),
            )
            .child(
                rail_button("rail-star", &t, "star", "收藏分支（P2）", false)
                    .on_click(cx.listener(|this, _, _, cx| this.not_yet("收藏分支", "P2 backlog", cx))),
            )
            .child(
                rail_button(
                    "rail-time",
                    &t,
                    "clock-counter-clockwise",
                    "时光机 / 快照（M3）",
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.not_yet("时光机（快照回溯）", "M3", cx))),
            )
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        rail_button_tone(
                            "rail-ai",
                            &t,
                            "sparkle",
                            "AI 工具 / 待确认队列（M4）",
                            false,
                            t.mag,
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.not_yet("AI 工具接入与待确认队列", "M4", cx)),
                        ),
                    )
                    .child(
                        rail_button(
                            "rail-console",
                            &t,
                            "terminal-window",
                            "Console ⌘6",
                            tab == Tab::Console,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Console, cx))),
                    )
                    .child(
                        rail_button("rail-settings", &t, "gear", "设置 / Keymap（M4）", false).on_click(
                            cx.listener(|this, _, _, cx| this.not_yet("设置与 keymap 预设", "M4", cx)),
                        ),
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
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_by(-1, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_by(1, cx)))
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
                this.close_diff(cx);
                this.focus.focus(window);
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
            .on_action(cx.listener(|this, _: &ToggleSelected, _, cx| this.toggle_selected_work_file(cx)))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(t.paper)
            .text_color(t.ink)
            .font_family(FONT_BODY)
            .text_size(px(13.))
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_rail(cx))
                    .child(body),
            )
            .children(self.render_toast())
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

/// 28×28 icon button for the left tool rail.
pub fn rail_button(
    id: &'static str,
    t: &Theme,
    name: &'static str,
    tip: &'static str,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    rail_button_tone(id, t, name, tip, active, if active { t.cyan } else { t.muted })
}

pub fn rail_button_tone(
    id: &'static str,
    t: &Theme,
    name: &'static str,
    tip: &'static str,
    active: bool,
    color: gpui::Rgba,
) -> gpui::Stateful<gpui::Div> {
    let t2 = *t;
    div()
        .id(id)
        .w(px(28.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor_pointer()
        .when(active, |d| d.bg(t2.cyan_16))
        .hover(move |s| s.bg(t2.ink_08))
        .active(move |s| s.bg(t2.ink_13))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(icon(name, px(17.), color))
}

#[allow(dead_code)]
fn _unused(_: &App, _: &FileDiff) {}
