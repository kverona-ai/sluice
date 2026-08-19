//! File diff viewer (prototype screens 01/04): toolbar (path, stats, side-by-side /
//! unified, whitespace, context, hunk navigation), virtualized rows with
//! word-level highlights, and — for working-tree diffs — hunk / line checkboxes
//! that drive line-level staging (`git apply --cached`).

use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::{
    Context, HighlightStyle, InteractiveElement, IntoElement, ParentElement, ScrollStrategy,
    StatefulInteractiveElement, Styled, StyledText, UniformListScrollHandle, div, px, uniform_list,
};
use sluice_core::FileChange;
use sluice_core::diff::{FileDiff, LineKind, side_by_side};

use crate::icons::icon_b;
use crate::theme::{FONT_MONO, Theme};
use crate::workbench::{Workbench, checkbox};

pub const DIFF_ROW_H: f32 = 19.;

#[derive(Clone, Debug)]
pub enum DiffRow {
    HunkHeader {
        hunk: usize,
    },
    /// Side-by-side: optional left / right line indices into the hunk's lines.
    Pair {
        hunk: usize,
        left: Option<usize>,
        right: Option<usize>,
    },
    /// Unified: one line.
    Line {
        hunk: usize,
        line: usize,
    },
}

#[derive(Clone)]
pub struct DiffView {
    pub title: String,
    pub change: FileChange,
    pub diff: Option<FileDiff>,
    pub error: Option<String>,
    pub rows_sbs: Vec<DiffRow>,
    pub rows_unified: Vec<DiffRow>,
    pub scroll: UniformListScrollHandle,
    pub current_hunk: usize,
    /// Show staging checkboxes (working-tree, unstaged side).
    pub stageable: bool,
}

impl DiffView {
    pub fn loading(title: String, change: FileChange) -> Self {
        Self {
            title,
            change,
            diff: None,
            error: None,
            rows_sbs: Vec::new(),
            rows_unified: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            current_hunk: 0,
            stageable: false,
        }
    }

