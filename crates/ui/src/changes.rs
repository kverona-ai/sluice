//! Local Changes workspace (prototype screens 02/04, M2): change tree with
//! stage checkboxes, working-tree diff with hunk / line staging, commit panel
//! (message editor, AI draft, Amend / Sign-off / Author), push / pull.

use crate::i18n::tr;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use gpui_component::input::Input;
use sluice_core::diff::partial_patch;
use sluice_core::*;
use sluice_domain::CommitOptions;

use crate::diff_view::DiffView;
use crate::icons::{icon_b, icon_f};
use crate::theme::{FONT_MONO, Theme};
use crate::workbench::{WorkFile, Workbench, agent_badge, checkbox, section_label};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Conflicts,
    Staged,
    Unstaged,
    Untracked,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Conflicts => "Conflicts",
            Group::Staged => "Staged",
            Group::Unstaged => "Unstaged",
            Group::Untracked => "Untracked",
        }
    }
}

/// Flattened rows of the change tree (for keyboard navigation too).
fn work_rows(status: &WorkStatus) -> Vec<(Group, StatusEntry)> {
    let mut rows = Vec::new();
    for e in status.conflicted() {
        rows.push((Group::Conflicts, e.clone()));
    }
    for e in status.staged() {
        rows.push((Group::Staged, e.clone()));
    }
    for e in status.unstaged() {
        rows.push((Group::Unstaged, e.clone()));
    }
    for e in status.untracked() {
        rows.push((Group::Untracked, e.clone()));
    }
    rows
}

impl Workbench {
    fn cli(&self) -> Option<Arc<sluice_backend_cli::GitCli>> {
        self.repo.cli.clone()
    }

