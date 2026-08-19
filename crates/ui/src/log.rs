//! Log workspace (prototype screen 01): refs tree · filter bar · commit list
//! with the lane graph · status bar · commit details.

use chrono::{DateTime, Datelike, FixedOffset, Local, Timelike};
use gpui::prelude::FluentBuilder;
use gpui::{
    BorderStyle, Bounds, Context, Corners, Edges, InteractiveElement, IntoElement, ParentElement,
    PathBuilder, Pixels, Point, Rgba, StatefulInteractiveElement, Styled, Window, canvas, div, point, px,
    quad, size, uniform_list,
};
use sluice_core::*;
use sluice_graph::{Edge, RowLayout};

use crate::icons::{icon, icon16};
use crate::theme::{FONT_HEADING, FONT_MONO, Theme};
use crate::workbench::{Workbench, agent_badge, section_label};

pub const ROW_H: f32 = 26.;
const LANE_X0: f32 = 12.;
const LANE_DX: f32 = 15.;

pub fn fmt_date(t: &DateTime<FixedOffset>) -> String {
    let l = t.with_timezone(&Local);
    format!(
        "{}/{}/{} {:02}:{:02}",
        l.year(),
        l.month(),
        l.day(),
        l.hour(),
        l.minute()
    )
}

enum SideRow {
    Head,
    Group(&'static str),
    Remote(String),
    Ref(usize),
}

impl Workbench {
    fn graph_width(&self) -> f32 {
        let lanes = self.store.graph.max_lanes.max(3) as f32;
        (LANE_X0 + (lanes - 1.) * LANE_DX + 20.).max(62.)
    }

    pub(crate) fn render_log(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .child(self.render_refs_sidebar(cx))
            .child(self.render_log_center(cx))
            .child(self.render_details())
    }

    // ----- refs sidebar ----------------------------------------------------

    fn side_rows(&self) -> Vec<SideRow> {
        let mut rows = vec![SideRow::Head, SideRow::Group("Local")];
        for (ix, r) in self.store.refs.iter().enumerate() {
            if r.kind == RefKind::LocalBranch {
                rows.push(SideRow::Ref(ix));
            }
        }
        rows.push(SideRow::Group("Remote"));
        let mut remotes: Vec<String> = self
            .store
            .refs
            .iter()
            .filter_map(|r| match &r.kind {
                RefKind::RemoteBranch { remote } => Some(remote.clone()),
                _ => None,
            })
            .collect();
        remotes.sort();
        remotes.dedup();
        for remote in remotes {
            rows.push(SideRow::Remote(remote.clone()));
            for (ix, r) in self.store.refs.iter().enumerate() {
                if matches!(&r.kind, RefKind::RemoteBranch { remote: rm } if *rm == remote) {
                    rows.push(SideRow::Ref(ix));
                }
            }
        }
        rows.push(SideRow::Group("Tags"));
        for (ix, r) in self.store.refs.iter().enumerate() {
            if r.kind == RefKind::Tag {
                rows.push(SideRow::Ref(ix));
            }
        }
        rows
    }

    fn render_refs_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let rows = self.side_rows();
        let head_branch = self
            .store
            .info
            .head
            .branch
            .clone()
            .unwrap_or_else(|| "detached".into());
        let ahead = self.store.info.head.ahead;
        let selected_ref = self.selected_ref.clone();
        let head_full = self
            .store
            .refs
            .iter()
            .find(|r| r.is_head)
            .map(|r| r.full_name.clone());

