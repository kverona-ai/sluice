//! File history + blame (M3). Read-only git commands through the CLI (echoed to
//! the Console); rendered in place of the diff pane in both Log and Changes.

use crate::i18n::tr;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, div, px,
};
use sluice_backend_cli::{BlameLine, FileHistoryEntry};
use sluice_core::Oid;

use crate::icons::icon_b;
use crate::theme::{FONT_MONO, Theme};
use crate::workbench::{Tab, Workbench, chrome_button};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileViewMode {
    History,
    Blame,
}

#[derive(Clone, Debug)]
pub struct FileView {
    pub path: String,
    /// `None` = working tree.
    pub rev: Option<String>,
    pub mode: FileViewMode,
    pub history: Vec<FileHistoryEntry>,
    pub blame: Vec<BlameLine>,
    pub loading: bool,
    pub error: Option<String>,
    pub hover_sha: Option<String>,
}

impl Workbench {
    pub(crate) fn open_file_view(
        &mut self,
        path: String,
        rev: Option<String>,
        mode: FileViewMode,
        cx: &mut Context<Self>,
    ) {
        let Some(cli) = self.repo.cli.clone() else {
            self.toast("裸仓库没有工作区", cx);
            return;
        };
        self.file_view = Some(FileView {
            path: path.clone(),
            rev: rev.clone(),
            mode,
            history: Vec::new(),
            blame: Vec::new(),
            loading: true,
            error: None,
            hover_sha: None,
        });
        cx.notify();
        let p2 = path.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let history = cli.file_history(&p2, 500)?;
                    let blame = cli.blame(&p2, rev.as_deref())?;
                    anyhow::Ok((history, blame))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(fv) = this.file_view.as_mut().filter(|fv| fv.path == path) {
                    fv.loading = false;
                    match res {
                        Ok((history, blame)) => {
                            fv.history = history;
                            fv.blame = blame;
                        }
                        Err(e) => fv.error = Some(format!("{e:#}")),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_file_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let Some(fv) = self.file_view.clone() else {
            return div().into_any_element();
        };
        let mode = fv.mode;
        let mode_pill = |id: &'static str, label: &'static str, on: bool, m: FileViewMode| {
            div()
                .id(id)
                .px(px(8.))
                .py(px(1.))
                .rounded(px(4.))
                .text_size(px(11.5))
                .cursor_pointer()
                .when(on, |d| d.bg(t.cyan_16).text_color(t.cyan_deep))
                .when(!on, |d| d.text_color(t.muted).hover(move |s| s.bg(t.ink_05)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(fv) = this.file_view.as_mut() {
                        fv.mode = m;
                    }
                    cx.notify();
                }))
                .child(label)
        };
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
                chrome_button("fv-close", &t, "caret-left", tr("返回（Esc）"), false)
                    .on_click(cx.listener(|this, _, _, cx| this.close_diff(cx))),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .child(fv.path.clone()),
            )
            .child(div().text_size(px(11.)).text_color(t.faint).child(match &fv.rev {
                Some(r) => format!("@ {}", &r[..r.len().min(8)]),
                None => tr("工作区").to_string(),
            }))
            .child(div().ml_auto())
            .child(mode_pill(
                "fv-history",
                tr("文件历史"),
                mode == FileViewMode::History,
                FileViewMode::History,
            ))
            .child(mode_pill(
                "fv-blame",
                "Blame",
                mode == FileViewMode::Blame,
                FileViewMode::Blame,
            ))
            .child(div().text_size(px(11.)).text_color(t.faint).child(match mode {
                FileViewMode::History => tf!("{} 次提交 · git log --follow", fv.history.len()),
                FileViewMode::Blame => tf!("{} 行 · git blame -w", fv.blame.len()),
            }));

        let body: gpui::AnyElement = if fv.loading {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.faint)
                .text_size(px(12.5))
                .child(tr("加载中…"))
                .into_any_element()
        } else if let Some(err) = &fv.error {
            div()
                .flex_1()
                .p(px(12.))
                .text_color(t.mag_deep)
                .text_size(px(12.5))
                .child(err.clone())
                .into_any_element()
        } else {
            match mode {
                FileViewMode::History => self.render_history_list(&fv, cx).into_any_element(),
                FileViewMode::Blame => self.render_blame(&fv, cx).into_any_element(),
            }
        };
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(t.surface)
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_history_list(&mut self, fv: &FileView, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut list = div()
            .id("fv-history-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(4.));
        for (i, e) in fv.history.iter().enumerate() {
            let when = chrono::DateTime::from_timestamp(e.time, 0)
                .map(|d| {
                    d.with_timezone(&chrono::Local)
                        .format("%Y/%m/%d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            let sha = e.sha.clone();
            let sha2 = e.sha.clone();
            let path = fv.path.clone();
            list = list.child(
                div()
                    .id(("fv-h", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mx(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.ink_05))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tr(
                            "点击：在提交图中选中 · Blame 按钮：查看该版本的 blame",
                        ))
                        .build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.jump_to_commit(&sha, cx)))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(t.cyan_deep)
                            .child(e.sha[..8].to_string()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(e.subject.clone()))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.muted)
                            .child(e.author.clone()),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(when),
                    )
                    .child(
                        div()
                            .id(("fv-h-blame", i))
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .text_size(px(11.))
                            .text_color(t.cyan_deep)
                            .hover(move |s| s.bg(t.cyan_16))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_file_view(
                                    path.clone(),
                                    Some(sha2.clone()),
                                    FileViewMode::Blame,
                                    cx,
                                );
                            }))
                            .child("blame"),
                    ),
            );
        }
        if fv.history.is_empty() {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_color(t.faint)
                    .text_size(px(12.5))
                    .child(tr("没有历史（文件可能尚未提交）")),
            );
        }
        list
    }

    fn render_blame(&mut self, fv: &FileView, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        // Stable per-commit tint: alternate two washes by commit change; hover highlights the whole commit.
        let mut list = div()
            .id("fv-blame-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .font_family(FONT_MONO)
            .text_size(px(11.5));
        let mut prev_sha: Option<&str> = None;
        let mut band = false;
        let hover = fv.hover_sha.clone();
        for (i, l) in fv.blame.iter().enumerate() {
            let new_group = prev_sha != Some(l.sha.as_str());
            if new_group {
                band = !band;
            }
            prev_sha = Some(l.sha.as_str());
            let is_hover = hover.as_deref() == Some(l.sha.as_str());
            let sha = l.sha.clone();
            let sha_jump = l.sha.clone();
            let when = chrono::DateTime::from_timestamp(l.time, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%Y/%m/%d").to_string())
                .unwrap_or_default();
            let gutter_label = if new_group {
                format!("{} {:<10.10} {}", &l.sha[..8], l.author, when)
            } else {
                String::new()
            };
            let summary = l.summary.clone();
            list = list.child(
                div()
                    .id(("bl", i))
                    .flex()
                    .items_center()
                    .h(px(20.))
                    .when(is_hover, |d| d.bg(t.cyan_16))
                    .when(!is_hover && band, |d| d.bg(t.ink_05))
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                        if let Some(fv) = this.file_view.as_mut() {
                            fv.hover_sha = if *on { Some(sha.clone()) } else { None };
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id(("bl-g", i))
                            .w(px(250.))
                            .flex_none()
                            .px(px(8.))
                            .text_color(if new_group { t.muted } else { t.faint })
                            .cursor_pointer()
                            .truncate()
                            .when(new_group, |d| {
                                d.tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(summary.clone()).build(window, cx)
                                })
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.jump_to_commit(&sha_jump, cx)))
                            .child(gutter_label),
                    )
                    .child(
                        div()
                            .w(px(40.))
                            .flex_none()
                            .text_right()
                            .pr(px(8.))
                            .text_color(t.faint)
                            .child(l.line_no.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(l.text.clone()),
                    ),
            );
        }
        if fv.blame.is_empty() {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_color(t.faint)
                    .text_size(px(12.5))
                    .child(tr("没有 blame 数据")),
            );
        }
        list
    }

    /// Select a commit in the Log by full sha (loads more if needed is out of scope: first page only).
    pub(crate) fn jump_to_commit(&mut self, sha: &str, cx: &mut Context<Self>) {
        let Some(log) = self.log.clone() else { return };
        let want = Oid::new(sha);
        if let Some(pos) = log.commits.iter().position(|c| c.id == want) {
            self.tab = Tab::Log;
            self.filter = Default::default();
            self.apply_filter(cx);
            if let Some(vix) = self.visible.iter().position(|&ix| ix == pos) {
                self.select(vix, cx);
                self.scroll.scroll_to_item(vix, gpui::ScrollStrategy::Center);
            }
            self.toast(tf!("已定位到 {}", &sha[..8]), cx);
        } else {
            self.toast("该提交不在当前已加载的提交图中", cx);
        }
        cx.notify();
    }
}

#[allow(dead_code)]
fn _theme_keep(_: &Theme) {
    let _ = icon_b;
}
