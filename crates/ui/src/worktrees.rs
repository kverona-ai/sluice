//! Worktree management (M3): list / add / remove, open a worktree as a repository,
//! and launch an AI CLI inside it (05 §6 — parallel agents per worktree).

use std::path::PathBuf;

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use gpui_component::input::Input;
use sluice_backend_cli::WorktreeEntry;

use crate::i18n::tr;
use crate::icons::{icon_b, icon_f};
use crate::overlays::{ConfirmAction, Overlay};
use crate::theme::FONT_MONO;
use crate::workbench::Workbench;

impl Workbench {
    pub(crate) fn open_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else {
            self.toast(tr("裸仓库没有工作区"), cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            let res = cx.background_spawn(async move { cli.worktree_list() }).await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        this.worktrees = v;
                        this.overlay = Some(Overlay::Worktrees);
                    }
                    Err(e) => this.toast(tf!("读取 worktree 失败：{}", format!("{e:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn refresh_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else { return };
        cx.spawn(async move |this, cx| {
            if let Ok(v) = cx.background_spawn(async move { cli.worktree_list() }).await {
                this.update(cx, |this, cx| {
                    this.worktrees = v;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Create `<repo>/../<repo>-<branch>` (or the typed path) on a new branch.
    pub(crate) fn worktree_add_from_input(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else { return };
        let branch = self.worktree_branch.read(cx).value().trim().to_string();
        if branch.is_empty() {
            self.toast(tr("请输入新分支名"), cx);
            return;
        }
        let root = cli.workdir().to_path_buf();
        let slug: String = branch
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let path = root
            .parent()
            .map(|p| {
                p.join(format!(
                    "{}-{}",
                    root.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "repo".into()),
                    slug
                ))
            })
            .unwrap_or_else(|| root.join(format!(".worktrees/{slug}")));
        let exists_branch = self.log.as_ref().is_some_and(|l| {
            l.refs
                .iter()
                .any(|r| r.kind == sluice_core::RefKind::LocalBranch && r.short_name == branch)
        });
        let shown = path.display().to_string();
        self.run_git(
            tf!("新建 worktree {}", shown.clone()),
            move |cli| {
                cli.worktree_add(&path, &branch, !exists_branch)?;
                Ok(path.display().to_string())
            },
            cx,
        );
        self.refresh_worktrees(cx);
    }

    pub(crate) fn render_worktrees(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let list = self.worktrees.clone();
        let current = self.repo.cli.as_ref().map(|c| c.workdir().to_path_buf());
        let ai = crate::ai::detect_tool();
        let mut rows = div().id("wt-list").max_h(px(340.)).overflow_y_scroll().py(px(4.));
        if list.is_empty() {
            rows = rows.child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child(tr("没有 worktree")),
            );
        }
        for (i, w) in list.iter().enumerate() {
            let is_cur = current.as_ref().map(|c| c.canonicalize().ok()) == Some(w.path.canonicalize().ok());
            let p_open = w.path.clone();
            let p_rm = w.path.clone();
            let p_ai = w.path.clone();
            let ai_name = ai.as_ref().map(|(_, n)| n.clone());
            let ai_bin = ai
                .as_ref()
                .map(|(id, _)| {
                    crate::ai::TOOLS
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.bin.to_string())
                })
                .flatten();
            rows = rows.child(
                div()
                    .id(("wt", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mx(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .when(is_cur, |d| d.bg(t.sel))
                    .hover(move |s| s.bg(if is_cur { t.sel } else { t.ink_05 }))
                    .child(icon_f("folder", px(14.), if w.locked { t.mag } else { t.muted }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(w.branch.clone().unwrap_or_else(|| "(detached)".into())),
                                    )
                                    .child(
                                        div()
                                            .font_family(FONT_MONO)
                                            .text_size(px(11.))
                                            .text_color(t.faint)
                                            .child(w.head.chars().take(8).collect::<String>()),
                                    )
                                    .when(is_cur, |d| {
                                        d.child(
                                            div().text_size(px(11.)).text_color(t.faint).child(tr("当前")),
                                        )
                                    })
                                    .when(w.bare, |d| {
                                        d.child(div().text_size(px(11.)).text_color(t.faint).child("bare"))
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(t.faint)
                                    .truncate()
                                    .child(w.path.display().to_string()),
                            ),
                    )
                    .when(!is_cur && !w.bare, |d| {
                        d.child(
                            div()
                                .id(("wt-open", i))
                                .px(px(7.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .text_size(px(11.5))
                                .text_color(t.cyan_deep)
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.cyan_16))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.overlay = None;
                                    this.switch_repository(p_open.clone(), window, cx);
                                }))
                                .child(tr("打开")),
                        )
                    })
                    .when(ai_name.is_some() && !w.bare, |d| {
                        let label = tf!("在 {} 中打开", ai_name.clone().unwrap_or_default());
                        d.child(
                            div()
                                .id(("wt-ai", i))
                                .px(px(7.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .text_size(px(11.5))
                                .text_color(t.mag_deep)
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.mag_soft))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.launch_ai_in(p_ai.clone(), ai_bin.clone(), cx);
                                }))
                                .child(label),
                        )
                    })
                    .when(!is_cur && !w.bare, |d| {
                        d.child(
                            div()
                                .id(("wt-rm", i))
                                .w(px(20.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.mag_soft))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.overlay = Some(Overlay::Confirm(ConfirmAction::RemoveWorktree {
                                        path: p_rm.clone(),
                                    }));
                                    cx.notify();
                                }))
                                .child(icon_b("x", px(11.), t.mag_deep)),
                        )
                    }),
            );
        }
        div()
            .w(px(600.))
            .flex()
            .flex_col()
            .child(self.panel_header(
                &t,
                tr("Worktree"),
                tf!("{} 个工作树 · 每个 AI 代理一个目录并行开发", list.len()),
                cx,
            ))
            .child(rows)
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
                            .flex_1()
                            .border_1()
                            .border_color(t.line)
                            .bg(t.surface)
                            .rounded(px(6.))
                            .px(px(9.))
                            .py(px(2.))
                            .text_size(px(12.5))
                            .child(Input::new(&self.worktree_branch).appearance(false)),
                    )
                    .child(
                        div()
                            .id("wt-add")
                            .px(px(14.))
                            .py(px(5.))
                            .bg(t.cyan)
                            .text_color(t.surface)
                            .rounded(px(4.))
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.cyan_deep))
                            .on_click(cx.listener(|this, _, _, cx| this.worktree_add_from_input(cx)))
                            .child(tr("新建 worktree（同级目录）")),
                    ),
            )
    }

    /// Open the user's terminal in `dir` running the AI CLI there (macOS: Terminal.app;
    /// Windows: a new console; else x-terminal-emulator). Best effort, never blocks.
    pub(crate) fn launch_ai_in(&mut self, dir: PathBuf, bin: Option<String>, cx: &mut Context<Self>) {
        let Some(bin) = bin else {
            self.toast(tr("未检测到 AI CLI"), cx);
            return;
        };
        let d = dir.display().to_string();
        let res = if cfg!(target_os = "macos") {
            let script = format!(
                "tell application \"Terminal\"\nactivate\ndo script \"cd {} && {}\"\nend tell",
                shell_q(&d),
                bin
            );
            std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .spawn()
                .map(|_| ())
        } else if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args([
                    "/C",
                    "start",
                    "",
                    "cmd",
                    "/K",
                    &format!("cd /d \"{}\" && {}", d, bin),
                ])
                .spawn()
                .map(|_| ())
        } else {
            std::process::Command::new("x-terminal-emulator")
                .args(["-e", &format!("sh -c 'cd {} && {}'", shell_q(&d), bin)])
                .spawn()
                .map(|_| ())
        };
        match res {
            Ok(()) => self.toast(tf!("已启动 {} · 目录 {}", bin, d), cx),
            Err(e) => self.toast(tf!("启动失败：{}", e), cx),
        }
    }
}

fn shell_q(s: &str) -> String {
    if s.contains(' ') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

#[allow(dead_code)]
fn _keep(_: &WorktreeEntry, _: &mut Window) {}