        let list = div()
            .id("refs-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(6.))
            .children(rows.into_iter().enumerate().map(|(i, row)| {
                let (label, depth, icon_name, tone, meta, target, selectable, bold) = match row {
                    SideRow::Head => (
                        "HEAD (Current Branch)".to_string(),
                        0usize,
                        "git-commit",
                        t.cyan,
                        head_branch.clone(),
                        head_full.clone(),
                        head_full.is_some(),
                        true,
                    ),
                    SideRow::Group(name) => (
                        name.to_string(),
                        0,
                        "caret-down",
                        t.muted,
                        String::new(),
                        None,
                        false,
                        false,
                    ),
                    SideRow::Remote(name) => (name, 1, "folder", t.muted, String::new(), None, false, false),
                    SideRow::Ref(ix) => {
                        let r = &self.store.refs[ix];
                        let (label, depth, icon_name, tone) = match &r.kind {
                            RefKind::LocalBranch => {
                                let tone = if r.is_head { t.yellow } else { t.muted };
                                let icon_name = if r.is_head { "tag" } else { "git-branch" };
                                (r.short_name.clone(), 1, icon_name, tone)
                            }
                            RefKind::RemoteBranch { remote } => {
                                let short = r
                                    .short_name
                                    .strip_prefix(&format!("{remote}/"))
                                    .unwrap_or(&r.short_name);
                                (short.to_string(), 2, "git-branch", t.muted)
                            }
                            RefKind::Tag => (r.short_name.clone(), 1, "tag", t.muted),
                        };
                        let meta = if r.is_head && ahead > 0 {
                            format!("↑{ahead}")
                        } else {
                            String::new()
                        };
                        (
                            label,
                            depth,
                            icon_name,
                            tone,
                            meta,
                            Some(r.full_name.clone()),
                            true,
                            r.is_head,
                        )
                    }
                };
                let on = selectable && target.is_some() && selected_ref.as_deref() == target.as_deref();
                let target_for_click = target.clone();
                div()
                    .id(("ref", i))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .mx(px(8.))
                    .rounded(px(7.))
                    .pl(px(9. + depth as f32 * 14.))
                    .pr(px(9.))
                    .py(px(4.))
                    .text_size(px(13.))
                    .line_height(px(17.))
                    .when(on, |d| d.bg(t.cyan_16))
                    .when(bold || on, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                    .when(selectable, |d| {
                        d.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                            let next = if this.selected_ref == target_for_click {
                                None
                            } else {
                                target_for_click.clone()
                            };
                            this.pick_ref(next, cx);
                        }))
                    })
                    .child(icon(icon_name, px(13.), tone))
                    .child(div().truncate().child(label))
                    .child(
                        div()
                            .ml_auto()
                            .font_family(FONT_MONO)
                            .text_size(px(10.5))
                            .text_color(t.faint)
                            .child(meta),
                    )
            }));