    /// Run a git write on the background executor, toast the outcome, reload.
    pub(crate) fn run_git<F>(&mut self, what: impl Into<String>, f: F, cx: &mut Context<Self>)
    where
        F: FnOnce(&sluice_backend_cli::GitCli) -> anyhow::Result<String> + Send + 'static,
    {
        let Some(cli) = self.cli() else {
            self.toast("裸仓库没有工作区，无法执行写操作", cx);
            return;
        };
        let what = what.into();
        self.commit_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx.background_spawn(async move { f(&cli) }).await;
            this.update(cx, |this, cx| {
                this.commit_busy = false;
                match res {
                    Ok(msg) if msg.is_empty() => this.toast(tf!("{}：完成", what), cx),
                    Ok(msg) => this.toast(tf!("{}：{}", what, msg), cx),
                    Err(e) => this.toast(tf!("{} 失败：{}", what, first_line(&format!("{e:#}"))), cx),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn stage_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let n = paths.len();
        self.run_git(
            tr("暂存"),
            move |cli| {
                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                cli.stage(&refs)?;
                Ok(tf!("{} 个文件", n))
            },
            cx,
        );
    }

    pub(crate) fn unstage_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let n = paths.len();
        self.run_git(
            tr("取消暂存"),
            move |cli| {
                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                cli.unstage(&refs)?;
                Ok(tf!("{} 个文件", n))
            },
            cx,
        );
    }

    pub(crate) fn stage_all(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .changes
            .as_ref()
            .map(|c| {
                c.status
                    .unstaged()
                    .chain(c.status.untracked())
                    .map(|e| e.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.stage_paths(paths, cx);
    }

    pub(crate) fn unstage_all(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .changes
            .as_ref()
            .map(|c| c.status.staged().map(|e| e.path.clone()).collect())
            .unwrap_or_default();
        self.unstage_paths(paths, cx);
    }

    fn toggle_entry(&mut self, group: Group, path: String, cx: &mut Context<Self>) {
        match group {
            Group::Staged => self.unstage_paths(vec![path], cx),
            Group::Unstaged | Group::Untracked | Group::Conflicts => self.stage_paths(vec![path], cx),
        }
    }

    pub(crate) fn toggle_selected_work_file(&mut self, cx: &mut Context<Self>) {
        if self.tab != crate::workbench::Tab::Changes {
            return;
        }
        let Some(wf) = self.work_file.clone() else { return };
        let group = if wf.staged { Group::Staged } else { Group::Unstaged };
        self.toggle_entry(group, wf.path, cx);
    }

    pub fn move_work_file(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(ch) = self.changes.clone() else { return };
        let rows = work_rows(&ch.status);
        if rows.is_empty() {
            return;
        }
        let cur = self.work_file.as_ref().and_then(|wf| {
            rows.iter()
                .position(|(g, e)| e.path == wf.path && (*g == Group::Staged) == wf.staged)
        });
        let next = match cur {
            Some(i) => (i as isize + delta).clamp(0, rows.len() as isize - 1) as usize,
            None => 0,
        };
        let (g, e) = &rows[next];
        self.open_work_file(
            WorkFile {
                path: e.path.clone(),
                old_path: e.old_path.clone(),
                staged: *g == Group::Staged,
            },
            cx,
        );
    }

    pub(crate) fn open_work_file(&mut self, wf: WorkFile, cx: &mut Context<Self>) {
        // Conflicted entries open in the three-way resolver instead of the diff.
        let is_conflict = self.changes.as_ref().is_some_and(|c| {
            c.status
                .entries
                .iter()
                .any(|e| e.path == wf.path && e.conflict.is_some())
        });
        if is_conflict {
            self.work_file = Some(wf.clone());
            self.open_conflict(wf.path, cx);
            return;
        }
        self.conflict = None;
        let reader = self.repo.reader.clone();
        let opts = self.diff_opts;
        let same = self.work_file.as_ref() == Some(&wf);
        if !same {
            self.deselected.clear();
        }
        self.work_file = Some(wf.clone());
        let change = FileChange {
            path: wf.path.clone(),
            old_path: wf.old_path.clone(),
            kind: ChangeKind::Modified,
            additions: None,
            deletions: None,
            binary: false,
        };
        let mut dv = DiffView::loading(wf.path.clone(), change);
        dv.stageable = !wf.staged;
        if let Some(prev) = &self.work_diff
            && prev.title == wf.path
        {
            dv.scroll = prev.scroll.clone();
        }
        self.work_diff = Some(dv);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let wf2 = wf.clone();
            let res = cx
                .background_spawn(async move {
                    sluice_domain::diff_work_file(
                        &*reader,
                        &wf2.path,
                        wf2.old_path.as_deref(),
                        wf2.staged,
                        &opts,
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                if this.work_file.as_ref() == Some(&wf)
                    && let Some(dv) = &mut this.work_diff
                {
                    dv.set_result(res.map_err(|e| format!("{e:#}")));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn toggle_hunk(&mut self, hunk: usize, cx: &mut Context<Self>) {
        let Some(dv) = &self.work_diff else { return };
        let Some(d) = &dv.diff else { return };
        let Some(h) = d.hunks.get(hunk) else { return };
        let changed: Vec<usize> = h
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.kind != sluice_core::diff::LineKind::Context)
            .map(|(i, _)| i)
            .collect();
        let any_on = changed
            .iter()
            .any(|li| !self.deselected.contains_key(&(hunk, *li)));
        for li in changed {
            if any_on {
                self.deselected.insert((hunk, li), ());
            } else {
                self.deselected.remove(&(hunk, li));
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_lines(&mut self, hunk: usize, lines: &[usize], cx: &mut Context<Self>) {
        let any_on = lines.iter().any(|li| !self.deselected.contains_key(&(hunk, *li)));
        for li in lines {
            if any_on {
                self.deselected.insert((hunk, *li), ());
            } else {
                self.deselected.remove(&(hunk, *li));
            }
        }
        cx.notify();
    }

    /// Stage the selected hunks / lines of the open unstaged file (05 §5: `git apply --cached --unidiff-zero`).
    fn stage_selected_lines(&mut self, cx: &mut Context<Self>) {
        let Some(wf) = self.work_file.clone() else { return };
        let Some(dv) = &self.work_diff else { return };
        let Some(d) = dv.diff.clone() else { return };
        let deselected = self.deselected.clone();
        let all_selected = deselected.is_empty();
        let path = wf.path.clone();
        if all_selected {
            self.stage_paths(vec![path], cx);
            return;
        }
        let Some(patch) = partial_patch(&d, &path, |h, l| !deselected.contains_key(&(h, l))) else {
            self.toast("没有选中任何行", cx);
            return;
        };
        let untracked = self
            .changes
            .as_ref()
            .is_some_and(|c| c.status.entries.iter().any(|e| e.path == path && e.untracked));
        self.deselected.clear();
        self.run_git(
            tr("暂存所选行"),
            move |cli| {
                if untracked {
                    cli.intent_to_add(&[&path])?;
                }
                cli.apply_cached(&patch, false)?;
                Ok(path)
            },
            cx,
        );
    }

    /// ⌘K (IDEA "Commit"): bring up the commit panel and focus the message editor.
    pub(crate) fn focus_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = crate::workbench::Tab::Changes;
        self.commit_msg.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn commit_from_editor(&mut self, cx: &mut Context<Self>) {
        self.do_commit(false, cx);
    }

    /// ⌘↩ (IDEA "Commit" inside the panel): commit when the panel is showing, otherwise open it.
    pub(crate) fn commit_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab != crate::workbench::Tab::Changes {
            self.focus_commit(window, cx);
            return;
        }
        self.do_commit(false, cx);
    }

    fn do_commit(&mut self, push: bool, cx: &mut Context<Self>) {
        let message = self.commit_msg.read(cx).value().to_string();
        let message_for_history = message.clone();
        if message.trim().is_empty() && !self.amend {
            self.toast("提交信息不能为空", cx);
            return;
        }
        let staged = self
            .changes
            .as_ref()
            .map(|c| c.status.staged().count())
            .unwrap_or(0);
        if staged == 0 && !self.amend {
            self.toast("没有已暂存的变更", cx);
            return;
        }
        let Some(cli) = self.cli() else { return };
        let author = self.author_input.read(cx).value().trim().to_string();
        let opts = CommitOptions {
            amend: self.amend,
            signoff: self.signoff,
            no_verify: self.no_verify,
            author: (!author.is_empty()).then_some(author),
            sign: None,
        };
        let amend = self.amend;
        self.commit_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let sha = cli.commit(&message, &opts)?;
                    if push {
                        cli.push(None, None, false, false)?;
                    }
                    anyhow::Ok(sha)
                })
                .await;
            this.update(cx, |this, cx| {
                this.commit_busy = false;
                match res {
                    Ok(sha) => {
                        crate::recent::remember_message(&message_for_history);
                        this.clear_msg_pending = true;
                        this.amend = false;
                        this.toast(
                            tf!(
                                "{}：{}{}",
                                if push {
                                    tr("已提交并推送")
                                } else {
                                    tr("已提交")
                                },
                                &sha[..sha.len().min(8)],
                                if amend { tr("（amend）") } else { "" }
                            ),
                            cx,
                        );
                    }
                    Err(e) => this.toast(tf!("提交失败：{}", first_line(&format!("{e:#}"))), cx),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn git_pull(&mut self, cx: &mut Context<Self>) {
        self.run_git(
            tr("拉取"),
            |cli| {
                let out = cli.pull(None)?;
                let l = first_line(&out.stdout_str());
                Ok(if l.is_empty() { tr("完成").into() } else { l })
            },
            cx,
        );
    }

    fn ai_draft(&mut self, cx: &mut Context<Self>) {
        let Some((tool_id, tool_name)) = self.ai_tool.clone() else {
            self.toast("未检测到已安装的 AI CLI（claude / codex / grok / dsh）", cx);
            return;
        };
        let Some(cli) = self.cli() else { return };
        if self.ai_busy {
            return;
        }
        self.ai_busy = true;
        cx.notify();
        let subjects: Vec<String> = self
            .log
            .as_ref()
            .map(|l| l.commits.iter().take(30).map(|c| c.summary.clone()).collect())
            .unwrap_or_default();
        let console = self.repo.console.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let t0 = std::time::Instant::now();
                    let diff = cli.run_read(&["diff", "--cached", "--no-color", "--unified=3"])?;
                    let r = crate::ai::draft_commit_message(&tool_id, &diff.stdout_str(), &subjects);
                    console.log(ConsoleEntry {
                        at: chrono::Local::now(),
                        kind: ConsoleKind::Ai,
                        command: format!("{tool_name} headless · draft commit message"),
                        duration_ms: t0.elapsed().as_millis(),
                        exit_code: Some(if r.is_ok() { 0 } else { 1 }),
                        summary: match &r {
                            Ok(m) => m.lines().next().unwrap_or("").to_string(),
                            Err(e) => format!("{e:#}"),
                        },
                        stderr: String::new(),
                    });
                    r
                })
                .await;
            this.update(cx, |this, cx| {
                this.ai_busy = false;
                match res {
                    Ok(msg) => {
                        this.pending_ai_message = Some(msg);
                        this.toast("AI 草稿已生成，已填入消息框（可编辑，不会自动提交）", cx);
                    }
                    Err(e) => this.toast(tf!("AI 生成失败：{}", first_line(&format!("{e:#}"))), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- rendering ------------------------------------------------------------

    pub(crate) fn render_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // apply a pending AI draft into the editor (needs the window)
        if let Some(msg) = self.pending_ai_message.take() {
            self.commit_msg.update(cx, |s, cx| s.set_value(msg, window, cx));
        }
        if self.clear_msg_pending {
            self.clear_msg_pending = false;
            self.commit_msg.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let t = self.theme;
        let right: gpui::AnyElement = if self.conflict.is_some() {
            self.render_conflict_view(cx).into_any_element()
        } else if self.file_view.is_some() {
            self.render_file_view(cx).into_any_element()
        } else if self.work_diff.is_some() {
            self.render_work_diff_pane(cx).into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.faint)
                .text_size(px(12.5))
                .child(tr("选择左侧文件查看 diff；勾选 = 纳入暂存区"))
                .into_any_element()
        };
        let banner = self
            .changes
            .as_ref()
            .and_then(|c| c.status.in_progress)
            .map(|op| self.op_banner(op, cx));
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .children(banner)
            .child(
                gpui_component::resizable::h_resizable("changes-split")
                    .with_state(&self.changes_split.clone())
                    .on_resize(cx.listener(
                        |this, st: &gpui::Entity<gpui_component::resizable::ResizableState>, _, cx| {
                            if let Some(w) = st.read(cx).sizes().first() {
                                this.settings.changes_tree_w = f32::from(*w);
                                this.save_settings();
                            }
                        },
                    ))
                    .child(
                        gpui_component::resizable::resizable_panel()
                            .size(px(self.settings.changes_tree_w))
                            .size_range(px(260.)..px(720.))
                            .child(self.render_change_tree(window, cx)),
                    )
                    .child(gpui_component::resizable::resizable_panel().child(right)),
            )
    }

    fn render_work_diff_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let stageable = self.work_diff.as_ref().is_some_and(|d| d.stageable);
        let (hunks_on, lines_on, lines_total) = self
            .work_diff
            .as_ref()
            .and_then(|dv| dv.diff.as_ref())
            .map(|d| {
                let mut lines_total = 0;
                let mut lines_on = 0;
                let mut hunks_on = 0;
                for (hi, h) in d.hunks.iter().enumerate() {
                    let mut any = false;
                    for (li, l) in h.lines.iter().enumerate() {
                        if l.kind == sluice_core::diff::LineKind::Context {
                            continue;
                        }
                        lines_total += 1;
                        if !self.deselected.contains_key(&(hi, li)) {
                            lines_on += 1;
                            any = true;
                        }
                    }
                    if any {
                        hunks_on += 1;
                    }
                }
                (hunks_on, lines_on, lines_total)
            })
            .unwrap_or((0, 0, 0));
        let diff = self.render_diff_view(true, cx);
        let mut pane = div().flex_1().min_w_0().min_h_0().flex().flex_col().child(diff);
        if stageable {
            pane = pane.child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(t.line)
                    .px(px(12.))
                    .py(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().text_size(px(12.5)).child(tf!(
                        "已选 {} 块 / {} 行（共 {} 行变更）纳入暂存",
                        hunks_on,
                        lines_on,
                        lines_total
                    )))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.faint)
                            .child(tr("底层：git apply --cached --unidiff-zero（Console 可见）")),
                    )
                    .child(
                        div()
                            .id("stage-selected")
                            .ml_auto()
                            .px(px(18.))
                            .py(px(5.))
                            .bg(t.cyan)
                            .text_color(t.surface)
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.cyan_deep))
                            .on_click(cx.listener(|this, _, _, cx| this.stage_selected_lines(cx)))
                            .child(if lines_on == lines_total {
                                tr("暂存整个文件")
                            } else {
                                tr("暂存所选")
                            }),
                    ),
            );
        } else {
            let path = self
                .work_file
                .as_ref()
                .map(|w| w.path.clone())
                .unwrap_or_default();
            pane = pane.child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(t.line)
                    .px(px(12.))
                    .py(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(t.muted)
                            .child(tr("已暂存 —— 将随下一次提交落地")),
                    )
                    .child(
                        div()
                            .id("unstage-file")
                            .ml_auto()
                            .px(px(14.))
                            .py(px(5.))
                            .border_1()
                            .border_color(t.line)
                            .text_size(px(12.5))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.ink_05))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.unstage_paths(vec![path.clone()], cx)),
                            )
                            .child(tr("取消暂存")),
                    ),
            );
        }
        pane
    }

    fn render_change_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let changes = self.changes.clone();
        let open = self.work_file.clone();
        let staged_n = changes.as_ref().map(|c| c.status.staged().count()).unwrap_or(0);
        let total_n = changes.as_ref().map(|c| c.status.entries.len()).unwrap_or(0);

        let mut tree = div()
            .id("change-tree")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(6.));
        match &changes {
            None => {
                tree = tree.child(
                    div()
                        .p(px(12.))
                        .text_color(t.muted)
                        .child(if self.changes_loading {
                            tr("读取状态…")
                        } else {
                            tr("无法读取工作区状态")
                        }),
                );
            }
            Some(ch) => {
                if let Some(err) = &self.changes_error {
                    tree = tree.child(div().p(px(12.)).text_color(t.mag_deep).child(err.clone()));
                }
                if ch.status.entries.is_empty() {
                    tree = tree.child(
                        div()
                            .p(px(12.))
                            .text_color(t.muted)
                            .child(tr("工作区干净 —— 没有变更")),
                    );
                }
                let rows = work_rows(&ch.status);
                let mut last_group: Option<Group> = None;
                for (i, (group, e)) in rows.iter().enumerate() {
                    if last_group != Some(*group) {
                        last_group = Some(*group);
                        let count = rows.iter().filter(|(g, _)| g == group).count();
                        let (all_on, mixed) = match group {
                            Group::Staged => (true, false),
                            _ => (false, false),
                        };
                        let g = *group;
                        let paths: Vec<String> = rows
                            .iter()
                            .filter(|(x, _)| x == group)
                            .map(|(_, e)| e.path.clone())
                            .collect();
                        tree = tree.child(
                            div()
                                .id(("group", i))
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .px(px(10.))
                                .py(px(4.))
                                .mt(px(4.))
                                .text_size(px(12.5))
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.ink_05))
                                .on_click(cx.listener(move |this, _, _, cx| match g {
                                    Group::Staged => this.unstage_paths(paths.clone(), cx),
                                    _ => this.stage_paths(paths.clone(), cx),
                                }))
                                .child(checkbox(&t, all_on, mixed))
                                .child(icon_f(
                                    "folder",
                                    px(14.),
                                    if g == Group::Conflicts { t.mag } else { t.cyan },
                                ))
                                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(group.label()))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(t.faint)
                                        .child(format!("{count}")),
                                ),
                        );
                    }
                    let (dir, name) = match e.path.rfind('/') {
                        Some(ix) => (&e.path[..=ix], &e.path[ix + 1..]),
                        None => ("", e.path.as_str()),
                    };
                    let kind = match group {
                        Group::Staged => e.staged,
                        Group::Unstaged => e.unstaged,
                        Group::Untracked => Some(ChangeKind::Added),
                        Group::Conflicts => None,
                    };
                    let mark = match (group, kind) {
                        (Group::Conflicts, _) => "!".to_string(),
                        (Group::Untracked, _) => "?".to_string(),
                        (_, Some(k)) => k.mark().to_string(),
                        _ => String::new(),
                    };
                    let mark_color = match (group, kind) {
                        (Group::Conflicts, _) => t.mag,
                        (_, Some(ChangeKind::Added)) | (Group::Untracked, _) => t.cyan,
                        (_, Some(ChangeKind::Deleted)) => t.mag,
                        _ => t.muted,
                    };
                    let is_staged_row = *group == Group::Staged;
                    let is_open = open
                        .as_ref()
                        .is_some_and(|w| w.path == e.path && w.staged == is_staged_row);
                    let g = *group;
                    let path = e.path.clone();
                    let path2 = e.path.clone();
                    let old_path = e.old_path.clone();
                    let ctx_path = e.path.clone();
                    let ctx_old = e.old_path.clone();
                    let ctx_untracked = e.untracked;
                    tree = tree.child(
                        div()
                            .id(("wf", i))
                            .on_mouse_down(
                                gpui::MouseButton::Right,
                                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                    this.show_ctx_menu(
                                        ev,
                                        crate::overlays::CtxTarget::WorkFile {
                                            path: ctx_path.clone(),
                                            old_path: ctx_old.clone(),
                                            staged: is_staged_row,
                                            untracked: ctx_untracked,
                                        },
                                        cx,
                                    )
                                }),
                            )
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .pl(px(22.))
                            .pr(px(10.))
                            .py(px(3.))
                            .text_size(px(12.5))
                            .cursor_pointer()
                            .when(is_open, |d| d.bg(t.sel))
                            .when(!is_open, |d| d.hover(move |s| s.bg(t.ink_05)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_work_file(
                                    WorkFile {
                                        path: path2.clone(),
                                        old_path: old_path.clone(),
                                        staged: is_staged_row,
                                    },
                                    cx,
                                )
                            }))
                            .child(
                                div()
                                    .id(("wfchk", i))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_entry(g, path.clone(), cx);
                                    }))
                                    .child(checkbox(&t, is_staged_row, false)),
                            )
                            .child(
                                div()
                                    .w(px(13.))
                                    .flex_none()
                                    .text_center()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(mark_color)
                                    .child(mark),
                            )
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(11.5))
                                    .flex_none()
                                    .child(name.to_string()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(t.faint)
                                    .child(dir.to_string()),
                            )
                            .when_some(e.old_path.clone(), |d, op| {
                                d.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(t.faint)
                                        .child(format!("← {op}")),
                                )
                            })
                            .child(div().ml_auto().child(agent_badge(&t, Agent::Human))),
                    );
                }
            }
        }

        let toolbar = div()
            .px(px(10.))
            .py(px(7.))
            .flex()
            .items_center()
            .gap(px(12.))
            .border_b_1()
            .border_color(t.line_soft)
            .text_size(px(12.))
            .text_color(t.muted)
            .flex_none()
            .child(
                crate::workbench::chrome_button(
                    "stage-all",
                    &t,
                    "arrow-line-down",
                    tr("全部暂存 ⌥⌘A"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.stage_all(cx))),
            )
            .child(
                crate::workbench::chrome_button(
                    "unstage-all",
                    &t,
                    "arrow-line-up",
                    tr("全部取消暂存 ⌥⌘U"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.unstage_all(cx))),
            )
            .child(
                crate::workbench::chrome_button(
                    "refresh-changes",
                    &t,
                    "arrow-clockwise",
                    tr("刷新状态"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.reload_changes(cx))),
            )
            .child(div().ml_auto().child(tf!("{} / {} 已暂存", staged_n, total_n)));

        div()
            .size_full()
            .border_r_1()
            .border_color(t.line)
            .flex()
            .flex_col()
            .min_h_0()
            .child(toolbar)
            .child(tree)
            .child(self.render_commit_panel(window, cx))
    }

    fn render_commit_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let msg_focused = gpui::Focusable::focus_handle(self.commit_msg.read(cx), cx).is_focused(window);
        let author_focused = gpui::Focusable::focus_handle(self.author_input.read(cx), cx).is_focused(window);
        let ai_label = match (&self.ai_tool, self.ai_busy) {
            (None, _) => tr("AI 生成提交信息（未检测到 CLI）").to_string(),
            (Some(_), true) => tr("生成中…").to_string(),
            (Some((_, name)), false) => tf!("AI 生成提交信息 · {}", name),
        };
        let msg_len = self
            .commit_msg
            .read(cx)
            .value()
            .lines()
            .next()
            .map(|l| l.chars().count())
            .unwrap_or(0);
        let busy = self.commit_busy;
        let opt = |id: &'static str, on: bool, label: &'static str, warn: Option<String>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .cursor_pointer()
                .rounded(px(4.))
                .px(px(3.))
                .hover(move |s| s.bg(t.ink_05))
                .child(checkbox(&t, on, false))
                .child(label)
                .when_some(warn, |d, w| {
                    d.child(div().text_size(px(11.)).text_color(t.mag_deep).child(w))
                })
        };
        let amend_warn = if self.amend {
            Some(tr("将改写最近一次提交").to_string())
        } else {
            None
        };
        div()
            .flex_none()
            .border_t_1()
            .border_color(t.line)
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(section_label(&t, "Commit message"))
                    .child(
                        crate::workbench::chrome_button(
                            "msg-history",
                            &t,
                            "clock-counter-clockwise",
                            tr("最近提交信息（最多 50 条）"),
                            false,
                        )
                        .w(px(22.))
                        .h(px(22.))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, ev: &gpui::MouseDownEvent, _, cx| {
                                this.popup = match this.popup {
                                    Some((crate::workbench::Popup::Messages, _)) => None,
                                    _ => Some((crate::workbench::Popup::Messages, ev.position)),
                                };
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        div()
                            .id("ai-gen")
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .px(px(10.))
                            .py(px(3.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .border_1()
                            .border_color(t.mag)
                            .text_color(t.mag_deep)
                            .when(self.ai_busy, |d| d.bg(t.mag_soft))
                            .hover(move |s| s.bg(t.mag_soft))
                            .on_click(cx.listener(|this, _, _, cx| this.ai_draft(cx)))
                            .child(icon_b("sparkle", px(12.), t.mag_deep))
                            .child(ai_label),
                    ),
            )
            .child(
                div()
                    .border_1()
                    .border_color(if msg_focused { t.cyan } else { t.line })
                    .bg(t.surface)
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(12.5))
                    .child(Input::new(&self.commit_msg).appearance(false)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .child(div().text_color(t.muted).w(px(46.)).child("Author"))
                    .child(
                        div()
                            .flex_1()
                            .border_1()
                            .border_color(if author_focused { t.cyan } else { t.line })
                            .bg(t.surface)
                            .px(px(8.))
                            .py(px(1.))
                            .child(Input::new(&self.author_input).appearance(false)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(
                        opt("opt-amend", self.amend, "Amend", amend_warn).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.amend = !this.amend;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        opt("opt-signoff", self.signoff, "Sign-off", None).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.signoff = !this.signoff;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        opt(
                            "opt-noverify",
                            self.no_verify,
                            tr("跳过 hooks"),
                            if self.no_verify {
                                Some(tr("慎用").into())
                            } else {
                                None
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.no_verify = !this.no_verify;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(11.5))
                    .text_color(t.faint)
                    .child(format!("subject {msg_len} / 72"))
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .id("commit-push")
                                    .px(px(12.))
                                    .py(px(4.))
                                    .border_1()
                                    .border_color(t.line)
                                    .text_size(px(12.5))
                                    .text_color(t.ink)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.ink_05))
                                    .when(busy, |d| d.opacity(0.5))
                                    .on_click(cx.listener(|this, _, _, cx| this.do_commit(true, cx)))
                                    .child("Commit & Push"),
                            )
                            .child(
                                div()
                                    .id("commit")
                                    .px(px(16.))
                                    .py(px(4.))
                                    .bg(t.cyan)
                                    .text_color(t.surface)
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.cyan_deep))
                                    .when(busy, |d| d.opacity(0.5))
                                    .on_click(cx.listener(|this, _, _, cx| this.do_commit(false, cx)))
                                    .child(if busy { tr("执行中…") } else { "Commit" }),
                            ),
                    ),
            )
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

#[allow(dead_code)]
fn _t(_: &Theme) {}
