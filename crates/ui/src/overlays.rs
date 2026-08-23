//! Modal overlays (branches, stash, snapshots, settings, push, new-branch,
//! confirmations) and the right-click context menus. All destructive actions
//! go through [`ConfirmAction`] and take a safety snapshot first where the
//! content is recoverable (05 §6).

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Point, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::input::Input;
use sluice_backend_cli::{SnapshotEntry, StashEntry};
use sluice_core::*;

use crate::icons::{icon, icon_b, icon_f};
use crate::theme::{FONT_HEADING, FONT_MONO, Theme};
use crate::workbench::{Tab, Workbench, checkbox, section_label};

#[derive(Clone, Debug, PartialEq)]
pub enum Overlay {
    Branches,
    Stash,
    Snapshots,
    Settings,
    Push,
    NewBranch { from: Option<Oid> },
    Confirm(ConfirmAction),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmAction {
    DeleteBranch {
        name: String,
    },
    ResetHard {
        sha: Oid,
    },
    Discard {
        path: String,
        staged: bool,
        untracked: bool,
    },
    DropStash {
        id: String,
    },
    DeleteSnapshot {
        refname: String,
    },
}

impl ConfirmAction {
    fn title(&self) -> String {
        match self {
            ConfirmAction::DeleteBranch { name } => format!("删除分支 {name}？"),
            ConfirmAction::ResetHard { sha } => format!("Reset --hard 到 {}？", sha.short(8)),
            ConfirmAction::Discard { path, .. } => format!("丢弃 {path} 的变更？"),
            ConfirmAction::DropStash { id } => format!("删除 {id}？"),
            ConfirmAction::DeleteSnapshot { .. } => "删除该快照？".to_string(),
        }
    }
    fn body(&self) -> String {
        match self {
            ConfirmAction::DeleteBranch { .. } => {
                "等价 git branch -D（未合并提交也会被删除）。分支指向的提交在 reflog 保留期内仍可找回。"
                    .into()
            }
            ConfirmAction::ResetHard { .. } => {
                "工作区与暂存区将被重置。执行前会自动创建时光机快照（可从「时光机」恢复）。".into()
            }
            ConfirmAction::Discard { untracked, .. } => {
                if *untracked {
                    "该文件未被跟踪，无法进入快照 —— 删除后不可恢复。".into()
                } else {
                    "执行前会自动创建时光机快照（可从「时光机」恢复）。".into()
                }
            }
            ConfirmAction::DropStash { .. } => "stash 提交对象在 git 保留期内仍可通过其 SHA 找回。".into(),
            ConfirmAction::DeleteSnapshot { .. } => "仅删除快照引用；对象在 git gc 之前仍存在。".into(),
        }
    }
}

/// Right-click context menu.
#[derive(Clone, Debug)]
pub struct CtxMenu {
    pub at: Point<Pixels>,
    pub target: CtxTarget,
}

#[derive(Clone, Debug)]
pub enum CtxTarget {
    /// Index into `log.commits`.
    Commit(usize),
    WorkFile {
        path: String,
        old_path: Option<String>,
        staged: bool,
        untracked: bool,
    },
}

const KEYMAP_ROWS: &[(&str, &str, &str)] = &[
    ("日志 / 提交图", "⌘9", "Alt+9"),
    ("本地变更 / 提交面板", "⌘0", "Alt+0"),
    ("Console", "⌘6", "Alt+6"),
    ("分支面板", "⌃⇧`", "Ctrl+Shift+`"),
    ("Stash 面板", "⌘5", "Ctrl+5"),
    ("时光机（快照）", "⌘7", "Ctrl+7"),
    ("Push 对话框", "⌘⇧K", "Ctrl+Shift+K"),
    ("搜索提交", "⌘F", "Ctrl+F"),
    ("提交面板（聚焦消息）", "⌘K", "Ctrl+K"),
    ("提交", "⌘↩", "Ctrl+Enter"),
    ("全部暂存 / 取消暂存", "⌥⌘A / ⌥⌘U", "Ctrl+Alt+A / U"),
    ("切换选中项暂存", "Space", "Space"),
    ("下 / 上一处差异", "F7 / ⇧F7", "F7 / Shift+F7"),
    ("双栏 / 统一切换", "⌥⌘\\", "Ctrl+Alt+\\"),
    ("刷新", "⌥⌘Y", "Ctrl+Alt+Y"),
    ("关闭弹层 / diff", "Esc", "Esc"),
];

impl Workbench {
    // ----- openers ---------------------------------------------------------

    pub(crate) fn open_branches(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Branches);
        cx.notify();
    }