        div()
            .w(px(246.))
            .flex_none()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(t.ink_05)
            .border_r_1()
            .border_color(t.line_55)
            .child(
                div()
                    .p(px(8.))
                    .border_b_1()
                    .border_color(t.line_soft)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(icon("magnifying-glass", px(14.), t.faint))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(t.faint)
                            .child("Branch or tag"),
                    ),
            )
            .child(list)
    }

    // ----- center: filter bar + list + status ------------------------------

    fn render_filter_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let toggle = |on: bool, label: &'static str| {
            div()
                .font_family(FONT_MONO)
                .text_size(px(11.))
                .px(px(4.))
                .rounded(px(3.))
                .when(on, |d| d.bg(t.cyan).text_color(t.surface))
                .when(!on, |d| d.text_color(t.muted))
                .child(label)
        };
        let chip = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(px(12.))
                .px(px(10.))
                .py(px(4.))
                .rounded(px(7.))
                .text_color(t.muted)
                .bg(t.surface)
                .border_1()
                .border_color(t.line)
                .shadow_sm()
                .child(label)
                .child(icon("caret-down", px(10.), t.muted))
        };
        let ai_only = self.ai_only;
        div()
            .px(px(10.))
            .py(px(7.))
            .border_b_1()
            .border_color(t.line_soft)
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_none()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w(px(210.))
                    .px(px(9.))
                    .py(px(4.))
                    .rounded(px(7.))
                    .bg(t.surface)
                    .border_1()
                    .border_color(t.line)
                    .child(icon("magnifying-glass", px(13.), t.faint))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(t.faint)
                            .child("Text or hash"),
                    )
                    .child(
                        div()
                            .id("rx")
                            .ml_auto()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rx = !this.rx;
                                cx.notify();
                            }))
                            .child(toggle(self.rx, ".*")),
                    )
                    .child(
                        div()
                            .id("cc")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cc = !this.cc;
                                cx.notify();
                            }))
                            .child(toggle(self.cc, "Cc")),
                    ),
            )
            .child(chip("Branch"))
            .child(chip("User"))
            .child(chip("Date"))
            .child(chip("Paths"))
            .child(
                div()
                    .id("ai-only")
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(9.))
                    .py(px(3.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .border_1()
                    .border_color(if ai_only { t.mag } else { t.line })
                    .when(ai_only, |d| d.bg(t.mag_soft).text_color(t.mag_deep))
                    .when(!ai_only, |d| d.text_color(t.muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ai_only = !this.ai_only;
                        cx.notify();
                    }))
                    .child(icon(
                        "sparkle",
                        px(11.),
                        if ai_only { t.mag_deep } else { t.muted },
                    ))
                    .child("仅看 AI 提交"),
            )
            .child(div().ml_auto().child(icon("arrows-down-up", px(14.), t.muted)))
    }

    fn render_commit_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.store.commits.len();
        let graph_w = self.graph_width();
        uniform_list(
            "commits",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let t = this.theme;
                let mut items = Vec::with_capacity(range.len());
                for ix in range {
                    let Some(c) = this.store.commits.get(ix) else {
                        continue;
                    };
                    let selected = ix == this.selected;
                    let is_head = this.store.is_head(&c.id);
                    let dim = this.ai_only && !c.agent.is_ai();
                    let refs = this.store.refs_at(&c.id);
                    let badges = ref_badges(&refs);
                    let row = this.store.graph.rows.get(ix).cloned().unwrap_or_default();
                    let prev_out: Vec<Edge> = if ix > 0 {
                        this.store
                            .graph
                            .rows
                            .get(ix - 1)
                            .map(|r| r.out_edges.clone())
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let lane_color = t.lane(row.color);
                    let theme = t;
                    let graph = canvas(
                        move |_, _, _| (),
                        move |bounds, _, window, _| {
                            paint_graph_row(bounds, &row, &prev_out, is_head, lane_color, &theme, window)
                        },
                    )
                    .w(px(graph_w))
                    .h_full();

                    items.push(
                        div()
                            .id(("commit", ix))
                            .w_full()
                            .h(px(ROW_H))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pr(px(10.))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(t.sel))
                            .when(dim, |d| d.opacity(0.32))
                            .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
                            .child(div().flex_none().w(px(graph_w)).h_full().child(graph))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .when(is_head, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                                    .child(c.summary.clone()),
                            )
                            .children(badges.into_iter().map(|(label, remote, is_tag)| {
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(3.))
                                    .text_size(px(10.5))
                                    .line_height(px(14.))
                                    .px(px(6.))
                                    .py(px(1.))
                                    .whitespace_nowrap()
                                    .border_1()
                                    .border_color(if remote { t.cyan } else { t.yellow })
                                    .text_color(if is_tag { t.muted } else { t.cyan_deep })
                                    .child(icon("tag", px(10.), if is_tag { t.muted } else { t.cyan_deep }))
                                    .child(label)
                            }))
                            .child(agent_badge(&t, c.agent))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(92.))
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(t.muted)
                                    .child(c.author.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(118.))
                                    .font_family(FONT_MONO)
                                    .text_size(px(11.))
                                    .text_color(t.muted)
                                    .text_right()
                                    .child(fmt_date(&c.author.time)),
                            ),
                    );
                }
                items
            }),
        )
        .track_scroll(self.scroll.clone())
        .flex_1()
        .min_h_0()
    }

    fn render_status_bar(&self) -> impl IntoElement {
        let t = self.theme;
        let n = self.store.commits.len();
        let limit = self.store.query.limit;
        let count = if n >= limit {
            format!("前 {n} 条提交（上限 {limit}）· 加载 {}ms", self.store.load_ms)
        } else {
            format!("{n} 条提交 · 加载 {}ms", self.store.load_ms)
        };
        let head = &self.store.info.head;
        let ahead = head.ahead;
        let behind = head.behind;
        let right = match &self.status {
            Some(s) => s.clone(),
            None => match &head.upstream {
                Some(u) => format!("upstream {u} · watcher 待接入（M1）"),
                None => "无 upstream · watcher 待接入（M1）".to_string(),
            },
        };
        div()
            .h(px(25.))
            .flex_none()
            .border_t_1()
            .border_color(t.line_soft)
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(10.))
            .text_size(px(11.5))
            .text_color(t.muted)
            .child(count)
            .child(
                div()
                    .when(ahead > 0, |d| d.text_color(t.cyan))
                    .child(format!("↑{ahead} 未推送")),
            )
            .child(format!("↓{behind} 未拉取"))
            .child(div().ml_auto().truncate().child(right))
    }

    fn render_log_center(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.render_filter_bar(cx))
            .child(self.render_commit_list(cx))
            .child(self.render_status_bar())
    }

    // ----- details ---------------------------------------------------------

    fn render_details(&self) -> impl IntoElement {
        let t = self.theme;
        let mut panel = div()
            .id("details")
            .w(px(356.))
            .flex_none()
            .border_l_1()
            .border_color(t.line)
            .overflow_y_scroll()
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(section_label(&t, "Commit details"));

        let Some(detail) = self.detail.clone() else {
            let msg = self.detail_error.clone().unwrap_or_else(|| "未选择提交".into());
            return panel.child(div().text_size(px(12.5)).text_color(t.muted).child(msg));
        };
        let (d, files) = (&detail.0, &detail.1);
        let c = &d.commit;
        let refs = self.store.refs_at(&c.id);
        let refs_text = if refs.is_empty() {
            "—".to_string()
        } else {
            refs.iter()
                .map(|r| r.short_name.clone())
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let parents = if c.parents.is_empty() {
            "—（根提交）".to_string()
        } else {
            c.parents
                .iter()
                .map(|p| p.short(7).to_string())
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let label = |text: &'static str| div().w(px(62.)).flex_none().text_color(t.faint).child(text);
        let row = |k: &'static str, v: gpui::AnyElement| {
            div()
                .flex()
                .gap(px(10.))
                .text_size(px(12.))
                .line_height(px(17.))
                .child(label(k))
                .child(v)
        };
        let (adds, dels) = files.iter().fold((0u32, 0u32), |(a, d), f| {
            (a + f.additions.unwrap_or(0), d + f.deletions.unwrap_or(0))
        });
        let signature_text = if d.has_signature {
            "已签名（验证待 M2）"
        } else {
            "未签名"
        };
        let trace = if c.agent.is_ai() {
            format!(
                "{} · 依据 Co-authored-by / Sluice-Agent trailer 判定；会话 ID 与确定性关联由 M4 的 bridge 溯源库提供。",
                c.agent.label()
            )
        } else {
            format!("人类提交 —— 作者 {}。未发现 AI 代理 trailer。", c.author.name)
        };

        panel = panel
            .child(
                div()
                    .font_family(FONT_HEADING)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_size(px(15.5))
                    .line_height(px(21.))
                    .child(c.summary.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(row(
                        "Hash",
                        div()
                            .font_family(FONT_MONO)
                            .text_color(t.cyan_deep)
                            .child(c.id.short(12).to_string())
                            .into_any_element(),
                    ))
                    .child(row(
                        "作者",
                        div()
                            .truncate()
                            .child(format!("{} · {}", c.author.name, fmt_date(&c.author.time)))
                            .into_any_element(),
                    ))
                    .child(row(
                        "父提交",
                        div()
                            .font_family(FONT_MONO)
                            .truncate()
                            .child(parents)
                            .into_any_element(),
                    ))
                    .child(row("refs", div().truncate().child(refs_text).into_any_element()))
                    .child(row(
                        "签名",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(icon(
                                "shield-check",
                                px(13.),
                                if d.has_signature { t.cyan } else { t.faint },
                            ))
                            .child(signature_text)
                            .into_any_element(),
                    )),
            );

        // message body (if any beyond the summary)
        let body = d.message.trim_start_matches(&d.commit.summary).trim();
        if !body.is_empty() {
            panel = panel.child(
                div()
                    .border_t_1()
                    .border_color(t.line_soft)
                    .pt(px(10.))
                    .text_size(px(12.5))
                    .line_height(px(19.))
                    .text_color(t.ink)
                    .child(body.to_string()),
            );
        }

        let mut list = div()
            .border_t_1()
            .border_color(t.line_soft)
            .pt(px(10.))
            .flex()
            .flex_col()
            .child(div().pb(px(6.)).child(section_label(
                &t,
                format!("变更文件 · {} 个文件 +{adds} −{dels}", files.len()),
            )));
        for f in files.iter().take(400) {
            let (dir, name) = f.split_dir_name();
            let mark_color = match f.kind {
                ChangeKind::Added => t.cyan,
                ChangeKind::Deleted => t.mag,
                _ => t.muted,
            };
            let stat = match (f.additions, f.deletions, f.binary) {
                (_, _, true) => "bin".to_string(),
                (Some(a), Some(d), _) => format!("+{a} −{d}"),
                _ => String::new(),
            };
            list = list.child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(7.))
                    .py(px(3.))
                    .text_size(px(12.5))
                    .child(
                        div()
                            .w(px(13.))
                            .flex_none()
                            .text_center()
                            .font_family(FONT_MONO)
                            .text_size(px(10.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(mark_color)
                            .child(f.kind.mark()),
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
                    .child(
                        div()
                            .ml_auto()
                            .flex_none()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(stat),
                    ),
            );
        }
        if files.len() > 400 {
            list = list.child(
                div()
                    .text_size(px(11.5))
                    .text_color(t.faint)
                    .child(format!("… 还有 {} 个文件", files.len() - 400)),
            );
        }
        panel = panel.child(list);

        panel.child(
            div()
                .bg(t.mag_soft)
                .border_1()
                .border_color(t.mag_soft)
                .px(px(12.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.mag_deep)
                        .child("会话溯源".to_uppercase()),
                )
                .child(div().text_size(px(12.5)).line_height(px(19.)).child(trace))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.cyan_deep)
                        .child("查看 agent 会话摘要 →（M4）"),
                ),
        )
    }
}

