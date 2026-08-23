//! PR / MR review tab (M6, 05 §8): list from the logged-in forge CLI, local diff of
//! merge-base..head through Sluice's own viewer, review / comment / merge actions,
//! AI pre-review drafted into the comment box (still a proposal — you post it).

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use gpui_component::input::Input;
use sluice_bridge::forge::{Forge, PullRequest, ReviewVerdict};
use sluice_core::*;

use crate::diff_view::DiffView;
use crate::i18n::tr;
use crate::icons::icon_b;
use crate::theme::{FONT_HEADING, FONT_MONO};
use crate::workbench::{Workbench, chrome_button, section_label};

#[derive(Clone, Debug, Default)]
pub struct PullState {
    pub forge: Option<Forge>,
    pub cli_missing: bool,
    pub list: Vec<PullRequest>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected: Option<u64>,
    pub detail: Option<PullRequest>,
    pub files: Vec<FileChange>,
    pub base_oid: Option<Oid>,
    pub head_oid: Option<Oid>,
    pub busy: bool,
    pub ai_busy: bool,
    pub loaded_once: bool,
}

impl Workbench {
    /// First visit of the PR tab: detect the forge and list open PRs.
    pub(crate) fn ensure_pulls(&mut self, cx: &mut Context<Self>) {
        if self.pulls.loaded_once {
            return;
        }
        self.pulls.loaded_once = true;
        self.reload_pulls(cx);
    }

