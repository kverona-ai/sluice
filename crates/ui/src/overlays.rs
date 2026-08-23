//! Modal overlays (branches, stash, snapshots, settings, push, new-branch,
//! confirmations) and the right-click context menus. All destructive actions
//! go through [`ConfirmAction`] and take a safety snapshot first where the
//! content is recoverable (05 §6).

use crate::i18n::tr;
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
    Worktrees,
    Rebase,
    Recent,
    AiConnect,
    Proposals,
    Askpass,
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
    RemoveWorktree {
        path: std::path::PathBuf,
    },
    MergePull {
        number: u64,
    },
}

impl ConfirmAction {
    fn title(&self) -> String {
        match self {
            ConfirmAction::DeleteBranch { name } => tf!("删除分支 {}？", name),
            ConfirmAction::ResetHard { sha } => tf!("Reset --hard 到 {}？", sha.short(8)),
            ConfirmAction::Discard { path, .. } => tf!("丢弃 {} 的变更？", path),
            ConfirmAction::DropStash { id } => tf!("删除 {}？", id),
            ConfirmAction::DeleteSnapshot { .. } => tr("删除该快照？").to_string(),
            ConfirmAction::RemoveWorktree { path } => tf!("删除 worktree {}？", path.display()),
            ConfirmAction::MergePull { number } => tf!("Squash 合并 #{}？", number),
        }
    }
    fn body(&self) -> String {
        match self {
            ConfirmAction::DeleteBranch { .. } => {
                tr("等价 git branch -D（未合并提交也会被删除）。分支指向的提交在 reflog 保留期内仍可找回。")
                    .into()
            }
            ConfirmAction::ResetHard { .. } => {
                tr("工作区与暂存区将被重置。执行前会自动创建时光机快照（可从「时光机」恢复）。").into()
            }
            ConfirmAction::Discard { untracked, .. } => {
                if *untracked {
                    tr("该文件未被跟踪，无法进入快照 —— 删除后不可恢复。").into()
                } else {
                    tr("执行前会自动创建时光机快照（可从「时光机」恢复）。").into()
                }
            }
            ConfirmAction::DropStash { .. } => {
                tr("stash 提交对象在 git 保留期内仍可通过其 SHA 找回。").into()
            }
            ConfirmAction::DeleteSnapshot { .. } => tr("仅删除快照引用；对象在 git gc 之前仍存在。").into(),
            ConfirmAction::RemoveWorktree { .. } => {
                tr("等价 git worktree remove --force：目录会被删除，分支本身保留。").into()
            }
            ConfirmAction::MergePull { .. } => {
                tr("经 gh / glab 以你的账号合并并删除源分支；不可撤销。").into()
            }
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
    /// A file row in the commit details pane.
    DetailFile { path: String, sha: Oid },
}

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
                    Err(e) => this.toast(tf!("读取 stash 失败：{}", format!("{e:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn open_snapshots(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else {
            self.toast("裸仓库没有工作区", cx);
            return;
        };
        let jj = self.repo.jj.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let snaps = cli.snapshot_list()?;
                    let ops = match jj {
                        Some(jj) => jj.op_log(30).unwrap_or_default(),
                        None => Vec::new(),
                    };
                    anyhow::Ok((snaps, ops))
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok((v, ops)) => {
                        this.snapshots = v;
                        this.jj_ops = ops;
                        this.overlay = Some(Overlay::Snapshots);
                    }
                    Err(e) => this.toast(tf!("读取快照失败：{}", format!("{e:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// jujutsu operation log actions (time machine, 05 §8): undo / restore.
    pub(crate) fn jj_op_action(&mut self, restore: Option<String>, cx: &mut Context<Self>) {
        let Some(jj) = self.repo.jj.clone() else { return };
        let what = match &restore {
            Some(id) => tf!("jj op restore {}", id),
            None => "jj undo".to_string(),
        };
        self.overlay = None;
        self.run_git(
            what,
            move |_cli| {
                let out = match restore {
                    Some(id) => jj.op_restore(&id)?,
                    None => jj.undo()?,
                };
                Ok(out.lines().next().unwrap_or("").trim().to_string())
            },
            cx,
        );
    }

    /// Close the topmost transient surface. Returns true when something closed.
    pub(crate) fn dismiss_top(&mut self, cx: &mut Context<Self>) -> bool {
        if self.popup.is_some() {
            self.popup = None;
            cx.notify();
            return true;
        }
        if self.ctx_menu.is_some() {
            self.ctx_menu = None;
            cx.notify();
            return true;
        }
        if self.overlay.is_some() {
            if self.overlay == Some(Overlay::Askpass) {
                self.cancel_askpass_silent();
            }
            self.overlay = None;
            cx.notify();
            return true;
        }
        false
    }

    // ----- AI connect wizard (M4, 03 §2) ------------------------------------

    pub(crate) fn open_ai_connect(&mut self, cx: &mut Context<Self>) {
        self.ai_status = sluice_bridge::connect::status_all();
        self.ai_report = None;
        self.overlay = Some(Overlay::AiConnect);
        cx.notify();
    }

    fn ai_register(&mut self, ids: Vec<&'static str>, cx: &mut Context<Self>) {
        self.ai_busy_connect = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let mut lines = Vec::new();
                    for spec in sluice_bridge::connect::TOOLS
                        .iter()
                        .filter(|t| ids.contains(&t.id))
                    {
                        match sluice_bridge::connect::register(spec) {
                            Ok(r) => lines.push(format!(
                                "✓ {} — {}{}{}",
                                r.tool,
                                if r.method == "cli" {
                                    tr("官方命令写入")
                                } else {
                                    tr("写入配置文件")
                                },
                                r.backup
                                    .as_ref()
                                    .map(|b| format!("（备份 {}）", b.display()))
                                    .unwrap_or_default(),
                                match r.verified {
                                    Some(true) => tr("，已验证"),
                                    Some(false) => tr("，验证未通过（请手动 `mcp list` 检查）"),
                                    None => "",
                                }
                            )),
                            Err(e) => lines.push(format!("✗ {} — {e:#}", spec.name)),
                        }
                        match sluice_bridge::hooks::install(spec.id) {
                            Ok(h) => lines.push(tf!(
                                "   hooks：{} · {}（{}）",
                                h.outcome,
                                h.path.display(),
                                h.note
                            )),
                            Err(e) => lines.push(tf!("   hooks 失败：{}", format!("{e:#}"))),
                        }
                    }
                    lines
                })
                .await;
            this.update(cx, |this, cx| {
                this.ai_busy_connect = false;
                this.ai_report = Some(res.join("\n"));
                this.ai_status = sluice_bridge::connect::status_all();
                this.console_note("ai connect", &this.ai_report.clone().unwrap_or_default());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_ai_connect(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        use sluice_bridge::connect::Registration;
        let t = self.theme;
        let statuses = self.ai_status.clone();
        let installed: Vec<&'static str> = statuses
            .iter()
            .filter(|s| s.state != Registration::NotInstalled)
            .map(|s| s.id)
            .collect();
        let busy = self.ai_busy_connect;
        let (cmd, args) = sluice_bridge::connect::server_command();
        let mut rows = div().flex().flex_col().gap(px(2.)).px(px(8.));
        for (i, st) in statuses.iter().enumerate() {
            let (label, color) = match &st.state {
                Registration::NotInstalled => (tr("未安装").to_string(), t.faint),
                Registration::Installed => (tr("已安装 · 未接入").to_string(), t.muted),
                Registration::Registered(c) => (tf!("已接入 → {}", c), t.cyan_deep),
            };
            let id = st.id;
            let can = st.state != Registration::NotInstalled;
            let registered = matches!(st.state, Registration::Registered(_));
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    .hover(move |s| s.bg(t.ink_05))
                    .child(crate::workbench::agent_badge(
                        &t,
                        match st.id {
                            "claude-code" => Agent::ClaudeCode,
                            "codex" => Agent::CodexCli,
                            "grok-build" => Agent::GrokBuild,
                            "gemini" => Agent::Gemini,
                            "kimi" => Agent::KimiCode,
                            "qwen" => Agent::QwenCode,
                            "copilot" => Agent::Copilot,
                            "zcode" => Agent::ZCode,
                            _ => Agent::OtherAi,
                        },
                    ))
                    .child(
                        div()
                            .w(px(110.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(st.name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(color)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_family(FONT_MONO)
                            .text_color(t.faint)
                            .child(
                                st.config_path
                                    .file_name()
                                    .map(|f| f.to_string_lossy().to_string())
                                    .unwrap_or_default(),
                            ),
                    )
                    .when(can && !registered, |d| {
                        d.child(
                            div()
                                .id(("ai-connect", i))
                                .px(px(9.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .bg(t.cyan)
                                .text_color(t.surface)
                                .text_size(px(11.5))
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.cyan_deep))
                                .on_click(cx.listener(move |this, _, _, cx| this.ai_register(vec![id], cx)))
                                .child(tr("接入")),
                        )
                    })
                    .when(registered, |d| {
                        d.child(
                            div()
                                .id(("ai-disconnect", i))
                                .px(px(7.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .text_size(px(11.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.ink_08))
                                .tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(tr(
                                        "从该工具的配置里移除 sluice 条目",
                                    ))
                                    .build(window, cx)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(spec) =
                                        sluice_bridge::connect::TOOLS.iter().find(|t| t.id == id)
                                    {
                                        match sluice_bridge::connect::unregister(spec) {
                                            Ok(()) => this.toast(tf!("已从 {} 移除", spec.name), cx),
                                            Err(e) => this.toast(tf!("移除失败：{}", format!("{e:#}")), cx),
                                        }
                                        this.ai_status = sluice_bridge::connect::status_all();
                                        cx.notify();
                                    }
                                }))
                                .child(tr("移除")),
                        )
                    }),
            );
        }
        div()
            .w(px(620.))
            .flex()
            .flex_col()
            .child(self.panel_header(&t, tr("AI 工具接入"), tf!("{} 个已安装 · 零配置，不索取 API key", installed.len()), cx))
            .child(
                div().px(px(16.)).pt(px(10.)).pb(px(4.)).text_size(px(12.)).text_color(t.muted).line_height(px(18.)).child(
                    tr("一键把 Sluice 注册为每个工具的 MCP server（只读工具：repo_status / list_changes / get_diff / log_query）。优先用工具自己的 `mcp add` 命令；没有命令的工具会直接写入其配置文件，改动前自动备份 *.sluice.bak。"),
                ),
            )
            .child(
                div()
                    .mx(px(16.))
                    .mb(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .bg(t.ink_05)
                    .font_family(FONT_MONO)
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .child(format!("{} {}", cmd, args.join(" "))),
            )
            .child(rows)
            .when_some(self.ai_report.clone(), |d, rep| {
                d.child(
                    div()
                        .mx(px(16.))
                        .mt(px(8.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .bg(t.cyan_soft)
                        .text_size(px(11.5))
                        .line_height(px(17.))
                        .child(rep),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(10.))
                    .mt(px(6.))
                    .border_t_1()
                    .border_color(t.line_soft)
                    .child(div().text_size(px(11.5)).text_color(t.faint).child(tr("接入同时安装 hooks（文件改动 / 会话结束 → 会话溯源）")))
                    .child(div().ml_auto())
                    .child(self.ghost_btn(&t, "ai-refresh", tr("重新检测")).on_click(cx.listener(|this, _, _, cx| {
                        this.ai_status = sluice_bridge::connect::status_all();
                        cx.notify();
                    })))
                    .child(
                        self.primary_btn(&t, "ai-connect-all", if busy { tr("接入中…") } else { tr("全部接入") })
                            .when(installed.is_empty() || busy, |d| d.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.ai_busy_connect {
                                    let ids: Vec<&'static str> = this
                                        .ai_status
                                        .iter()
                                        .filter(|s| s.state == sluice_bridge::connect::Registration::Installed)
                                        .map(|s| s.id)
                                        .collect();
                                    if ids.is_empty() {
                                        this.toast("没有需要接入的工具", cx);
                                    } else {
                                        this.ai_register(ids, cx);
                                    }
                                }
                            })),
                    ),
            )
    }

    // ----- confirm dispatch -------------------------------------------------

    pub(crate) fn run_confirmed(&mut self, action: ConfirmAction, cx: &mut Context<Self>) {
        self.overlay = None;
        match action {
            ConfirmAction::DeleteBranch { name } => {
                self.run_git(
                    tf!("删除分支 {}", name),
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
                            Some(_) => tr("已创建快照").into(),
                            None => tr("工作区原本干净").into(),
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
                    tf!("丢弃 {}", path),
                    move |cli| {
                        if untracked {
                            cli.run(&["clean", "-f", "--", &path])?;
                            return Ok(tr("未跟踪文件已删除").into());
                        }
                        let snap = cli.snapshot_create(&format!("before discard {path}"))?;
                        if staged {
                            cli.unstage(&[&path])?;
                        }
                        cli.discard(&[&path])?;
                        Ok(match snap {
                            Some(_) => tr("已创建快照").into(),
                            None => String::new(),
                        })
                    },
                    cx,
                );
            }
            ConfirmAction::DropStash { id } => {
                self.run_git(
                    tf!("删除 {}", id),
                    move |cli| {
                        cli.stash_drop(&id)?;
                        Ok(String::new())
                    },
                    cx,
                );
                self.overlay = Some(Overlay::Stash);
                self.refresh_stash_list(cx);
            }
            ConfirmAction::MergePull { number } => {
                self.pull_action(
                    tf!("合并 #{}", number),
                    move |f, cwd| sluice_bridge::forge::merge(f, cwd, number, true),
                    cx,
                );
            }
            ConfirmAction::RemoveWorktree { path } => {
                let shown = path.display().to_string();
                self.run_git(
                    tf!("删除 worktree {}", shown),
                    move |cli| {
                        cli.worktree_remove(&path, true)?;
                        Ok(String::new())
                    },
                    cx,
                );
                self.overlay = Some(Overlay::Worktrees);
                self.refresh_worktrees(cx);
            }
            ConfirmAction::DeleteSnapshot { refname } => {
                self.run_git(
                    tr("删除快照").to_string(),
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
            Overlay::Worktrees => self.render_worktrees(cx).into_any_element(),
            Overlay::Rebase => self.render_rebase(cx).into_any_element(),
            Overlay::Recent => self.render_recent(cx).into_any_element(),
            Overlay::AiConnect => self.render_ai_connect(cx).into_any_element(),
            Overlay::Proposals => self.render_proposals(cx).into_any_element(),
            Overlay::Askpass => match self.render_askpass(cx) {
                Some(e) => e.into_any_element(),
                None => div().into_any_element(),
            },
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

    pub(crate) fn panel_header(
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
                            d.child(div().text_size(px(11.)).text_color(t.faint).child(tr("当前")))
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
                                        gpui_component::tooltip::Tooltip::new(tr("merge 到当前分支"))
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
                                        gpui_component::tooltip::Tooltip::new(tr(
                                            "把当前分支 rebase 到它上面",
                                        ))
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let b = rebase_name.clone();
                                        this.overlay = None;
                                        this.run_git(
                                            tf!("rebase 到 {}", b),
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
                                        gpui_component::tooltip::Tooltip::new(tr("删除分支（需确认）"))
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
                tr("分支"),
                tf!("{} 本地 · {} 远端", locals.len(), remotes.len()),
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
                            .child(tr("点击分支 = checkout；脏工作区会提示")),
                    )
                    .child(div().ml_auto())
                    .child(
                        self.primary_btn(&t, "branch-new", tr("新建分支…"))
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
            .child(self.panel_header(&t, tr("新建分支"), tf!("基于 {}", from_label), cx))
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
                            .child(tr("创建后自动 checkout")),
                    )
                    .child(div().ml_auto())
                    .child(self.ghost_btn(&t, "nb-cancel", tr("取消")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.overlay = None;
                            cx.notify();
                        },
                    )))
                    .child(
                        self.primary_btn(&t, "nb-create", tr("创建"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let name = this.new_branch_name.read(cx).value().trim().to_string();
                                if name.is_empty() {
                                    this.toast("分支名不能为空", cx);
                                    return;
                                }
                                let from = from2.clone();
                                this.overlay = None;
                                this.run_git(
                                    tf!("新建分支 {}", name),
                                    move |cli| {
                                        cli.branch_create(&name, from.as_ref().map(|o| o.as_str()), true)?;
                                        Ok(String::new())
                                    },
                                    cx,
                                );
                            })),
                    ),
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
                tr("推送"),
                format!(
                        "{branch} → {}",
                        upstream
                            .clone()
                            .unwrap_or_else(|| tr("origin（新建 upstream）").into())
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
                    .child(div().text_size(px(13.)).child(tf!("{} 个未推送提交", ahead)))
                    .child(
                        opt_row(
                            "push-lease",
                            lease,
                            "--force-with-lease",
                            tr("改写远端历史时的安全强推"),
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
                            tr("-u 设置 upstream"),
                            tr("首次推送新分支时勾选"),
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
                            .child(tr("凭据走系统 credential helper")),
                    )
                    .child(div().ml_auto())
                    .child(
                        self.ghost_btn(&t, "push-cancel", tr("取消"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.overlay = None;
                                cx.notify();
                            })),
                    )
                    .child(self.primary_btn(&t, "push-go", "Push").on_click(cx.listener(
                        |this, _, _, cx| {
                            let lease = this.push_lease;
                            let set_up = this.push_upstream;
                            this.overlay = None;
                            this.run_git(
                                tr("推送").to_string(),
                                move |cli| {
                                    let out = cli.push(None, None, set_up, lease)?;
                                    let l = out.stderr.lines().next().unwrap_or("").trim().to_string();
                                    Ok(if l.is_empty() { tr("完成").into() } else { l })
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
                    .child(tr("没有 stash")),
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
            .child(self.panel_header(&t, "Stash", tf!("{} 条", stashes.len()), cx))
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
                            .child(tr("含未跟踪")),
                    )
                    .child(
                        self.primary_btn(&t, "stash-push", tr("Stash 当前变更"))
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
        let ops = self.jj_ops.clone();
        let jj_section: Option<gpui::AnyElement> = (!ops.is_empty()).then(|| {
            let mut list = div().flex().flex_col().px(px(8.)).py(px(4.));
            for (i, op) in ops.iter().take(12).enumerate() {
                let id = op.id.clone();
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .text_size(px(12.))
                        .hover(move |s| s.bg(t.ink_05))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(11.))
                                .text_color(t.cyan_deep)
                                .child(op.id.clone()),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(op.description.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(t.faint)
                                .child(op.time.clone()),
                        )
                        .when(op.current, |d| {
                            d.child(div().text_size(px(11.)).text_color(t.faint).child(tr("当前")))
                        })
                        .when(!op.current, |d| {
                            d.child(
                                div()
                                    .id(("jj-op", i))
                                    .px(px(7.))
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .text_size(px(11.5))
                                    .text_color(t.cyan_deep)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.cyan_16))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.jj_op_action(Some(id.clone()), cx)
                                    }))
                                    .child(tr("恢复到此")),
                            )
                        }),
                );
            }
            div()
                .flex()
                .flex_col()
                .border_b_1()
                .border_color(t.line_soft)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(16.))
                        .pt(px(8.))
                        .child(section_label(&t, tr("jj 操作日志（每一步都可回溯）")))
                        .child(div().ml_auto())
                        .child(
                            div()
                                .id("jj-undo")
                                .px(px(9.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(t.line)
                                .text_size(px(11.5))
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.ink_05))
                                .on_click(cx.listener(|this, _, _, cx| this.jj_op_action(None, cx)))
                                .child("jj undo"),
                        ),
                )
                .child(list)
                .into_any_element()
        });
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
                    .child(tr(
                        "还没有快照。破坏性操作（丢弃 / reset --hard）前会自动创建；也可以手动创建。",
                    )),
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
                                gpui_component::tooltip::Tooltip::new(tr("把快照内容应用回工作区"))
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let sha = apply_sha.clone();
                                this.overlay = None;
                                this.run_git(
                                    tr("恢复快照").to_string(),
                                    move |cli| {
                                        cli.snapshot_apply(&sha)?;
                                        Ok(String::new())
                                    },
                                    cx,
                                );
                            }))
                            .child(tr("恢复")),
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
                            .child(tr("删除")),
                    ),
            );
        }
        div()
            .w(px(560.))
            .flex()
            .flex_col()
            .child(self.panel_header(
                &t,
                tr("时光机"),
                tf!("{} 个快照 · refs/sluice/snapshots", snaps.len()),
                cx,
            ))
            .children(jj_section)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(8.))
                    .child(
                        self.primary_btn(&t, "snap-create", tr("立即创建快照"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_git(
                                    tr("创建快照").to_string(),
                                    |cli| match cli.snapshot_create("manual snapshot")? {
                                        Some(sha) => Ok(sha[..8].to_string()),
                                        None => Ok(tr("工作区干净，无需快照").into()),
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
        let preset = self.settings.keymap.clone();
        let overrides = crate::keymap::load_overrides();
        let mut keys = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .max_h(px(260.))
            .overflow_hidden();
        for e in crate::keymap::IDEA {
            let chord = crate::keymap::effective(e.action, &preset, &overrides);
            let overridden = overrides.contains_key(e.action);
            keys = keys.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .py(px(2.))
                    .text_size(px(12.))
                    .child(div().flex_1().child(tr(e.label)))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(e.action),
                    )
                    .child(
                        div()
                            .w(px(200.))
                            .font_family(FONT_MONO)
                            .text_size(px(11.5))
                            .text_color(if overridden { t.mag_deep } else { t.cyan_deep })
                            .child(chord),
                    ),
            );
        }
        let is_vscode = preset == "vscode";
        let preset_btn = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .px(px(9.))
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(11.5))
                .cursor_pointer()
                .border_1()
                .border_color(if on { t.cyan } else { t.line })
                .text_color(if on { t.cyan_deep } else { t.ink })
                .when(on, |d| d.bg(t.cyan_16))
                .hover(move |s| s.bg(t.ink_05))
                .child(label)
        };
        let keymap_controls = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .pb(px(4.))
            .child(
                preset_btn("km-idea", "IDEA", !is_vscode)
                    .on_click(cx.listener(|this, _, _, cx| this.set_keymap_preset("idea", cx))),
            )
            .child(
                preset_btn("km-vscode", "VS Code", is_vscode)
                    .on_click(cx.listener(|this, _, _, cx| this.set_keymap_preset("vscode", cx))),
            )
            .child(div().ml_auto())
            .child(
                preset_btn("km-edit", "keymap.json", false).on_click(cx.listener(|this, _, _, cx| {
                    match crate::keymap::write_template_if_missing() {
                        Ok(p) => {
                            let _ = open_path(&p);
                            this.toast(tf!("已打开 {}", p.display()), cx);
                        }
                        Err(e) => this.toast(tf!("无法写入 keymap.json：{}", e), cx),
                    }
                })),
            )
            .child(
                preset_btn("km-reload", "reload", false).on_click(cx.listener(|this, _, _, cx| {
                    this.apply_keymap(cx);
                    this.toast(tr("已重新加载 keymap"), cx);
                })),
            );
        div()
            .w(px(520.))
            .flex()
            .flex_col()
            .child(self.panel_header(&t, tr("设置"), tr("IDEA 预设 keymap · 遥测").into(), cx))
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
                                tr("键位（预设 + ~/.sluice/keymap.json 逐条覆盖）"),
                            ))
                            .child(keymap_controls)
                            .child(keys),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(section_label(&t, tr("外观")))
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
                                    .child(tr("深色主题（⌘⇧T 随时切换；浅色为默认）")),
                            )
                            .child(
                                div()
                                    .id("lang-toggle")
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.5))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, window, cx| this.toggle_lang(window, cx)))
                                    .child(checkbox(&t, crate::i18n::lang() == crate::i18n::Lang::En, false))
                                    .child(tr("English 界面（⌘⇧L 切换；中文为默认）")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(section_label(&t, tr("隐私")))
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
                                        this.save_settings();
                                        this.toast(
                                            if this.telemetry {
                                                tr("遥测已开启（已持久化；上报端点随 public beta 启用）")
                                            } else {
                                                tr("遥测已关闭（默认状态）")
                                            },
                                            cx,
                                        );
                                    }))
                                    .child(checkbox(&t, telemetry, false))
                                    .child(tr("匿名使用统计与崩溃上报（默认关闭 · opt-in）")),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(t.faint)
                                    .child(tr("上报内容绝不包含仓库路径、文件名、提交信息或 diff。")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(11.5))
                                    .text_color(t.faint)
                                    .child(tf!(
                                        "端点：{} · 队列 {} 条",
                                        sluice_bridge::telemetry::endpoint(Some(
                                            &self.settings.telemetry_endpoint
                                        ))
                                        .unwrap_or_else(|| tr("未配置（仅本地队列）").to_string()),
                                        sluice_bridge::telemetry::queued()
                                    ))
                                    .child(
                                        div()
                                            .id("tele-flush")
                                            .px(px(7.))
                                            .py(px(1.))
                                            .rounded(px(4.))
                                            .text_color(t.cyan_deep)
                                            .cursor_pointer()
                                            .hover(move |s| s.bg(t.cyan_16))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                let ep = this.settings.telemetry_endpoint.clone();
                                                match sluice_bridge::telemetry::flush(Some(&ep)) {
                                                    Ok(0) => {
                                                        this.toast(tr("没有可发送的事件或未配置端点"), cx)
                                                    }
                                                    Ok(n) => this.toast(tf!("已发送 {} 条事件", n), cx),
                                                    Err(e) => {
                                                        this.toast(tf!("发送失败：{}", format!("{e:#}")), cx)
                                                    }
                                                }
                                            }))
                                            .child(tr("立即发送")),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(section_label(&t, tr("更新")))
                            .child(
                                div()
                                    .id("upd-toggle")
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.5))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings.check_updates = !this.settings.check_updates;
                                        this.save_settings();
                                        cx.notify();
                                    }))
                                    .child(checkbox(&t, self.settings.check_updates, false))
                                    .child(tr("启动时检查新版本（仅发送版本号到 GitHub Releases）")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.))
                                    .child(match &self.update_available {
                                        Some((tag, _)) => tf!("新版本 {} 可用", tag),
                                        None if self.update_checked => {
                                            tf!("已是最新（{}）", env!("CARGO_PKG_VERSION"))
                                        }
                                        None => tf!("当前 {}", env!("CARGO_PKG_VERSION")),
                                    })
                                    .child(
                                        div()
                                            .id("upd-check")
                                            .px(px(7.))
                                            .py(px(1.))
                                            .rounded(px(4.))
                                            .text_size(px(11.5))
                                            .text_color(t.cyan_deep)
                                            .cursor_pointer()
                                            .hover(move |s| s.bg(t.cyan_16))
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.check_updates_now(cx)),
                                            )
                                            .child(tr("立即检查")),
                                    )
                                    .when_some(self.update_available.clone(), |d, (_, url)| {
                                        d.child(
                                            div()
                                                .id("upd-open")
                                                .px(px(7.))
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .text_size(px(11.5))
                                                .bg(t.cyan)
                                                .text_color(t.surface)
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
                                                .child(tr("下载")),
                                        )
                                    }),
                            )
                            .child(div().text_size(px(11.5)).text_color(t.faint).child(tr(
                                "下载后替换 .app / exe 即可；签名与自动替换随 public beta 提供。",
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(section_label(&t, tr("关于")))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .child(format!("Sluice {} · Apache-2.0", env!("CARGO_PKG_VERSION"))),
                            )
                            .child(div().text_size(px(11.5)).text_color(t.faint).child(tr(
                                "读路径 gitoxide · 写路径为你自己的 git · git2 被 cargo-deny 禁止",
                            ))),
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
                    .child(
                        self.ghost_btn(&t, "confirm-cancel", tr("取消"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.overlay = None;
                                cx.notify();
                            })),
                    )
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
                            .child(tr("确认执行")),
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
                items.push(("copy", tr("复制 hash"), MenuAct::CopyHash(c.id.clone()), false));
                items.push((
                    "git-branch",
                    tr("从此提交新建分支…"),
                    MenuAct::NewBranchFrom(c.id.clone()),
                    false,
                ));
                items.push((
                    "git-commit",
                    tr("Checkout 该提交（detached）"),
                    MenuAct::CheckoutRev(c.id.clone()),
                    false,
                ));
                items.push((
                    "git-merge",
                    tr("Cherry-pick 到当前分支"),
                    MenuAct::CherryPick(c.id.clone()),
                    false,
                ));
                items.push((
                    "arrow-clockwise",
                    tr("Revert 该提交"),
                    MenuAct::Revert(c.id.clone()),
                    false,
                ));
                items.push((
                    "arrow-line-down",
                    tr("Reset --soft 到此"),
                    MenuAct::Reset(c.id.clone(), "soft"),
                    false,
                ));
                items.push((
                    "arrow-line-down",
                    tr("Reset --mixed 到此"),
                    MenuAct::Reset(c.id.clone(), "mixed"),
                    false,
                ));
                items.push((
                    "arrow-line-down",
                    tr("Reset --hard 到此…"),
                    MenuAct::ResetHard(c.id.clone()),
                    true,
                ));
                items.push((
                    "arrows-down-up",
                    tr("交互式 rebase（从此提交起）…"),
                    MenuAct::RebaseFrom(c.id.clone()),
                    false,
                ));
                if is_head && self.repo.info.head.ahead > 0 {
                    items.push((
                        "arrow-line-up",
                        tr("撤销该提交（保留变更）"),
                        MenuAct::UndoCommit,
                        false,
                    ));
                }
            }
            CtxTarget::DetailFile { path, sha } => {
                items.push(("copy", tr("复制路径"), MenuAct::CopyPath(path.clone()), false));
                items.push((
                    "clock-counter-clockwise",
                    tr("文件历史"),
                    MenuAct::FileHistory(path.clone()),
                    false,
                ));
                items.push((
                    "eye",
                    tr("Blame（此提交）"),
                    MenuAct::Blame(path.clone(), Some(sha.to_string())),
                    false,
                ));
                items.push((
                    "eye",
                    tr("Blame（工作区）"),
                    MenuAct::Blame(path.clone(), None),
                    false,
                ));
            }
            CtxTarget::WorkFile {
                path,
                staged,
                untracked,
                ..
            } => {
                items.push(("copy", tr("复制路径"), MenuAct::CopyPath(path.clone()), false));
                items.push((
                    "clock-counter-clockwise",
                    tr("文件历史"),
                    MenuAct::FileHistory(path.clone()),
                    false,
                ));
                if !*untracked {
                    items.push((
                        "eye",
                        tr("Blame（工作区）"),
                        MenuAct::Blame(path.clone(), None),
                        false,
                    ));
                }
                if *staged {
                    items.push((
                        "arrow-line-up",
                        tr("取消暂存"),
                        MenuAct::Unstage(path.clone()),
                        false,
                    ));
                } else {
                    items.push(("arrow-line-down", tr("暂存"), MenuAct::Stage(path.clone()), false));
                }
                items.push((
                    "x",
                    if *untracked {
                        tr("删除未跟踪文件…")
                    } else {
                        tr("丢弃变更…")
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
                self.toast(tf!("已复制 {}", id.short(12)), cx);
            }
            MenuAct::CopyPath(p) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(p.clone()));
                self.toast(tf!("已复制 {}", p), cx);
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
                        Ok(tr("现在处于 detached HEAD").into())
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
                    tr("撤销最近一次提交").to_string(),
                    |cli| {
                        cli.undo_last_commit()?;
                        Ok(tr("变更已回到暂存区").into())
                    },
                    cx,
                );
            }
            MenuAct::RebaseFrom(id) => self.open_rebase_from(id, cx),
            MenuAct::FileHistory(p) => {
                self.open_file_view(p, None, crate::file_view::FileViewMode::History, cx)
            }
            MenuAct::Blame(p, rev) => self.open_file_view(p, rev, crate::file_view::FileViewMode::Blame, cx),
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
                    .child(tf!("{} 进行中", name)),
            )
            .child(
                div()
                    .text_color(t.muted)
                    .child(tr("解决冲突后 continue；随时 abort 回到操作前")),
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
    FileHistory(String),
    Blame(String, Option<String>),
    RebaseFrom(Oid),
}

#[allow(dead_code)]
fn _keep(_: &StashEntry, _: &SnapshotEntry, _: Tab) {}

/// Open a file with the OS default handler (best effort).
fn open_path(p: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(p).spawn().map(|_| ())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &p.to_string_lossy()])
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::process::Command::new("xdg-open").arg(p).spawn().map(|_| ())
    }
}