    pub(crate) fn open_push(&mut self, cx: &mut Context<Self>) {
        self.push_upstream = self.repo.info.head.upstream.is_none();
        self.push_lease = false;
        self.overlay = Some(Overlay::Push);
        cx.notify();
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Settings);
        cx.notify();
    }

    pub(crate) fn open_stashes(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else {
            self.toast("裸仓库没有工作区", cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            let res = cx.background_spawn(async move { cli.stash_list() }).await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        this.stashes = v;
                        this.overlay = Some(Overlay::Stash);
                    }
                    Err(e) => this.toast(format!("读取 stash 失败：{e:#}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_snapshots(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else {
            self.toast("裸仓库没有工作区", cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            let res = cx.background_spawn(async move { cli.snapshot_list() }).await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        this.snapshots = v;
                        this.overlay = Some(Overlay::Snapshots);
                    }
                    Err(e) => this.toast(format!("读取快照失败：{e:#}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Close the topmost transient surface. Returns true when something closed.
    pub(crate) fn dismiss_top(&mut self, cx: &mut Context<Self>) -> bool {
        if self.ctx_menu.is_some() {
            self.ctx_menu = None;
            cx.notify();
            return true;
        }
        if self.overlay.is_some() {
            self.overlay = None;
            cx.notify();
            return true;
        }
        false
    }

    // ----- confirm dispatch -------------------------------------------------

    pub(crate) fn run_confirmed(&mut self, action: ConfirmAction, cx: &mut Context<Self>) {
        self.overlay = None;
        match action {
            ConfirmAction::DeleteBranch { name } => {
                self.run_git(
                    format!("删除分支 {name}"),
                    move |cli| {
                        cli.branch_delete(&name, true)?;
                        Ok(String::new())
                    },
                    cx,
                );
            }
            ConfirmAction::ResetHard { sha } => {
                let short = sha.short(8).to_string();
                self.run_git(
                    format!("reset --hard {short}"),
                    move |cli| {
                        let snap = cli.snapshot_create(&format!("before reset --hard {short}"))?;
                        cli.reset("hard", sha.as_str())?;
                        Ok(match snap {
                            Some(_) => "已创建快照".into(),
                            None => "工作区原本干净".into(),
                        })
                    },
                    cx,
                );
            }
            ConfirmAction::Discard {
                path,
                staged,
                untracked,
            } => {
                self.run_git(
                    format!("丢弃 {path}"),
                    move |cli| {
                        if untracked {
                            cli.run(&["clean", "-f", "--", &path])?;
                            return Ok("未跟踪文件已删除".into());
                        }
                        let snap = cli.snapshot_create(&format!("before discard {path}"))?;
                        if staged {
                            cli.unstage(&[&path])?;
                        }
                        cli.discard(&[&path])?;
                        Ok(match snap {
                            Some(_) => "已创建快照".into(),
                            None => String::new(),
                        })
                    },
                    cx,
                );
            }
            ConfirmAction::DropStash { id } => {
                self.run_git(
                    format!("删除 {id}"),
                    move |cli| {
                        cli.stash_drop(&id)?;
                        Ok(String::new())
                    },
                    cx,
                );
                self.overlay = Some(Overlay::Stash);
                self.refresh_stash_list(cx);
            }
            ConfirmAction::DeleteSnapshot { refname } => {
                self.run_git(
                    "删除快照".to_string(),
                    move |cli| {
                        cli.snapshot_delete(&refname)?;
                        Ok(String::new())
                    },
                    cx,
                );
                self.overlay = Some(Overlay::Snapshots);
                self.refresh_snapshot_list(cx);
            }
        }
    }

    pub(crate) fn refresh_stash_list(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else { return };
        cx.spawn(async move |this, cx| {
            if let Ok(v) = cx.background_spawn(async move { cli.stash_list() }).await {
                this.update(cx, |this, cx| {
                    this.stashes = v;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn refresh_snapshot_list(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else { return };
        cx.spawn(async move |this, cx| {
            if let Ok(v) = cx.background_spawn(async move { cli.snapshot_list() }).await {
                this.update(cx, |this, cx| {
                    this.snapshots = v;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    // ----- overlay rendering ------------------------------------------------

    pub(crate) fn render_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let t = self.theme;
        let overlay = self.overlay.clone()?;
        let content: gpui::AnyElement = match &overlay {
            Overlay::Branches => self.render_branches(window, cx).into_any_element(),
            Overlay::Stash => self.render_stash(cx).into_any_element(),
            Overlay::Snapshots => self.render_snapshots(cx).into_any_element(),
            Overlay::Settings => self.render_settings(cx).into_any_element(),
            Overlay::Push => self.render_push(cx).into_any_element(),
            Overlay::NewBranch { from } => self.render_new_branch(from.clone(), cx).into_any_element(),
            Overlay::Confirm(action) => self.render_confirm(action.clone(), cx).into_any_element(),
        };
        Some(
            div()
                .id("overlay-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x201e1d55))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.overlay = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("overlay-panel")
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .bg(t.paper)
                        .border_1()
                        .border_color(t.line)
                        .rounded(px(10.))
                        .shadow_lg()
                        .child(content),
                ),
        )
    }

    fn panel_header(
        &self,
        t: &Theme,
        title: &'static str,
        subtitle: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .py(px(12.))
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                div()
                    .font_family(FONT_HEADING)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_size(px(16.))
                    .child(title),
            )
            .child(div().text_size(px(12.)).text_color(t.faint).child(subtitle))
            .child(
                div()
                    .id("overlay-close")
                    .ml_auto()
                    .w(px(24.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover({
                        let t2 = *t;
                        move |s| s.bg(t2.ink_08)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.overlay = None;
                        cx.notify();
                    }))
                    .child(icon_b("x", px(13.), t.muted)),
            )
    }

    fn primary_btn(&self, t: &Theme, id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
        let t2 = *t;
        div()
            .id(id)
            .px(px(16.))
            .py(px(5.))
            .bg(t.cyan)
            .text_color(t.surface)
            .text_size(px(12.5))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |s| s.bg(t2.cyan_deep))
            .child(label)
    }

    fn ghost_btn(&self, t: &Theme, id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
        let t2 = *t;
        div()
            .id(id)
            .px(px(14.))
            .py(px(5.))
            .border_1()
            .border_color(t.line)
            .text_size(px(12.5))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |s| s.bg(t2.ink_05))
            .child(label)
    }

    // ----- branches ---------------------------------------------------------

    fn render_branches(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let filter = self.branch_filter.read(cx).value().to_string().to_lowercase();
        let focused = gpui::Focusable::focus_handle(self.branch_filter.read(cx), cx).is_focused(window);
        let current = self.repo.info.head.branch.clone();
        let refs = self.log.as_ref().map(|l| l.refs.clone()).unwrap_or_default();
        let locals: Vec<&Ref> = refs
            .iter()
            .filter(|r| {
                r.kind == RefKind::LocalBranch
                    && (filter.is_empty() || r.short_name.to_lowercase().contains(&filter))
            })
            .collect();
        let remotes: Vec<&Ref> = refs
            .iter()
            .filter(|r| {
                matches!(r.kind, RefKind::RemoteBranch { .. })
                    && (filter.is_empty() || r.short_name.to_lowercase().contains(&filter))
            })
            .collect();

        let mut list = div()
            .id("branches-list")
            .max_h(px(380.))
            .overflow_y_scroll()
            .py(px(4.));
        let mut ix = 0usize;
        for (label, group) in [("Local", &locals), ("Remote", &remotes)] {
            if group.is_empty() {
                continue;
            }
            list = list.child(
                div()
                    .px(px(16.))
                    .pt(px(8.))
                    .pb(px(2.))
                    .child(section_label(&t, label)),
            );
            for r in group {
                ix += 1;
                let name = r.short_name.clone();
                let is_current =
                    current.as_deref() == Some(r.short_name.as_str()) && r.kind == RefKind::LocalBranch;
                let is_local = r.kind == RefKind::LocalBranch;
                let checkout_name = name.clone();
                let merge_name = name.clone();
                let rebase_name = name.clone();
                let del_name = name.clone();
                list = list.child(
                    div()
                        .id(("branch-row", ix))
                        .group("brow")
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .mx(px(8.))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .text_size(px(13.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(t.ink_05))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let target = checkout_name.clone();
                            this.overlay = None;
                            this.run_git(
                                format!("checkout {target}"),
                                move |cli| {
                                    cli.checkout(&target)?;
                                    Ok(String::new())
                                },
                                cx,
                            );
                        }))
                        .child(if is_current {
                            icon_f("tag", px(14.), t.yellow).into_any_element()
                        } else {
                            icon_b("git-branch", px(14.), t.muted).into_any_element()
                        })
                        .child(
                            div()
                                .when(is_current, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                                .child(name.clone()),
                        )
                        .when(is_current, |d| {
                            d.child(div().text_size(px(11.)).text_color(t.faint).child("当前"))
                        })
                        .child(div().ml_auto())
                        .when(!is_current, |d| {
                            d.child(
                                div()
                                    .id(("branch-merge", ix))
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .text_size(px(11.5))
                                    .text_color(t.cyan_deep)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.cyan_16))
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new("merge 到当前分支")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let b = merge_name.clone();
                                        this.overlay = None;
                                        this.run_git(
                                            format!("merge {b}"),
                                            move |cli| {
                                                cli.merge(&b, false)?;
                                                Ok(String::new())
                                            },
                                            cx,
                                        );
                                    }))
                                    .child("merge"),
                            )
                            .child(
                                div()
                                    .id(("branch-rebase", ix))
                                    .px(px(6.))
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .text_size(px(11.5))
                                    .text_color(t.cyan_deep)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.cyan_16))
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new("把当前分支 rebase 到它上面")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let b = rebase_name.clone();
                                        this.overlay = None;
                                        this.run_git(
                                            format!("rebase 到 {b}"),
                                            move |cli| {
                                                cli.rebase_onto(&b)?;
                                                Ok(String::new())
                                            },
                                            cx,
                                        );
                                    }))
                                    .child("rebase"),
                            )
                        })
                        .when(is_local && !is_current, |d| {
                            d.child(
                                div()
                                    .id(("branch-del", ix))
                                    .w(px(20.))
                                    .h(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.mag_soft))
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new("删除分支（需确认）")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.overlay = Some(Overlay::Confirm(ConfirmAction::DeleteBranch {
                                            name: del_name.clone(),
                                        }));
                                        cx.notify();
                                    }))
                                    .child(icon_b("x", px(11.), t.mag_deep)),
                            )
                        }),
                );
            }
        }

        div()
            .w(px(460.))
            .flex()
            .flex_col()
            .child(self.panel_header(
                &t,
                "分支",
                format!("{} 本地 · {} 远端", locals.len(), remotes.len()),
                cx,
            ))
            .child(
                div().px(px(16.)).py(px(8.)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(9.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .bg(t.surface)
                        .border_1()
                        .border_color(if focused { t.cyan } else { t.line })
                        .child(icon_b("magnifying-glass", px(13.), t.faint))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.5))
                                .child(Input::new(&self.branch_filter).appearance(false)),
                        ),
                ),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line_soft)
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.faint)
                            .child("点击分支 = checkout；脏工作区会提示"),
                    )
                    .child(div().ml_auto())
                    .child(
                        self.primary_btn(&t, "branch-new", "新建分支…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.overlay = Some(Overlay::NewBranch { from: None });
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_new_branch(&mut self, from: Option<Oid>, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let from_label = from
            .as_ref()
            .map(|o| o.short(8).to_string())
            .unwrap_or_else(|| "HEAD".into());
        let from2 = from.clone();
        div()
            .w(px(400.))
            .flex()
            .flex_col()
            .child(self.panel_header(&t, "新建分支", format!("基于 {from_label}"), cx))
            .child(
                div().px(px(16.)).py(px(10.)).flex().flex_col().gap(px(8.)).child(
                    div()
                        .border_1()
                        .border_color(t.line)
                        .bg(t.surface)
                        .rounded(px(6.))
                        .px(px(9.))
                        .py(px(3.))
                        .text_size(px(12.5))
                        .child(Input::new(&self.new_branch_name).appearance(false)),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line_soft)
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.faint)
                            .child("创建后自动 checkout"),
                    )
                    .child(div().ml_auto())
                    .child(
                        self.ghost_btn(&t, "nb-cancel", "取消")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.overlay = None;
                                cx.notify();
                            })),
                    )
                    .child(self.primary_btn(&t, "nb-create", "创建").on_click(cx.listener(
                        move |this, _, _, cx| {
                            let name = this.new_branch_name.read(cx).value().trim().to_string();
                            if name.is_empty() {
                                this.toast("分支名不能为空", cx);
                                return;
                            }
                            let from = from2.clone();
                            this.overlay = None;
                            this.run_git(
                                format!("新建分支 {name}"),
                                move |cli| {
                                    cli.branch_create(&name, from.as_ref().map(|o| o.as_str()), true)?;
                                    Ok(String::new())
                                },
                                cx,
                            );
                        },
                    ))),
            )
    }

    // ----- push -------------------------------------------------------------

    fn render_push(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let head = &self.repo.info.head;
        let branch = head.branch.clone().unwrap_or_else(|| "HEAD".into());
        let upstream = head.upstream.clone();
        let ahead = head.ahead;
        let lease = self.push_lease;
        let set_up = self.push_upstream;
        let opt_row = |id: &'static str, on: bool, label: &'static str, note: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(4.))
                .py(px(3.))
                .rounded(px(4.))
                .text_size(px(12.5))
                .cursor_pointer()
                .hover(move |s| s.bg(t.ink_05))
                .child(checkbox(&t, on, false))
                .child(label)
                .child(div().text_size(px(11.)).text_color(t.faint).child(note))
        };
        div()
            .w(px(430.))
            .flex()
            .flex_col()
            .child(self.panel_header(
                &t,
                "推送",
                format!(
                        "{branch} → {}",
                        upstream
                            .clone()
                            .unwrap_or_else(|| "origin（新建 upstream）".into())
                    ),
                cx,
            ))
            .child(
                div()
                    .px(px(16.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_size(px(13.)).child(format!("{ahead} 个未推送提交")))
                    .child(
                        opt_row(
                            "push-lease",
                            lease,
                            "--force-with-lease",
                            "改写远端历史时的安全强推",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.push_lease = !this.push_lease;
                            cx.notify();
                        })),
                    )
                    .child(
                        opt_row(
                            "push-upstream",
                            set_up,
                            "-u 设置 upstream",
                            "首次推送新分支时勾选",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.push_upstream = !this.push_upstream;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line_soft)
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.faint)
                            .child("凭据走系统 credential helper"),
                    )
                    .child(div().ml_auto())
                    .child(self.ghost_btn(&t, "push-cancel", "取消").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.overlay = None;
                            cx.notify();
                        },
                    )))
                    .child(self.primary_btn(&t, "push-go", "Push").on_click(cx.listener(
                        |this, _, _, cx| {
                            let lease = this.push_lease;
                            let set_up = this.push_upstream;
                            this.overlay = None;
                            this.run_git(
                                "推送".to_string(),
                                move |cli| {
                                    let out = cli.push(None, None, set_up, lease)?;
                                    let l = out.stderr.lines().next().unwrap_or("").trim().to_string();
                                    Ok(if l.is_empty() { "完成".into() } else { l })
                                },
                                cx,
                            );
                        },
                    ))),
            )
    }

    // ----- stash ------------------------------------------------------------

    fn render_stash(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let stashes = self.stashes.clone();
        let mut list = div()
            .id("stash-list")
            .max_h(px(340.))
            .overflow_y_scroll()
            .py(px(4.));
        if stashes.is_empty() {
            list = list.child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child("没有 stash"),
            );
        }
        for (i, st) in stashes.iter().enumerate() {
            let when = chrono::DateTime::from_timestamp(st.time, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%m/%d %H:%M").to_string())
                .unwrap_or_default();
            let apply_id = st.id.clone();
            let pop_id = st.id.clone();
            let drop_id = st.id.clone();
            let act = |id: (&'static str, usize), label: &'static str| {
                div()
                    .id(id)
                    .px(px(7.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .text_size(px(11.5))
                    .text_color(t.cyan_deep)
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.cyan_16))
                    .child(label)
            };
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mx(px(8.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    .hover(move |s| s.bg(t.ink_05))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(st.id.clone()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(st.message.clone()))
                    .child(div().text_size(px(11.)).text_color(t.faint).child(when))
                    .child(
                        act(("stash-apply", i), "apply").on_click(cx.listener(move |this, _, _, cx| {
                            let id = apply_id.clone();
                            this.run_git(
                                format!("stash apply {id}"),
                                move |cli| {
                                    cli.stash_apply(&id, false)?;
                                    Ok(String::new())
                                },
                                cx,
                            );
                            this.refresh_stash_list(cx);
                        })),
                    )
                    .child(
                        act(("stash-pop", i), "pop").on_click(cx.listener(move |this, _, _, cx| {
                            let id = pop_id.clone();
                            this.overlay = None;
                            this.run_git(
                                format!("stash pop {id}"),
                                move |cli| {
                                    cli.stash_apply(&id, true)?;
                                    Ok(String::new())
                                },
                                cx,
                            );
                        })),
                    )
                    .child(
                        div()
                            .id(("stash-drop", i))
                            .px(px(7.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .text_size(px(11.5))
                            .text_color(t.mag_deep)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.mag_soft))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.overlay =
                                    Some(Overlay::Confirm(ConfirmAction::DropStash { id: drop_id.clone() }));
                                cx.notify();
                            }))
                            .child("drop"),
                    ),
            );
        }
        let include_untracked = self.stash_untracked;
        div()
            .w(px(560.))
            .flex()
            .flex_col()
            .child(self.panel_header(&t, "Stash", format!("{} 条", stashes.len()), cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .border_1()
                            .border_color(t.line)
                            .bg(t.surface)
                            .rounded(px(6.))
                            .px(px(9.))
                            .py(px(2.))
                            .text_size(px(12.5))
                            .child(Input::new(&self.stash_msg).appearance(false)),
                    )
                    .child(
                        div()
                            .id("stash-untracked")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stash_untracked = !this.stash_untracked;
                                cx.notify();
                            }))
                            .child(checkbox(&t, include_untracked, false))
                            .child("含未跟踪"),
                    )
                    .child(
                        self.primary_btn(&t, "stash-push", "Stash 当前变更")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let msg = this.stash_msg.read(cx).value().trim().to_string();
                                let untracked = this.stash_untracked;
                                this.run_git(
                                    "stash push".to_string(),
                                    move |cli| {
                                        cli.stash_push(&msg, untracked, false)?;
                                        Ok(String::new())
                                    },
                                    cx,
                                );
                                this.refresh_stash_list(cx);
                            })),
                    ),
            )
            .child(list)
    }

    // ----- snapshots (time machine v1) --------------------------------------

    fn render_snapshots(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let snaps = self.snapshots.clone();
        let mut list = div()
            .id("snap-list")
            .max_h(px(340.))
            .overflow_y_scroll()
            .py(px(4.));
        if snaps.is_empty() {
            list = list.child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child("还没有快照。破坏性操作（丢弃 / reset --hard）前会自动创建；也可以手动创建。"),
            );
        }
        for (i, sn) in snaps.iter().enumerate() {
            let when = chrono::DateTime::from_timestamp(sn.time, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%m/%d %H:%M").to_string())
                .unwrap_or_default();
            let apply_sha = sn.sha.clone();
            let del_ref = sn.refname.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mx(px(8.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    .hover(move |s| s.bg(t.ink_05))
                    .child(icon("clock-counter-clockwise", px(14.), t.cyan))
                    .child(div().flex_1().min_w_0().truncate().child(sn.message.clone()))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(sn.sha[..8].to_string()),
                    )
                    .child(div().text_size(px(11.)).text_color(t.faint).child(when))
                    .child(
                        div()
                            .id(("snap-apply", i))
                            .px(px(7.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .text_size(px(11.5))
                            .text_color(t.cyan_deep)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.cyan_16))
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new("把快照内容应用回工作区")
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let sha = apply_sha.clone();
                                this.overlay = None;
                                this.run_git(
                                    "恢复快照".to_string(),
                                    move |cli| {
                                        cli.snapshot_apply(&sha)?;
                                        Ok(String::new())
                                    },
                                    cx,
                                );
                            }))
                            .child("恢复"),
                    )
                    .child(
                        div()
                            .id(("snap-del", i))
                            .px(px(7.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .text_size(px(11.5))
                            .text_color(t.mag_deep)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.mag_soft))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.overlay = Some(Overlay::Confirm(ConfirmAction::DeleteSnapshot {
                                    refname: del_ref.clone(),
                                }));
                                cx.notify();
                            }))
                            .child("删除"),
                    ),
            );
        }
        div()
            .w(px(560.))
            .flex()
            .flex_col()
            .child(self.panel_header(
                &t,
                "时光机",
                format!("{} 个快照 · refs/sluice/snapshots", snaps.len()),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(8.))
                    .child(
                        self.primary_btn(&t, "snap-create", "立即创建快照")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_git(
                                    "创建快照".to_string(),
                                    |cli| match cli.snapshot_create("manual snapshot")? {
                                        Some(sha) => Ok(sha[..8].to_string()),
                                        None => Ok("工作区干净，无需快照".into()),
                                    },
                                    cx,
                                );
                                this.refresh_snapshot_list(cx);
                            })),
                    ),
            )
            .child(list)
    }

    // ----- settings ---------------------------------------------------------

    fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let telemetry = self.telemetry;
        let mut keys = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .max_h(px(300.))
            .overflow_hidden();
        for (action, mac, win) in KEYMAP_ROWS {
            keys = keys.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .py(px(2.))
                    .text_size(px(12.))
                    .child(div().flex_1().child(*action))
                    .child(
                        div()
                            .w(px(120.))
                            .font_family(FONT_MONO)
                            .text_size(px(11.5))
                            .text_color(t.cyan_deep)
                            .child(*mac),
                    )
                    .child(
                        div()
                            .w(px(130.))
                            .font_family(FONT_MONO)
                            .text_size(px(11.5))
                            .text_color(t.muted)
                            .child(*win),
                    ),
            );
        }
        div()
            .w(px(520.))
            .flex()
            .flex_col()
            .child(self.panel_header(&t, "设置", "IDEA 预设 keymap · 遥测".into(), cx))
            .child(
                div()
                    .id("settings-body")
                    .max_h(px(430.))
                    .overflow_y_scroll()
                    .px(px(16.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(section_label(
                                &t,
                                "键位（IDEA 预设 · VS Code 预设与逐条自定义在 M4 提供）",
                            ))
                            .child(keys),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(section_label(&t, "外观"))
                            .child(
                                div()
                                    .id("theme-toggle")
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.5))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
                                    .child(checkbox(&t, t.is_dark, false))
                                    .child("深色主题（⌘⇧T 随时切换；浅色为默认）"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(section_label(&t, "隐私"))
                            .child(
                                div()
                                    .id("telemetry-toggle")
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.5))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.telemetry = !this.telemetry;
                                        this.toast(
                                            if this.telemetry {
                                                "遥测已开启（本会话内存开关；持久化与上报端点随 M4 提供）"
                                            } else {
                                                "遥测已关闭（默认状态）"
                                            },
                                            cx,
                                        );
                                    }))
                                    .child(checkbox(&t, telemetry, false))
                                    .child("匿名使用统计与崩溃上报（默认关闭 · opt-in）"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(t.faint)
                                    .child("上报内容绝不包含仓库路径、文件名、提交信息或 diff。"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(section_label(&t, "关于"))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .child(format!("Sluice {} · Apache-2.0", env!("CARGO_PKG_VERSION"))),
                            )
                            .child(
                                div().text_size(px(11.5)).text_color(t.faint).child(
                                    "读路径 gitoxide · 写路径为你自己的 git · git2 被 cargo-deny 禁止",
                                ),
                            ),
                    ),
            )
    }

    // ----- confirm ----------------------------------------------------------

    fn render_confirm(&mut self, action: ConfirmAction, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let action2 = action.clone();
        div()
            .w(px(420.))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.))
                    .py(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_family(FONT_HEADING)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(15.))
                            .child(action.title()),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(19.))
                            .text_color(t.muted)
                            .child(action.body()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line_soft)
                    .child(div().ml_auto())
                    .child(self.ghost_btn(&t, "confirm-cancel", "取消").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.overlay = None;
                            cx.notify();
                        },
                    )))
                    .child(
                        div()
                            .id("confirm-go")
                            .px(px(16.))
                            .py(px(5.))
                            .bg(t.mag)
                            .text_color(t.surface)
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.mag_deep))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.run_confirmed(action2.clone(), cx)),
                            )
                            .child("确认执行"),
                    ),
            )
    }

    // ----- context menu -----------------------------------------------------

    pub(crate) fn show_ctx_menu(&mut self, ev: &MouseDownEvent, target: CtxTarget, cx: &mut Context<Self>) {
        self.ctx_menu = Some(CtxMenu {
            at: ev.position,
            target,
        });
        cx.notify();
    }

    pub(crate) fn render_ctx_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let t = self.theme;
        let menu = self.ctx_menu.clone()?;
        let vp = window.viewport_size();
        let x = menu.at.x.min(vp.width - px(230.));
        let y = menu.at.y.min(vp.height - px(280.));
        let mut items: Vec<(&'static str, &'static str, MenuAct, bool)> = Vec::new(); // (icon,label,act,danger)
        match &menu.target {
            CtxTarget::Commit(ix) => {
                let log = self.log.as_ref()?;
                let c = log.commits.get(*ix)?;
                let is_head = log.is_head(&c.id);
                items.push(("copy", "复制 hash", MenuAct::CopyHash(c.id.clone()), false));
                items.push((
                    "git-branch",
                    "从此提交新建分支…",
                    MenuAct::NewBranchFrom(c.id.clone()),
                    false,
                ));
                items.push((
                    "git-commit",
                    "Checkout 该提交（detached）",
                    MenuAct::CheckoutRev(c.id.clone()),
                    false,
                ));
                items.push((
                    "git-merge",
                    "Cherry-pick 到当前分支",
                    MenuAct::CherryPick(c.id.clone()),
                    false,
                ));
                items.push((
                    "arrow-clockwise",
                    "Revert 该提交",
                    MenuAct::Revert(c.id.clone()),
                    false,
                ));
                items.push((
                    "arrow-line-down",
                    "Reset --soft 到此",
                    MenuAct::Reset(c.id.clone(), "soft"),
                    false,
                ));
                items.push((
                    "arrow-line-down",
                    "Reset --mixed 到此",
                    MenuAct::Reset(c.id.clone(), "mixed"),
                    false,
                ));
                items.push((
                    "arrow-line-down",
                    "Reset --hard 到此…",
                    MenuAct::ResetHard(c.id.clone()),
                    true,
                ));
                if is_head && self.repo.info.head.ahead > 0 {
                    items.push((
                        "arrow-line-up",
                        "撤销该提交（保留变更）",
                        MenuAct::UndoCommit,
                        false,
                    ));
                }
            }
            CtxTarget::WorkFile {
                path,
                staged,
                untracked,
                ..
            } => {
                items.push(("copy", "复制路径", MenuAct::CopyPath(path.clone()), false));
                if *staged {
                    items.push(("arrow-line-up", "取消暂存", MenuAct::Unstage(path.clone()), false));
                } else {
                    items.push(("arrow-line-down", "暂存", MenuAct::Stage(path.clone()), false));
                }
                items.push((
                    "x",
                    if *untracked {
                        "删除未跟踪文件…"
                    } else {
                        "丢弃变更…"
                    },
                    MenuAct::Discard {
                        path: path.clone(),
                        staged: *staged,
                        untracked: *untracked,
                    },
                    true,
                ));
            }
        }
        Some(
            div()
                .id("ctx-menu")
                .occlude()
                .absolute()
                .left(x)
                .top(y)
                .w(px(224.))
                .bg(t.surface)
                .border_1()
                .border_color(t.line)
                .rounded(px(8.))
                .shadow_md()
                .py(px(4.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.ctx_menu = None;
                    cx.notify();
                }))
                .children(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, (ic, label, act, danger))| {
                            let color = if danger { t.mag_deep } else { t.ink };
                            div()
                                .id(("ctx-item", i))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .mx(px(4.))
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(5.))
                                .text_size(px(12.5))
                                .text_color(color)
                                .cursor_pointer()
                                .hover(move |s| s.bg(if danger { t.mag_soft } else { t.ink_05 }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ctx_menu = None;
                                    this.run_menu_act(act.clone(), cx);
                                }))
                                .child(icon_b(ic, px(13.), if danger { t.mag_deep } else { t.muted }))
                                .child(label)
                        }),
                ),
        )
    }

    fn run_menu_act(&mut self, act: MenuAct, cx: &mut Context<Self>) {
        match act {
            MenuAct::CopyHash(id) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.to_string()));
                self.toast(format!("已复制 {}", id.short(12)), cx);
            }
            MenuAct::CopyPath(p) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(p.clone()));
                self.toast(format!("已复制 {p}"), cx);
            }
            MenuAct::NewBranchFrom(id) => {
                self.overlay = Some(Overlay::NewBranch { from: Some(id) });
                cx.notify();
            }
            MenuAct::CheckoutRev(id) => {
                let sha = id.to_string();
                self.run_git(
                    format!("checkout {}", id.short(8)),
                    move |cli| {
                        cli.checkout(&sha)?;
                        Ok("现在处于 detached HEAD".into())
                    },
                    cx,
                );
            }
            MenuAct::CherryPick(id) => {
                let sha = id.to_string();
                self.run_git(
                    format!("cherry-pick {}", id.short(8)),
                    move |cli| {
                        cli.cherry_pick(&sha, false)?;
                        Ok(String::new())
                    },
                    cx,
                );
            }
            MenuAct::Revert(id) => {
                let sha = id.to_string();
                self.run_git(
                    format!("revert {}", id.short(8)),
                    move |cli| {
                        cli.revert(&sha)?;
                        Ok(String::new())
                    },
                    cx,
                );
            }
            MenuAct::Reset(id, mode) => {
                let sha = id.to_string();
                self.run_git(
                    format!("reset --{mode} {}", id.short(8)),
                    move |cli| {
                        cli.reset(mode, &sha)?;
                        Ok(String::new())
                    },
                    cx,
                );
            }
            MenuAct::ResetHard(id) => {
                self.overlay = Some(Overlay::Confirm(ConfirmAction::ResetHard { sha: id }));
                cx.notify();
            }
            MenuAct::UndoCommit => {
                self.run_git(
                    "撤销最近一次提交".to_string(),
                    |cli| {
                        cli.undo_last_commit()?;
                        Ok("变更已回到暂存区".into())
                    },
                    cx,
                );
            }
            MenuAct::Stage(p) => self.stage_paths(vec![p], cx),
            MenuAct::Unstage(p) => self.unstage_paths(vec![p], cx),
            MenuAct::Discard {
                path,
                staged,
                untracked,
            } => {
                self.overlay = Some(Overlay::Confirm(ConfirmAction::Discard {
                    path,
                    staged,
                    untracked,
                }));
                cx.notify();
            }
        }
    }

    /// The in-progress operation banner: continue / skip / abort (05 §6).
    pub(crate) fn op_banner(&mut self, op: InProgressOp, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let name = match op {
            InProgressOp::Merge => "Merge",
            InProgressOp::Rebase => "Rebase",
            InProgressOp::CherryPick => "Cherry-pick",
            InProgressOp::Revert => "Revert",
            InProgressOp::Bisect => "Bisect",
        };
        let step_btn = |id: (&'static str, usize), label: &'static str, step: &'static str, danger: bool| {
            let color = if danger { t.mag_deep } else { t.cyan_deep };
            div()
                .id(id)
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(11.5))
                .text_color(color)
                .border_1()
                .border_color(if danger { t.mag } else { t.cyan })
                .cursor_pointer()
                .hover(move |s| s.bg(if danger { t.mag_soft } else { t.cyan_soft }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.run_git(
                        step.to_string(),
                        move |cli| {
                            cli.op_step(op, step)?;
                            Ok(String::new())
                        },
                        cx,
                    );
                }))
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(5.))
            .bg(t.mag_soft)
            .border_b_1()
            .border_color(t.line_soft)
            .text_size(px(12.))
            .child(icon_b("git-merge", px(13.), t.mag_deep))
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(t.mag_deep)
                    .child(format!("{name} 进行中")),
            )
            .child(
                div()
                    .text_color(t.muted)
                    .child("解决冲突后 continue；随时 abort 回到操作前"),
            )
            .child(div().ml_auto())
            .child(step_btn(("op-continue", 0), "continue", "continue", false))
            .child(step_btn(("op-skip", 1), "skip", "skip", false))
            .child(step_btn(("op-abort", 2), "abort", "abort", true))
    }
}

#[derive(Clone, Debug)]
enum MenuAct {
    CopyHash(Oid),
    CopyPath(String),
    NewBranchFrom(Oid),
    CheckoutRev(Oid),
    CherryPick(Oid),
    Revert(Oid),
    Reset(Oid, &'static str),
    ResetHard(Oid),
    UndoCommit,
    Stage(String),
    Unstage(String),
    Discard {
        path: String,
        staged: bool,
        untracked: bool,
    },
}

#[allow(dead_code)]
fn _keep(_: &StashEntry, _: &SnapshotEntry, _: Tab) {}