/// Merge `main` + `origin/main` pointing at the same commit into one `origin & main`
/// badge (prototype behaviour). Returns (label, is_remote_ish, is_tag).
fn ref_badges(refs: &[&Ref]) -> Vec<(String, bool, bool)> {
    let mut out: Vec<(String, bool, bool)> = Vec::new();
    let mut used = vec![false; refs.len()];
    for (i, r) in refs.iter().enumerate() {
        if used[i] {
            continue;
        }
        match &r.kind {
            RefKind::LocalBranch => {
                let mut label = r.short_name.clone();
                let mut remote = false;
                for (j, other) in refs.iter().enumerate() {
                    if let RefKind::RemoteBranch { remote: rm } = &other.kind
                        && other.short_name == format!("{rm}/{}", r.short_name)
                    {
                        used[j] = true;
                        label = format!("{rm} & {}", r.short_name);
                        remote = true;
                    }
                }
                out.push((label, remote, false));
            }
            RefKind::RemoteBranch { .. } => out.push((r.short_name.clone(), true, false)),
            RefKind::Tag => out.push((r.short_name.clone(), false, true)),
        }
        used[i] = true;
    }
    out.truncate(3);
    out
}

fn lane_x(bounds: &Bounds<Pixels>, lane: u16) -> Pixels {
    bounds.origin.x + px(LANE_X0 + LANE_DX * lane as f32)
}

