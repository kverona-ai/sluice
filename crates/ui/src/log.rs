//! Log workspace (prototype screen 01): refs tree · filter bar · commit list
//! with the lane graph · status bar · commit details (or a file diff).

use crate::i18n::tr;
use std::sync::Arc;

use chrono::{DateTime, Datelike, FixedOffset, Local, Timelike};
use gpui::AppContext as _;
use gpui::prelude::FluentBuilder;
use gpui::{
    BorderStyle, Bounds, Context, Corners, Edges, InteractiveElement, IntoElement, ParentElement,
    PathBuilder, Pixels, Point, Rgba, StatefulInteractiveElement, Styled, Window, canvas, div, point, px,
    quad, size, uniform_list,
};
use gpui::{Corner, MouseDownEvent, anchored, deferred};
use gpui_component::input::Input;
use sluice_core::*;
use sluice_domain::{DateFilter, LogSnapshot};
use sluice_graph::{Edge, RowLayout};

use crate::icons::{icon_b, icon_f};
use crate::theme::{FONT_HEADING, FONT_MONO, Theme};
use crate::workbench::{Popup, Workbench, agent_badge, section_label};

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
        let lanes = self.log.as_ref().map(|l| l.graph.max_lanes).unwrap_or(1).max(3) as f32;
        (LANE_X0 + (lanes - 1.) * LANE_DX + 20.).max(62.)
    }

    pub(crate) fn render_log(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let center: gpui::AnyElement = if self.file_view.is_some() {
            self.render_file_view(cx).into_any_element()
        } else if self.commit_diff.is_some() {
            self.render_diff_view(false, cx).into_any_element()
        } else {
            self.render_log_center(window, cx).into_any_element()
        };
        let sidebar: Option<gpui::AnyElement> =
            (!self.sidebar_hidden).then(|| self.render_refs_sidebar(cx).into_any_element());
        let details = self.render_details(cx).into_any_element();
        // Dock-style split: every pane is resizable by dragging the handles (sizes persisted).
        let widths = self.settings.log_widths;
        let state = self.log_split.clone();
        let mut group = gpui_component::resizable::h_resizable("log-split")
            .with_state(&state)
            .on_resize(cx.listener(
                |this, st: &gpui::Entity<gpui_component::resizable::ResizableState>, _, cx| {
                    let sizes = st.read(cx).sizes().clone();
                    this.remember_log_widths(&sizes);
                },
            ));
        if let Some(sb) = sidebar {
            group = group.child(
                gpui_component::resizable::resizable_panel()
                    .size(px(widths[0]))
                    .size_range(px(170.)..px(520.))
                    .child(sb),
            );
        }
        group = group
            .child(gpui_component::resizable::resizable_panel().child(center))
            .child(
                gpui_component::resizable::resizable_panel()
                    .size(px(widths[1]))
                    .size_range(px(260.)..px(720.))
                    .child(details),
            );
        div().flex_1().min_w_0().min_h_0().flex().child(group)
    }

    // ----- refs sidebar ----------------------------------------------------

    fn side_rows(&self, log: &LogSnapshot) -> Vec<SideRow> {
        let mut rows = vec![SideRow::Head, SideRow::Group("Local")];
        for (ix, r) in log.refs.iter().enumerate() {
            if r.kind == RefKind::LocalBranch {
                rows.push(SideRow::Ref(ix));
            }
        }
        rows.push(SideRow::Group("Remote"));
        let mut remotes: Vec<String> = log
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
            for (ix, r) in log.refs.iter().enumerate() {
                if matches!(&r.kind, RefKind::RemoteBranch { remote: rm } if *rm == remote) {
                    rows.push(SideRow::Ref(ix));
                }
            }
        }
        rows.push(SideRow::Group("Tags"));
        for (ix, r) in log.refs.iter().enumerate() {
            if r.kind == RefKind::Tag {
                rows.push(SideRow::Ref(ix));
            }
        }
        rows
    }

    fn render_refs_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let log = self.log.clone();
        let head_branch = self
            .repo
            .info
            .head
            .branch
            .clone()
            .unwrap_or_else(|| "detached".into());
        let ahead = self.repo.info.head.ahead;
        let selected_ref = self.selected_ref.clone();

        let mut list = div()
            .id("refs-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(6.));
        if let Some(log) = log {
            let head_full = log.refs.iter().find(|r| r.is_head).map(|r| r.full_name.clone());
            let rows = self.side_rows(&log);
            list = list.children(rows.into_iter().enumerate().map(|(i, row)| {
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
                        let r = &log.refs[ix];
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
                        d.cursor_pointer()
                            .hover(move |s| s.bg(if on { t.cyan_16 } else { t.ink_08 }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let next = if this.selected_ref == target_for_click {
                                    None
                                } else {
                                    target_for_click.clone()
                                };
                                this.pick_ref(next, cx);
                            }))
                    })
                    .child(match icon_name {
                        "tag" | "star" | "folder" => icon_f(icon_name, px(14.), tone),
                        "caret-down" => icon_b(icon_name, px(12.), tone),
                        _ => icon_b(icon_name, px(14.), tone),
                    })
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
        }

        div()
            .size_full()
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
                    .child(icon_b("magnifying-glass", px(14.), t.faint))
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

    fn render_filter_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let search_focused = gpui::Focusable::focus_handle(self.search.read(cx), cx).is_focused(window);
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
        let chip = |on: bool, label: String| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(px(12.))
                .px(px(10.))
                .py(px(4.))
                .rounded(px(7.))
                .text_color(if on { t.cyan_deep } else { t.muted })
                .bg(t.surface)
                .border_1()
                .border_color(if on { t.cyan } else { t.line })
                .shadow_sm()
                .hover(move |s| s.border_color(t.cyan).text_color(t.cyan_deep))
                .child(label)
                .child(icon_b(
                    "caret-down",
                    px(10.),
                    if on { t.cyan_deep } else { t.muted },
                ))
        };
        let ai_only = self.filter.ai_only;
        let users_on = !self.filter.authors.is_empty();
        let users_label = if users_on {
            format!("User · {}", self.filter.authors.len())
        } else {
            "User".to_string()
        };
        let date_on = self.filter.date != DateFilter::Any;
        let date_label = self.filter.date.label().to_string();
        let branch_label = match &self.selected_ref {
            Some(r) => format!("Branch · {}", r.rsplit('/').next().unwrap_or(r)),
            None => "Branch".to_string(),
        };
        let branch_on = self.selected_ref.is_some();

        div()
            .relative()
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
                    .min_w(px(230.))
                    .px(px(9.))
                    .py(px(2.))
                    .rounded(px(7.))
                    .bg(t.surface)
                    .border_1()
                    .border_color(if search_focused { t.cyan } else { t.line })
                    .child(icon_b(
                        "magnifying-glass",
                        px(13.),
                        if search_focused { t.cyan } else { t.faint },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.5))
                            .child(Input::new(&self.search).appearance(false)),
                    )
                    .child(
                        div()
                            .id("rx")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.filter.regex = !this.filter.regex;
                                this.apply_filter(cx);
                            }))
                            .child(toggle(self.filter.regex, ".*")),
                    )
                    .child(
                        div()
                            .id("cc")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.filter.case_sensitive = !this.filter.case_sensitive;
                                this.apply_filter(cx);
                            }))
                            .child(toggle(self.filter.case_sensitive, "Cc")),
                    ),
            )
            .child(
                div()
                    .id("f-branch")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.selected_ref.is_some() {
                            this.pick_ref(None, cx);
                        } else {
                            this.toast("在左侧 refs 树中点击分支 / 标签即可按其过滤", cx);
                        }
                    }))
                    .child(chip(branch_on, branch_label)),
            )
            .child(
                div()
                    .id("f-user")
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            this.popup = match this.popup {
                                Some((Popup::Users, _)) => None,
                                _ => Some((Popup::Users, ev.position)),
                            };
                            cx.notify();
                        }),
                    )
                    .child(chip(users_on, users_label)),
            )
            .child(
                div()
                    .id("f-date")
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            this.popup = match this.popup {
                                Some((Popup::Date, _)) => None,
                                _ => Some((Popup::Date, ev.position)),
                            };
                            cx.notify();
                        }),
                    )
                    .child(chip(date_on, date_label)),
            )
            .child(
                div()
                    .id("f-paths")
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                            this.popup = match this.popup {
                                Some((Popup::Paths, _)) => None,
                                _ => Some((Popup::Paths, ev.position)),
                            };
                            cx.notify();
                        }),
                    )
                    .child(chip(
                        !self.filter.paths.is_empty(),
                        if self.filter.paths.is_empty() {
                            "Paths".into()
                        } else {
                            format!("Paths · {}", self.filter.paths.len())
                        },
                    )),
            )
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
                        this.filter.ai_only = !this.filter.ai_only;
                        this.apply_filter(cx);
                    }))
                    .hover(move |s| s.border_color(t.mag))
                    .child(icon_b(
                        "sparkle",
                        px(11.),
                        if ai_only { t.mag_deep } else { t.muted },
                    ))
                    .child(tr("仅看 AI 提交")),
            )
            .child(
                div()
                    .id("order")
                    .ml_auto()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.query.order = match this.query.order {
                            LogOrder::DateOrder => LogOrder::TopoOrder,
                            LogOrder::TopoOrder => LogOrder::DateOrder,
                        };
                        let label = match this.query.order {
                            LogOrder::DateOrder => tr("时间序（保持拓扑约束）"),
                            LogOrder::TopoOrder => tr("拓扑序"),
                        };
                        this.toast(tf!("排序：{}", label), cx);
                        this.reload_log(cx);
                    }))
                    .child(icon_b(
                        "arrows-down-up",
                        px(14.),
                        if self.query.order == LogOrder::TopoOrder {
                            t.cyan
                        } else {
                            t.muted
                        },
                    )),
            )
    }

    /// User / Date filter dropdowns. Painted through `deferred(anchored(..))` so the
    /// menu always draws above the commit list and clamps to the window.
    pub(crate) fn render_popup(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = self.theme;
        let (popup, at) = self.popup?;
        let mut panel = div()
            .id("filter-popup")
            .occlude()
            .w(px(252.))
            .max_h(px(360.))
            .overflow_y_scroll()
            .bg(t.surface)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .py(px(4.))
            .text_size(px(12.5))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.popup = None;
                cx.notify();
            }));
        match popup {
            Popup::Users => {
                let mut counts: std::collections::HashMap<String, (usize, bool)> =
                    std::collections::HashMap::new();
                if let Some(log) = self.log.as_ref() {
                    for c in &log.commits {
                        let e = counts.entry(c.author.name.clone()).or_insert((0, false));
                        e.0 += 1;
                        e.1 |= c.agent.is_ai();
                    }
                }
                let authors = self.log.as_ref().map(|l| l.authors.clone()).unwrap_or_default();
                let selected = self.filter.authors.clone();
                let any = !selected.is_empty();
                panel = panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(12.))
                            .py(px(5.))
                            .border_b_1()
                            .border_color(t.line_soft)
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(t.faint)
                                    .child(tf!("按作者过滤 · {} 人 · 可多选", authors.len())),
                            )
                            .child(
                                div()
                                    .id("users-clear")
                                    .ml_auto()
                                    .px(px(7.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .text_size(px(11.5))
                                    .cursor_pointer()
                                    .text_color(if any { t.cyan_deep } else { t.faint })
                                    .hover(move |st| st.bg(t.cyan_16))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.filter.authors.clear();
                                        this.apply_filter(cx);
                                    }))
                                    .child(tr("清除")),
                            ),
                    )
                    .children(authors.into_iter().enumerate().map(|(i, name)| {
                        let on = selected.contains(&name);
                        let (n_commits, is_ai) = counts.get(&name).copied().unwrap_or((0, false));
                        let initial: String = name.chars().next().map(|c| c.to_string()).unwrap_or_default();
                        let dot_color = t.lane((name.len() as u16) % 3);
                        let n2 = name.clone();
                        div()
                            .id(("user", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .mx(px(4.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(on, |d| d.bg(t.sel))
                            .hover(move |st| st.bg(if on { t.sel } else { t.ink_05 }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.filter.authors.remove(&n2) {
                                    this.filter.authors.insert(n2.clone());
                                }
                                this.apply_filter(cx);
                            }))
                            .child(crate::workbench::checkbox(&t, on, false))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(18.))
                                    .h(px(18.))
                                    .rounded(px(9.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .border_1()
                                    .border_color(dot_color)
                                    .text_color(dot_color)
                                    .text_size(px(10.))
                                    .child(initial),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .when(is_ai, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(9.5))
                                        .px(px(4.))
                                        .border_1()
                                        .border_color(t.mag)
                                        .text_color(t.mag_deep)
                                        .rounded(px(4.))
                                        .child("AI"),
                                )
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.5))
                                    .text_color(t.faint)
                                    .child(format!("{n_commits}")),
                            )
                    }));
            }
            Popup::Paths => {
                let paths = self.filter.paths.clone();
                panel = panel
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(5.))
                            .border_b_1()
                            .border_color(t.line_soft)
                            .child(
                                div().text_size(px(11.)).text_color(t.faint).child(tr(
                                    "按路径过滤 · 输入路径或前缀，Enter 添加（git log -- <path>）",
                                )),
                            ),
                    )
                    .child(
                        div().px(px(10.)).py(px(6.)).child(
                            div()
                                .border_1()
                                .border_color(t.cyan)
                                .bg(t.surface)
                                .rounded(px(6.))
                                .px(px(8.))
                                .py(px(2.))
                                .text_size(px(12.))
                                .child(
                                    gpui_component::input::Input::new(&self.paths_input).appearance(false),
                                ),
                        ),
                    )
                    .children(paths.iter().enumerate().map(|(i, p)| {
                        let p2 = p.clone();
                        div()
                            .id(("path-row", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .mx(px(4.))
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(6.))
                            .font_family(FONT_MONO)
                            .text_size(px(11.5))
                            .hover(move |st| st.bg(t.ink_05))
                            .child(div().flex_1().min_w_0().truncate().child(p.clone()))
                            .child(
                                div()
                                    .id(("path-x", i))
                                    .cursor_pointer()
                                    .text_color(t.faint)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.filter.paths.retain(|x| x != &p2);
                                        this.recompute_path_filter(cx);
                                    }))
                                    .child("×"),
                            )
                    }))
                    .when(!paths.is_empty(), |d| {
                        d.child(
                            div()
                                .id("paths-clear")
                                .mx(px(8.))
                                .my(px(4.))
                                .px(px(8.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .text_size(px(11.5))
                                .text_color(t.cyan_deep)
                                .cursor_pointer()
                                .hover(move |st| st.bg(t.cyan_16))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.filter.paths.clear();
                                    this.recompute_path_filter(cx);
                                }))
                                .child(tr("清除全部")),
                        )
                    });
            }
            Popup::Messages => {
                let msgs = crate::recent::load_messages();
                panel = panel.child(
                    div()
                        .px(px(12.))
                        .py(px(5.))
                        .border_b_1()
                        .border_color(t.line_soft)
                        .child(div().text_size(px(11.)).text_color(t.faint).child(format!(
                            "{} · {}",
                            tr("最近提交信息"),
                            msgs.len()
                        ))),
                );
                if msgs.is_empty() {
                    panel = panel.child(
                        div()
                            .px(px(12.))
                            .py(px(10.))
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(tr("还没有记录；成功提交后会出现在这里")),
                    );
                }
                panel = panel.children(msgs.into_iter().enumerate().map(|(i, m)| {
                    let first = m.lines().next().unwrap_or("").to_string();
                    let more = m.lines().count() > 1;
                    let full = m.clone();
                    div()
                        .id(("msg-hist", i))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .mx(px(4.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |st| st.bg(t.ink_05))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let text = full.clone();
                            this.commit_msg.update(cx, |s, cx| s.set_value(text, window, cx));
                            this.popup = None;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .child(first),
                        )
                        .when(more, |d| {
                            d.child(div().text_size(px(10.5)).text_color(t.faint).child("…"))
                        })
                }));
            }
            Popup::Date => {
                let current = self.filter.date;
                panel = panel
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(5.))
                            .border_b_1()
                            .border_color(t.line_soft)
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(t.faint)
                                    .child(tr("按时间过滤")),
                            ),
                    )
                    .children(DateFilter::ALL.into_iter().enumerate().map(|(i, d)| {
                        let on = d == current;
                        div()
                            .id(("date", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .mx(px(4.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(on, |x| x.bg(t.sel))
                            .hover(move |st| st.bg(if on { t.sel } else { t.ink_05 }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.filter.date = d;
                                this.popup = None;
                                this.apply_filter(cx);
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(12.))
                                    .h(px(12.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(if on { t.cyan } else { t.line })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(div().w(px(6.)).h(px(6.)).rounded(px(3.)).bg(if on {
                                        t.cyan
                                    } else {
                                        gpui::transparent_black().into()
                                    })),
                            )
                            .child(if d == DateFilter::Any {
                                tr("全部时间").to_string()
                            } else {
                                d.label().to_string()
                            })
                    }));
            }
        }
        Some(
            deferred(
                anchored()
                    .position(at + gpui::point(px(-8.), px(14.)))
                    .anchor(Corner::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(panel),
            )
            .with_priority(2),
        )
    }

    fn render_commit_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let graph_w = self.graph_width();
        let Some(log) = self.log.clone() else {
            let msg = if self.log_loading {
                tr("正在读取仓库…").to_string()
            } else {
                self.log_error.clone().unwrap_or_else(|| tr("无提交").into())
            };
            return div()
                .flex_1()
                .p(px(16.))
                .text_color(t.muted)
                .child(msg)
                .into_any_element();
        };
        let filtered = self.filter.is_active();
        let visible: Arc<Vec<usize>> = Arc::new(self.visible.clone());
        let count = visible.len();
        uniform_list(
            "commits",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let mut items = Vec::with_capacity(range.len());
                for vix in range {
                    let Some(&ix) = visible.get(vix) else { continue };
                    let Some(c) = log.commits.get(ix) else { continue };
                    let selected = vix == this.selected;
                    let is_head = log.is_head(&c.id);
                    let refs = log.refs_at(&c.id);
                    let badges = ref_badges(&refs);
                    let row = log.graph.rows.get(ix).cloned().unwrap_or_default();
                    let prev_out: Vec<Edge> = if ix > 0 && !filtered {
                        log.graph
                            .rows
                            .get(ix - 1)
                            .map(|r| r.out_edges.clone())
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let row_for_paint = if filtered {
                        RowLayout {
                            out_edges: Vec::new(),
                            ..row.clone()
                        }
                    } else {
                        row.clone()
                    };
                    let lane_color = t.lane(row.color);
                    let theme = t;
                    let graph = canvas(
                        move |_, _, _| (),
                        move |bounds, _, window, _| {
                            paint_graph_row(
                                bounds,
                                &row_for_paint,
                                &prev_out,
                                is_head,
                                lane_color,
                                &theme,
                                window,
                            )
                        },
                    )
                    .w(px(graph_w))
                    .h_full();

                    items.push(
                        div()
                            .id(("commit", vix))
                            .on_mouse_down(
                                gpui::MouseButton::Right,
                                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                    this.select(vix, cx);
                                    this.show_ctx_menu(ev, crate::overlays::CtxTarget::Commit(ix), cx)
                                }),
                            )
                            .w_full()
                            .h(px(ROW_H))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pr(px(10.))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(t.sel))
                            .when(!selected, |d| d.hover(move |s| s.bg(t.ink_05)))
                            .on_click(cx.listener(move |this, _, _, cx| this.select(vix, cx)))
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
                                    .child(icon_f("tag", px(10.), if is_tag { t.muted } else { t.cyan_deep }))
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
        .into_any_element()
    }

    fn render_status_bar(&self) -> impl IntoElement {
        let t = self.theme;
        let (n_all, load_ms, limit) = self
            .log
            .as_ref()
            .map(|l| (l.commits.len(), l.load_ms, l.query.limit))
            .unwrap_or((0, 0, 0));
        let shown = self.visible.len();
        let mut count = if n_all >= limit && limit > 0 {
            tf!("前 {} 条提交（上限 {}）· 加载 {}ms", n_all, limit, load_ms)
        } else {
            tf!("{} 条提交 · 加载 {}ms", n_all, load_ms)
        };
        if self.filter.is_active() {
            count = format!("{shown} / {count}");
        }
        if self.log_loading {
            count.push_str(tr(" · 刷新中…"));
        }
        let head = &self.repo.info.head;
        let ahead = head.ahead;
        let behind = head.behind;
        let right = match &head.upstream {
            Some(u) => tf!("upstream {} · watcher 活跃", u),
            None => tr("无 upstream · watcher 活跃").to_string(),
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
                    .child(tf!("↑{} 未推送", ahead)),
            )
            .child(tf!("↓{} 未拉取", behind))
            .child(div().ml_auto().truncate().child(right))
    }

    fn render_log_center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.render_filter_bar(window, cx))
            .child(self.render_commit_list(cx))
            .child(self.render_status_bar())
    }

    // ----- details ---------------------------------------------------------

    fn render_details(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut panel = div()
            .id("details")
            .size_full()
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
            let msg = if self.selected_commit().is_some() {
                tr("读取中…")
            } else {
                tr("未选择提交")
            };
            return panel.child(div().text_size(px(12.5)).text_color(t.muted).child(msg));
        };
        let d = &detail.detail;
        let detail_sha = d.commit.id.clone();
        let files = &detail.changes;
        let c = &d.commit;
        let refs_text = self
            .log
            .as_ref()
            .map(|l| {
                l.refs_at(&c.id)
                    .iter()
                    .map(|r| r.short_name.clone())
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "—".to_string());
        let parents = if c.parents.is_empty() {
            tr("—（根提交）").to_string()
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
            tr("已签名（验证待 M2）")
        } else {
            tr("未签名")
        };
        // Session provenance: hook events (sluice hook <tool>) touching this commit's
        // files within the 12 h before the commit.
        let commit_at = c.author.time.timestamp();
        let commit_files: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let matches = self.provenance_for(&c.id, &commit_files, commit_at);
        let trace = if c.agent.is_ai() {
            tf!(
                "{} · 依据 Co-authored-by / Sluice-Agent trailer 判定。",
                c.agent.label()
            )
        } else if matches.is_empty() {
            tf!(
                "人类提交 —— 作者 {}。未发现 AI 代理 trailer，也没有匹配的 AI 会话记录。",
                c.author.name
            )
        } else {
            tf!(
                "作者 {}；trailer 未标注 AI，但以下 AI 会话在提交前修改过这些文件：",
                c.author.name
            )
        };
        let open_path = self.commit_diff.as_ref().map(|dv| dv.change.path.clone());

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
                    .children(self.jj_change_row(&c.id, &t, cx))
                    .child(row("Hash", {
                        let full = c.id.to_string();
                        div()
                            .id("hash-copy")
                            .font_family(FONT_MONO)
                            .text_color(t.cyan_deep)
                            .cursor_pointer()
                            .hover(move |s| s.text_color(t.cyan))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(full.clone()));
                                this.toast(tf!("已复制 {}", &full[..full.len().min(12)]), cx);
                            }))
                            .child(format!("{}  ⎘", c.id.short(12)))
                            .into_any_element()
                    }))
                    .child(row(
                        tr("作者"),
                        div()
                            .truncate()
                            .child(format!("{} · {}", c.author.name, fmt_date(&c.author.time)))
                            .into_any_element(),
                    ))
                    .child(row(
                        tr("父提交"),
                        div()
                            .font_family(FONT_MONO)
                            .truncate()
                            .child(parents)
                            .into_any_element(),
                    ))
                    .child(row("refs", div().truncate().child(refs_text).into_any_element()))
                    .child(row(
                        tr("签名"),
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(icon_b(
                                "shield-check",
                                px(13.),
                                if d.has_signature { t.cyan } else { t.faint },
                            ))
                            .child(signature_text)
                            .into_any_element(),
                    )),
            );

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
                tf!("变更文件 · {} 个文件 +{} −{}", files.len(), adds, dels),
            )));
        for (i, f) in files.iter().take(400).enumerate() {
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
            let is_open = open_path.as_deref() == Some(f.path.as_str());
            let change = f.clone();
            let ctx_path = f.path.clone();
            let ctx_sha = detail_sha.clone();
            list = list.child(
                div()
                    .id(("file", i))
                    .flex()
                    .items_baseline()
                    .gap(px(7.))
                    .py(px(3.))
                    .px(px(4.))
                    .mx(px(-4.))
                    .rounded(px(4.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .when(is_open, |d| d.bg(t.sel))
                    .when(!is_open, |d| d.hover(move |s| s.bg(t.ink_05)))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_commit_file(change.clone(), cx)))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            this.show_ctx_menu(
                                ev,
                                crate::overlays::CtxTarget::DetailFile {
                                    path: ctx_path.clone(),
                                    sha: ctx_sha.clone(),
                                },
                                cx,
                            )
                        }),
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
                    .child(tf!("… 还有 {} 个文件", files.len() - 400)),
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
                        .child(tr("会话溯源").to_uppercase()),
                )
                .child(div().text_size(px(12.5)).line_height(px(19.)).child(trace))
                .children(matches.iter().take(6).map(|m| {
                    let when = chrono::DateTime::from_timestamp(m.last_at, 0)
                        .map(|d| d.with_timezone(&chrono::Local).format("%m/%d %H:%M").to_string())
                        .unwrap_or_default();
                    let sid: String = m.session_id.chars().take(8).collect();
                    let agent = sluice_core::Agent::detect(&format!("Sluice-Agent: {}", m.tool), "", "");
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.5))
                        .child(crate::workbench::agent_badge(&t, agent))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(m.tool.clone()),
                        )
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_color(t.faint)
                                .child(tf!("会话 {}", sid)),
                        )
                        .child(div().text_color(t.muted).child(tf!(
                            "{} 次改动 · {} 个文件 · {}",
                            m.events,
                            m.files.len(),
                            when
                        )))
                }))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.faint)
                        .child(if matches.is_empty() {
                            tr("溯源数据来自各 AI CLI 的 hooks（AI 接入面板一键安装）").to_string()
                        } else {
                            tf!(
                                "匹配规则：提交前 12 小时内、触及相同路径的 hook 事件，共 {} 个会话",
                                matches.len()
                            )
                        }),
                ),
        )
    }

    /// Cached provenance matches for the commit shown in the details pane.
    fn provenance_for(
        &mut self,
        id: &Oid,
        files: &[String],
        commit_at: i64,
    ) -> Vec<sluice_bridge::provenance::SessionMatch> {
        if let Some((cached_id, m)) = &self.provenance_cache
            && cached_id == id
        {
            return m.clone();
        }
        let root = self.repo.cli.as_ref().map(|c| c.workdir().to_path_buf());
        let events = root
            .map(|r| sluice_bridge::provenance::load(&r))
            .unwrap_or_default();
        let m = sluice_bridge::provenance::matches_for_commit(&events, files, commit_at, 12 * 3600);
        self.provenance_cache = Some((id.clone(), m.clone()));
        m
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

impl Workbench {
    /// jujutsu: show the change id next to the commit id (fetched lazily per commit).
    fn jj_change_row(&mut self, id: &Oid, t: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let jj = self.repo.jj.clone()?;
        let value = match self.jj_change_ids.get(id) {
            Some(v) => v.clone(),
            None => {
                self.jj_change_ids.insert(id.clone(), "…".into());
                let sha = id.to_string();
                let key = id.clone();
                cx.spawn(async move |this, cx| {
                    let res = cx.background_spawn(async move { jj.change_id_of(&sha) }).await;
                    this.update(cx, |this, cx| {
                        this.jj_change_ids.insert(key, res.unwrap_or_else(|_| "—".into()));
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
                "…".into()
            }
        };
        Some(
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .text_size(px(12.))
                .child(div().w(px(56.)).flex_none().text_color(t.faint).child("Change"))
                .child(div().font_family(FONT_MONO).text_color(t.mag_deep).child(value)),
        )
    }
}