    pub(crate) fn reload_pulls(&mut self, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else { return };
        self.pulls.loading = true;
        self.pulls.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let url = cli.remote_url("origin")?;
                    let forge = sluice_bridge::forge::detect(&url)
                        .ok_or_else(|| anyhow::anyhow!("origin 不是 GitHub / GitLab：{url}"))?;
                    if sluice_bridge::forge::cli_path(forge).is_none() {
                        return Ok((forge, true, Vec::new()));
                    }
                    let list = sluice_bridge::forge::list(forge, cli.workdir(), 50)?;
                    anyhow::Ok((forge, false, list))
                })
                .await;
            this.update(cx, |this, cx| {
                this.pulls.loading = false;
                match res {
                    Ok((forge, missing, list)) => {
                        this.pulls.forge = Some(forge);
                        this.pulls.cli_missing = missing;
                        this.pulls.list = list;
                    }
                    Err(e) => this.pulls.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Select a PR: load metadata + fetch its head locally + compute the file list.
    pub fn select_pull(&mut self, n: u64, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else { return };
        let Some(forge) = self.pulls.forge else { return };
        self.pulls.selected = Some(n);
        self.pulls.detail = None;
        self.pulls.files.clear();
        self.pull_diff = None;
        self.pulls.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let detail = sluice_bridge::forge::view(forge, cli.workdir(), n)?;
                    let local_ref = format!("refs/remotes/origin/{}/{}", forge.noun().to_lowercase(), n);
                    cli.fetch_refspec("origin", &format!("{}:{}", forge.head_ref(n), local_ref))?;
                    let head = cli.rev_parse(&local_ref)?;
                    let base_ref = format!("origin/{}", detail.base);
                    let _ = cli.fetch_refspec(
                        "origin",
                        &format!("refs/heads/{}:refs/remotes/origin/{}", detail.base, detail.base),
                    );
                    let base = cli.merge_base(&base_ref, &head).unwrap_or_else(|_| head.clone());
                    let files: Vec<FileChange> = cli
                        .diff_name_status(&base, &head)?
                        .into_iter()
                        .map(|(code, path, old)| FileChange {
                            path,
                            old_path: old,
                            kind: match code {
                                'A' => ChangeKind::Added,
                                'D' => ChangeKind::Deleted,
                                'R' => ChangeKind::Renamed,
                                'C' => ChangeKind::Copied,
                                _ => ChangeKind::Modified,
                            },
                            additions: None,
                            deletions: None,
                            binary: false,
                        })
                        .collect();
                    anyhow::Ok((detail, Oid::new(&base), Oid::new(&head), files))
                })
                .await;
            this.update(cx, |this, cx| {
                this.pulls.busy = false;
                match res {
                    Ok((detail, base, head, files)) => {
                        this.pulls.detail = Some(detail);
                        this.pulls.base_oid = Some(base);
                        this.pulls.head_oid = Some(head);
                        this.pulls.files = files;
                    }
                    Err(e) => this.toast(tf!("加载 {} 失败：{}", forge.noun(), format!("{e:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_pull_file(&mut self, change: FileChange, cx: &mut Context<Self>) {
        let (Some(base), Some(head)) = (self.pulls.base_oid.clone(), self.pulls.head_oid.clone()) else {
            return;
        };
        let reader = self.repo.reader.clone();
        let opts = self.diff_opts;
        let mut dv = DiffView::loading(change.path.clone(), change.clone());
        dv.stageable = false;
        self.pull_diff = Some(dv);
        cx.notify();
        let path = change.path.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    sluice_domain::diff_range_file(
                        reader.as_ref(),
                        &base,
                        &head,
                        &change.path,
                        change.old_path.as_deref(),
                        &opts,
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(dv) = this.pull_diff.as_mut().filter(|d| d.title == path) {
                    dv.set_result(res.map_err(|e| format!("{e:#}")));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn pull_action<F>(&mut self, what: String, f: F, cx: &mut Context<Self>)
    where
        F: FnOnce(Forge, &std::path::Path) -> anyhow::Result<String> + Send + 'static,
    {
        let Some(cli) = self.repo.cli.clone() else { return };
        let Some(forge) = self.pulls.forge else { return };
        self.pulls.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx.background_spawn(async move { f(forge, cli.workdir()) }).await;
            this.update(cx, |this, cx| {
                this.pulls.busy = false;
                match res {
                    Ok(out) => {
                        let line = out.lines().last().unwrap_or("").trim().to_string();
                        this.repo.console.note(&what, &line);
                        this.toast(
                            format!(
                                "{what}：{}",
                                if line.is_empty() {
                                    tr("完成").to_string()
                                } else {
                                    line
                                }
                            ),
                            cx,
                        );
                        if let Some(n) = this.pulls.selected {
                            this.select_pull(n, cx);
                        }
                        this.refresh(cx);
                    }
                    Err(e) => this.toast(tf!("{} 失败：{}", what, format!("{e:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn pull_review(&mut self, verdict: ReviewVerdict, cx: &mut Context<Self>) {
        let Some(n) = self.pulls.selected else { return };
        let body = self.pull_comment.read(cx).value().trim().to_string();
        if verdict != ReviewVerdict::Approve && body.is_empty() {
            self.toast(tr("请先写评论内容"), cx);
            return;
        }
        let what = match verdict {
            ReviewVerdict::Approve => tr("批准").to_string(),
            ReviewVerdict::RequestChanges => tr("请求修改").to_string(),
            ReviewVerdict::Comment => tr("评论").to_string(),
        };
        self.pull_action(
            what,
            move |forge, cwd| sluice_bridge::forge::review(forge, cwd, n, verdict, &body),
            cx,
        );
    }

    pub(crate) fn pull_ai_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tool) = self.ai_tool.clone() else {
            self.toast(tr("未检测到 AI CLI"), cx);
            return;
        };
        let (Some(base), Some(head)) = (self.pulls.base_oid.clone(), self.pulls.head_oid.clone()) else {
            return;
        };
        let files = self.pulls.files.clone();
        let reader = self.repo.reader.clone();
        let opts = self.diff_opts;
        let title = self
            .pulls
            .detail
            .as_ref()
            .map(|d| d.title.clone())
            .unwrap_or_default();
        self.pulls.ai_busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let mut patch = String::new();
                    for f in files.iter().take(40) {
                        let d = sluice_domain::diff_range_file(
                            reader.as_ref(),
                            &base,
                            &head,
                            &f.path,
                            f.old_path.as_deref(),
                            &opts,
                        )?;
                        patch.push_str(&format!("--- a/{0}\n+++ b/{0}\n", f.path));
                        for h in &d.hunks {
                            patch.push_str(&h.header());
                            patch.push('\n');
                            for l in &h.lines {
                                let sign = match l.kind {
                                    sluice_core::diff::LineKind::Context => ' ',
                                    sluice_core::diff::LineKind::Added => '+',
                                    sluice_core::diff::LineKind::Deleted => '-',
                                };
                                patch.push(sign);
                                patch.push_str(&l.text);
                                patch.push('\n');
                            }
                        }
                        if patch.len() > 120 * 1024 {
                            patch.push_str("\n… (truncated)\n");
                            break;
                        }
                    }
                    crate::ai::draft_review(&tool.0, &title, &patch)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.pulls.ai_busy = false;
                match res {
                    Ok(text) => {
                        this.pull_comment
                            .update(cx, |s, cx| s.set_value(text, window, cx));
                        this.toast(tr("AI 预审草稿已填入评论框——请审阅后再发布"), cx);
                    }
                    Err(e) => this.toast(tf!("AI 预审失败：{}", format!("{e:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_pulls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let st = self.pulls.clone();
        let forge = st.forge;
        let noun = forge.map(|f| f.noun()).unwrap_or("PR");

        // ----- left: list -----
        let mut list = div()
            .id("pr-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(4.));
        if st.loading {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_size(px(12.5))
                    .text_color(t.faint)
                    .child(tr("加载中…")),
            );
        } else if let Some(e) = &st.error {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_size(px(12.5))
                    .text_color(t.mag_deep)
                    .child(e.clone()),
            );
        } else if st.cli_missing {
            let cli = forge.map(|f| f.cli()).unwrap_or("gh");
            list = list.child(
                div()
                    .p(px(12.))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .line_height(px(19.))
                    .child(tf!(
                        "未找到 {}。安装后执行 `{} auth login`，Sluice 复用它的登录态，不索取 token。",
                        cli,
                        cli
                    )),
            );
        } else if st.list.is_empty() {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child(tf!("没有打开的 {}", noun)),
            );
        }
        for (i, pr) in st.list.iter().enumerate() {
            let n = pr.number;
            let selected = st.selected == Some(n);
            let decision_color = match pr.decision.as_str() {
                "APPROVED" | "mergeable" => t.cyan_deep,
                "CHANGES_REQUESTED" => t.mag_deep,
                _ => t.faint,
            };
            list = list.child(
                div()
                    .id(("pr", i))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .mx(px(6.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(t.sel))
                    .hover(move |s| s.bg(if selected { t.sel } else { t.ink_05 }))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_pull(n, cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(11.))
                                    .text_color(t.cyan_deep)
                                    .child(format!("#{n}")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(pr.title.clone()),
                            )
                            .when(pr.draft, |d| {
                                d.child(
                                    div()
                                        .text_size(px(10.))
                                        .px(px(4.))
                                        .rounded(px(3.))
                                        .bg(t.ink_08)
                                        .text_color(t.muted)
                                        .child("draft"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(pr.author.clone())
                            .child(format!("{} → {}", pr.head, pr.base))
                            .when(!pr.decision.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_color(decision_color)
                                        .child(pr.decision.to_lowercase().replace('_', " ")),
                                )
                            }),
                    ),
            );
        }
        let left = div()
            .size_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(34.))
                    .px(px(10.))
                    .border_b_1()
                    .border_color(t.line_soft)
                    .child(
                        div()
                            .font_family(FONT_HEADING)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(13.))
                            .child(match forge {
                                Some(f) => format!("{} {}", f.label(), f.noun()),
                                None => "PR".into(),
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(tf!("{} 个打开", st.list.len())),
                    )
                    .child(div().ml_auto())
                    .child(
                        chrome_button("pr-reload", &t, "arrow-clockwise", tr("刷新列表"), false)
                            .on_click(cx.listener(|this, _, _, cx| this.reload_pulls(cx))),
                    ),
            )
            .child(list);

        // ----- right: detail or diff -----
        let right: gpui::AnyElement = if self.pull_diff.is_some() {
            self.render_pull_diff(cx).into_any_element()
        } else if let Some(d) = st.detail.clone() {
            self.render_pull_detail(&d, &st, window, cx).into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.faint)
                .text_size(px(12.5))
                .child(if st.busy {
                    tr("加载中…")
                } else {
                    tr("选择左侧的 PR 查看详情、文件与评审")
                })
                .into_any_element()
        };

        div().flex_1().min_w_0().min_h_0().flex().child(
            gpui_component::resizable::h_resizable("pr-split")
                .with_state(&self.pulls_split.clone())
                .child(
                    gpui_component::resizable::resizable_panel()
                        .size(px(360.))
                        .size_range(px(260.)..px(640.))
                        .child(left),
                )
                .child(gpui_component::resizable::resizable_panel().child(right)),
        )
    }

    fn render_pull_diff(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Reuse the diff viewer on `pull_diff` (swap it in as the commit_diff temporarily would be
        // invasive; instead we render a thin wrapper that delegates).
        let t = self.theme;
        let title = self
            .pull_diff
            .as_ref()
            .map(|d| d.title.clone())
            .unwrap_or_default();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(34.))
            .px(px(10.))
            .border_b_1()
            .border_color(t.line_soft)
            .bg(t.paper)
            .child(
                chrome_button("prd-close", &t, "caret-left", tr("返回（Esc）"), false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.pull_diff = None;
                        cx.notify();
                    },
                )),
            )
            .child(div().font_family(FONT_MONO).text_size(px(12.)).child(title));
        let Some(dv) = self.pull_diff.clone() else {
            return div().into_any_element();
        };
        let body = self.render_diff_rows(false, &dv, cx).into_any_element();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_pull_detail(
        &mut self,
        d: &PullRequest,
        st: &PullState,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.theme;
        let n = d.number;
        let url = d.url.clone();
        let busy = st.busy;
        let checks_color = match d.checks.as_str() {
            "success" => t.cyan_deep,
            "failure" => t.mag_deep,
            "pending" => t.yellow,
            _ => t.faint,
        };
        let mut files = div().flex().flex_col().gap(px(1.));
        for (i, f) in st.files.iter().enumerate() {
            let ch = f.clone();
            let mark = match f.kind {
                ChangeKind::Added => "A",
                ChangeKind::Deleted => "D",
                ChangeKind::Renamed => "R",
                ChangeKind::Copied => "C",
                _ => "M",
            };
            files = files.child(
                div()
                    .id(("prf", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(6.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.ink_05))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_pull_file(ch.clone(), cx)))
                    .child(
                        div()
                            .w(px(12.))
                            .font_family(FONT_MONO)
                            .text_size(px(10.5))
                            .text_color(t.muted)
                            .child(mark),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .child(f.path.clone()),
                    ),
            );
        }
        let act = |id: &'static str, label: &'static str, primary: bool, danger: bool| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(3.))
                .rounded(px(4.))
                .text_size(px(12.))
                .cursor_pointer()
                .when(primary, |x| {
                    x.bg(t.cyan)
                        .text_color(t.surface)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                })
                .when(!primary, |x| {
                    x.border_1()
                        .border_color(if danger { t.mag } else { t.line })
                        .text_color(if danger { t.mag_deep } else { t.ink })
                })
                .hover(move |s| {
                    if primary {
                        s.bg(t.cyan_deep)
                    } else {
                        s.bg(t.ink_05)
                    }
                })
                .when(busy, |x| x.opacity(0.6))
                .child(label)
        };
        let mut comments = div().flex().flex_col().gap(px(6.));
        for c in d
            .comments
            .iter()
            .rev()
            .take(30)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            comments = comments.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(t.ink_05)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(t.ink)
                                    .child(c.author.clone()),
                            )
                            .when(!c.state.is_empty(), |x| {
                                x.child(c.state.to_lowercase().replace('_', " "))
                            })
                            .child(c.at.chars().take(16).collect::<String>()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .whitespace_normal()
                            .child(c.body.clone()),
                    ),
            );
        }
        div()
            .id("pr-detail")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(14.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .font_family(FONT_HEADING)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(16.))
                            .child(format!("#{} {}", d.number, d.title)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .text_size(px(11.5))
                            .text_color(t.muted)
                            .child(d.author.clone())
                            .child(format!("{} → {}", d.head, d.base))
                            .child(div().text_color(t.add_mark).child(format!("+{}", d.additions)))
                            .child(div().text_color(t.del_mark).child(format!("−{}", d.deletions)))
                            .when(!d.checks.is_empty(), |x| {
                                x.child(
                                    div()
                                        .text_color(checks_color)
                                        .child(tf!("检查：{}", d.checks.clone())),
                                )
                            })
                            .when(!d.decision.is_empty(), |x| {
                                x.child(d.decision.to_lowercase().replace('_', " "))
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        act("pr-checkout", tr("Checkout"), false, false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.pull_action(
                                    tf!("checkout #{}", n),
                                    move |f, cwd| sluice_bridge::forge::checkout(f, cwd, n),
                                    cx,
                                )
                            },
                        )),
                    )
                    .child(
                        act("pr-open", tr("在浏览器打开"), false, false)
                            .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                    )
                    .child(div().ml_auto())
                    .child(
                        act("pr-merge", tr("Squash 合并"), false, true).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.overlay = Some(crate::overlays::Overlay::Confirm(
                                    crate::overlays::ConfirmAction::MergePull { number: n },
                                ));
                                cx.notify();
                            },
                        )),
                    ),
            )
            .when(!d.body.trim().is_empty(), |x| {
                x.child(
                    div()
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .bg(t.paper)
                        .border_1()
                        .border_color(t.line_soft)
                        .text_size(px(12.5))
                        .line_height(px(19.))
                        .whitespace_normal()
                        .child(d.body.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(section_label(&t, tf!("文件 · {}", st.files.len())))
                    .child(files),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(section_label(&t, tr("评审")))
                    .child(
                        div()
                            .border_1()
                            .border_color(t.line)
                            .bg(t.surface)
                            .rounded(px(6.))
                            .px(px(9.))
                            .py(px(4.))
                            .text_size(px(12.5))
                            .child(Input::new(&self.pull_comment).appearance(false)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(act("pr-approve", tr("批准"), true, false).on_click(
                                cx.listener(|this, _, _, cx| this.pull_review(ReviewVerdict::Approve, cx)),
                            ))
                            .child(
                                act("pr-changes", tr("请求修改"), false, true).on_click(cx.listener(
                                    |this, _, _, cx| this.pull_review(ReviewVerdict::RequestChanges, cx),
                                )),
                            )
                            .child(act("pr-comment", tr("评论"), false, false).on_click(
                                cx.listener(|this, _, _, cx| this.pull_review(ReviewVerdict::Comment, cx)),
                            ))
                            .child(div().ml_auto())
                            .child(
                                div()
                                    .id("pr-ai")
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .px(px(10.))
                                    .py(px(3.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(t.mag)
                                    .text_color(t.mag_deep)
                                    .text_size(px(12.))
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.mag_soft))
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.pull_ai_review(window, cx)),
                                    )
                                    .child(icon_b("sparkle", px(12.), t.mag))
                                    .child(if st.ai_busy {
                                        tr("AI 预审中…")
                                    } else {
                                        tr("AI 预审（草稿）")
                                    }),
                            ),
                    )
                    .child(div().text_size(px(11.)).text_color(t.faint).child(tr(
                        "评论与审阅经 gh / glab 以你的账号发布；AI 预审只填入草稿，不会自动发布",
                    ))),
            )
            .when(!d.comments.is_empty(), |x| {
                x.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(section_label(&t, tf!("讨论 · {}", d.comments.len())))
                        .child(comments),
                )
            })
    }
}