fn stroke(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, color: Rgba) {
    let mut pb = PathBuilder::stroke(px(1.6));
    pb.move_to(a);
    pb.line_to(b);
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

/// Paint one row of the lane graph: the upper halves of the edges arriving from
/// the previous row, the lower halves of the edges leaving this row, then the dot.
fn paint_graph_row(
    bounds: Bounds<Pixels>,
    row: &RowLayout,
    prev_out: &[Edge],
    is_head: bool,
    lane_color: Rgba,
    t: &Theme,
    window: &mut Window,
) {
    let top = bounds.origin.y;
    let bottom = top + bounds.size.height;
    let cy = top + bounds.size.height / 2.;
    for e in prev_out {
        let x_from = lane_x(&bounds, e.from_lane);
        let x_to = lane_x(&bounds, e.to_lane);
        let x_mid = (x_from + x_to) / 2.;
        stroke(window, point(x_mid, top), point(x_to, cy), t.lane(e.color));
    }
    for e in &row.out_edges {
        let x_from = lane_x(&bounds, e.from_lane);
        let x_to = lane_x(&bounds, e.to_lane);
        let x_mid = (x_from + x_to) / 2.;
        stroke(window, point(x_from, cy), point(x_mid, bottom), t.lane(e.color));
    }
    let r = px(4.2);
    let center = point(lane_x(&bounds, row.lane), cy);
    let dot = Bounds {
        origin: point(center.x - r, center.y - r),
        size: size(r * 2., r * 2.),
    };
    let fill_color = if is_head { t.paper } else { lane_color };
    window.paint_quad(quad(
        dot,
        Corners::all(r),
        fill_color,
        Edges::all(px(1.8)),
        lane_color,
        BorderStyle::default(),
    ));
}

#[allow(dead_code)]
fn _keep(_: &dyn Fn() -> Rgba) {
    let _ = icon16;
}