    pub fn set_result(&mut self, res: Result<FileDiff, String>) {
        match res {
            Ok(d) => {
                self.rows_sbs = build_rows(&d, true);
                self.rows_unified = build_rows(&d, false);
                self.diff = Some(d);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn rows(&self, sbs: bool) -> &[DiffRow] {
        if sbs { &self.rows_sbs } else { &self.rows_unified }
    }

    pub fn jump_hunk(&mut self, delta: isize) {
        let Some(d) = &self.diff else { return };
        if d.hunks.is_empty() {
            return;
        }
        let n = d.hunks.len() as isize;
        self.current_hunk = ((self.current_hunk as isize + delta).rem_euclid(n)) as usize;
        let target = self.current_hunk;
        if let Some(ix) = self
            .rows_sbs
            .iter()
            .position(|r| matches!(r, DiffRow::HunkHeader { hunk } if *hunk == target))
        {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        }
    }
}

fn build_rows(d: &FileDiff, sbs: bool) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (hi, h) in d.hunks.iter().enumerate() {
        rows.push(DiffRow::HunkHeader { hunk: hi });
        if sbs {
            for r in side_by_side(h) {
                rows.push(DiffRow::Pair {
                    hunk: hi,
                    left: r.left,
                    right: r.right,
                });
            }
        } else {
            for li in 0..h.lines.len() {
                rows.push(DiffRow::Line { hunk: hi, line: li });
            }
        }
    }
    rows
}

fn code_text(
    _t: &Theme,
    text: &str,
    highlights: &[Range<usize>],
    color: gpui::Rgba,
    hl_bg: gpui::Rgba,
) -> impl IntoElement {
    let shown: String = text.replace('\t', "    ");
    if highlights.is_empty() || shown.len() != text.len() {
        return div().text_color(color).child(shown).into_any_element();
    }
    let hls: Vec<(Range<usize>, HighlightStyle)> = highlights
        .iter()
        .filter(|r| r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
        .map(|r| {
            (
                r.clone(),
                HighlightStyle {
                    background_color: Some(hl_bg.into()),
                    ..Default::default()
                },
            )
        })
        .collect();
    div()
        .text_color(color)
        .child(StyledText::new(shown).with_highlights(hls))
        .into_any_element()
}

impl Workbench {
    /// Render a diff view. `work` selects the Changes-tab behaviours (staging checkboxes).
    pub(crate) fn render_diff_view(&mut self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let sbs = self.side_by_side;
        let ctx_n = self.diff_opts.context;
        let ignore_ws = self.diff_opts.ignore_whitespace;
        let dv = if work {
            self.work_diff.clone()
        } else {
            self.commit_diff.clone()
        };
        let Some(dv) = dv else {
            return div().flex_1().into_any_element();
        };
        let stats = dv
            .diff
            .as_ref()
            .map(|d| (d.additions(), d.deletions(), d.hunks.len()))
            .unwrap_or((0, 0, 0));
        let origin = if work {
            if dv.stageable {
                "工作区 vs 暂存区"
            } else {
                "暂存区 vs HEAD"
            }
        } else {
            "提交 vs 父提交"
        };

        let pill = |on: bool, label: &'static str| {
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(12.))
                .border_1()
                .border_color(if on {
                    t.line
                } else {
                    gpui::transparent_black().into()
                })
                .text_color(if on { t.ink } else { t.faint })
                .hover(move |s| s.bg(t.ink_05).text_color(t.ink))
                .child(label)
        };

        let toolbar = div()
            .px(px(12.))
            .py(px(7.))
            .border_b_1()
            .border_color(t.line_soft)
            .flex()
            .items_center()
            .gap(px(12.))
            .text_size(px(12.))
            .text_color(t.muted)
            .flex_none()
            .child(
                div()
                    .id("diff-back")
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_color(t.cyan_deep)
                    .hover(move |s| s.text_color(t.cyan))
                    .on_click(cx.listener(|this, _, _, cx| this.close_diff(cx)))
                    .child(icon_b("caret-left", px(12.), t.cyan_deep))
                    .child(if work { "变更列表" } else { "日志" }),
            )
            .child(
                crate::workbench::chrome_button("hunk-prev", &t, "arrow-line-up", "上一处差异 ⇧F7", false)
                    .w(px(22.))
                    .h(px(22.))
                    .on_click(cx.listener(|this, _, _, cx| this.jump_hunk(-1, cx))),
            )
            .child(
                crate::workbench::chrome_button("hunk-next", &t, "arrow-line-down", "下一处差异 F7", false)
                    .w(px(22.))
                    .h(px(22.))
                    .on_click(cx.listener(|this, _, _, cx| this.jump_hunk(1, cx))),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(11.5))
                    .text_color(t.ink)
                    .child(dv.title.clone()),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(11.))
                    .text_color(t.cyan_deep)
                    .child(format!("+{} −{} · {} 块", stats.0, stats.1, stats.2)),
            )
            .child(
                div()
                    .id("sbs")
                    .cursor_pointer()
                    .flex()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.side_by_side = true;
                        cx.notify();
                    }))
                    .child(pill(sbs, "双栏")),
            )
            .child(
                div()
                    .id("unified")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.side_by_side = false;
                        cx.notify();
                    }))
                    .child(pill(!sbs, "统一")),
            )
            .child(
                div()
                    .id("ws")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_ignore_ws(cx)))
                    .child(pill(ignore_ws, "忽略空白")),
            )
            .child(
                div()
                    .id("ctx")
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next = match ctx_n {
                            0 => 3,
                            3 => 8,
                            8 => 20,
                            _ => 0,
                        };
                        this.set_context(next, cx);
                    }))
                    .child(pill(
                        true,
                        match ctx_n {
                            0 => "上下文 0 行",
                            3 => "上下文 3 行",
                            8 => "上下文 8 行",
                            _ => "上下文 20 行",
                        },
                    )),
            )
            .child(div().ml_auto().text_color(t.faint).child(origin));

        let body: gpui::AnyElement = if let Some(err) = &dv.error {
            div()
                .p(px(16.))
                .text_color(t.mag_deep)
                .child(format!("diff 失败：{err}"))
                .into_any_element()
        } else if let Some(d) = &dv.diff {
            if d.binary {
                div()
                    .p(px(16.))
                    .text_color(t.muted)
                    .child("二进制文件（不显示内容）")
                    .into_any_element()
            } else if d.hunks.is_empty() {
                div()
                    .p(px(16.))
                    .text_color(t.muted)
                    .child("无差异")
                    .into_any_element()
            } else {
                self.render_diff_rows(work, &dv, cx).into_any_element()
            }
        } else {
            div()
                .p(px(16.))
                .text_color(t.muted)
                .child("正在计算 diff…")
                .into_any_element()
        };

        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(body)
            .into_any_element()
    }

    fn render_diff_rows(&mut self, work: bool, dv: &DiffView, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let sbs = self.side_by_side;
        let rows: Vec<DiffRow> = dv.rows(sbs).to_vec();
        let diff = dv.diff.clone().unwrap_or_default();
        let stageable = work && dv.stageable;
        let count = rows.len();
        let scroll = dv.scroll.clone();
        let deselected = self.deselected.clone();

        uniform_list(
            "diff-rows",
            count,
            cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                let mut items = Vec::with_capacity(range.len());
                for ix in range {
                    let Some(row) = rows.get(ix) else { continue };
                    let el = match row {
                        DiffRow::HunkHeader { hunk } => {
                            let h = &diff.hunks[*hunk];
                            let hi = *hunk;
                            let (all_on, some_on) = if stageable {
                                let changed: Vec<usize> = h
                                    .lines
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, l)| l.kind != LineKind::Context)
                                    .map(|(i, _)| i)
                                    .collect();
                                let on = changed
                                    .iter()
                                    .filter(|li| !deselected.contains_key(&(hi, **li)))
                                    .count();
                                (on == changed.len() && !changed.is_empty(), on > 0)
                            } else {
                                (false, false)
                            };
                            div()
                                .id(("hunk", ix))
                                .w_full()
                                .h(px(DIFF_ROW_H + 6.))
                                .flex()
                                .items_center()
                                .gap(px(9.))
                                .px(px(12.))
                                .bg(t.panel)
                                .border_t_1()
                                .border_color(t.line)
                                .font_family(FONT_MONO)
                                .text_size(px(11.5))
                                .text_color(t.cyan_deep)
                                .when(stageable, |d| {
                                    d.cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_hunk(hi, cx)))
                                        .child(checkbox(&t, all_on, some_on && !all_on))
                                })
                                .child(h.header())
                                .child(
                                    div()
                                        .ml_auto()
                                        .font_family(crate::theme::FONT_BODY)
                                        .text_size(px(11.5))
                                        .text_color(t.muted)
                                        .child(format!("+{} −{}", h.additions(), h.deletions())),
                                )
                                .into_any_element()
                        }
                        DiffRow::Pair { hunk, left, right } => {
                            let h = &diff.hunks[*hunk];
                            let hi = *hunk;
                            let l = left.map(|i| &h.lines[i]);
                            let r = right.map(|i| &h.lines[i]);
                            let row_bg = match (l.map(|x| x.kind), r.map(|x| x.kind)) {
                                (Some(LineKind::Context), _) => None,
                                (Some(LineKind::Deleted), None) => Some(t.del_bg),
                                _ => Some(t.add_bg),
                            };
                            let num = |n: Option<u32>| {
                                div()
                                    .w(px(44.))
                                    .flex_none()
                                    .text_right()
                                    .px(px(8.))
                                    .text_size(px(10.5))
                                    .text_color(t.faint)
                                    .child(n.map(|n| n.to_string()).unwrap_or_default())
                            };
                            let left_code = match l {
                                Some(line) => {
                                    let color = match line.kind {
                                        LineKind::Deleted => t.del_mark,
                                        _ => t.ink,
                                    };
                                    code_text(&t, &line.text, &line.highlights, color, t.sel_line)
                                        .into_any_element()
                                }
                                None => div().into_any_element(),
                            };
                            let right_code = match r {
                                Some(line) => {
                                    let color = match line.kind {
                                        LineKind::Added => t.add_mark,
                                        _ => t.ink,
                                    };
                                    code_text(&t, &line.text, &line.highlights, color, t.sel_line)
                                        .into_any_element()
                                }
                                None => div().into_any_element(),
                            };
                            // staging checkbox applies to the changed line(s) in this row
                            let changed_lines: Vec<usize> = [*left, *right]
                                .into_iter()
                                .flatten()
                                .filter(|i| h.lines[*i].kind != LineKind::Context)
                                .collect();
                            let on = !changed_lines.is_empty()
                                && changed_lines
                                    .iter()
                                    .all(|li| !deselected.contains_key(&(hi, *li)));
                            let cl = changed_lines.clone();
                            div()
                                .id(("drow", ix))
                                .w_full()
                                .h(px(DIFF_ROW_H))
                                .flex()
                                .items_center()
                                .font_family(FONT_MONO)
                                .text_size(px(11.5))
                                .line_height(px(DIFF_ROW_H))
                                .when_some(row_bg, |d, bg| d.bg(bg))
                                .child(num(l.and_then(|x| x.old_no)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .px(px(8.))
                                        .child(left_code),
                                )
                                .child(num(r.and_then(|x| x.new_no)))
                                .child(
                                    div()
                                        .id(("dchk", ix))
                                        .w(px(20.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(stageable && !changed_lines.is_empty(), |d| {
                                            d.cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_lines(hi, &cl, cx)
                                                }))
                                                .child(checkbox(&t, on, false))
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .px(px(8.))
                                        .child(right_code),
                                )
                                .into_any_element()
                        }
                        DiffRow::Line { hunk, line } => {
                            let h = &diff.hunks[*hunk];
                            let hi = *hunk;
                            let li = *line;
                            let l = &h.lines[li];
                            let (bg, color, sign) = match l.kind {
                                LineKind::Context => (None, t.ink, " "),
                                LineKind::Added => (Some(t.add_bg), t.add_mark, "+"),
                                LineKind::Deleted => (Some(t.del_bg), t.del_mark, "−"),
                            };
                            let on = !deselected.contains_key(&(hi, li));
                            div()
                                .id(("urow", ix))
                                .w_full()
                                .h(px(DIFF_ROW_H))
                                .flex()
                                .items_center()
                                .font_family(FONT_MONO)
                                .text_size(px(11.5))
                                .line_height(px(DIFF_ROW_H))
                                .when_some(bg, |d, bg| d.bg(bg))
                                .child(
                                    div()
                                        .w(px(44.))
                                        .flex_none()
                                        .text_right()
                                        .px(px(8.))
                                        .text_size(px(10.5))
                                        .text_color(t.faint)
                                        .child(l.old_no.map(|n| n.to_string()).unwrap_or_default()),
                                )
                                .child(
                                    div()
                                        .w(px(44.))
                                        .flex_none()
                                        .text_right()
                                        .px(px(8.))
                                        .text_size(px(10.5))
                                        .text_color(t.faint)
                                        .child(l.new_no.map(|n| n.to_string()).unwrap_or_default()),
                                )
                                .child(
                                    div()
                                        .id(("uchk", ix))
                                        .w(px(20.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(stageable && l.kind != LineKind::Context, |d| {
                                            d.cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_lines(hi, &[li], cx)
                                                }))
                                                .child(checkbox(&t, on, false))
                                        }),
                                )
                                .child(div().w(px(14.)).flex_none().text_color(color).child(sign))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .px(px(4.))
                                        .child(code_text(&t, &l.text, &l.highlights, color, t.sel_line)),
                                )
                                .into_any_element()
                        }
                    };
                    items.push(el);
                }
                items
            }),
        )
        .track_scroll(scroll)
        .flex_1()
        .min_h_0()
    }
}
