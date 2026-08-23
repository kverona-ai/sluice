//! Three-way conflict resolver (M3, 05 §6). Parses conflict markers from the
//! working-tree file (incl. diff3 `|||||||` base), lets the user take ours /
//! theirs / both per block, writes the file back and marks it resolved
//! (`git add`). Whole-file shortcuts use `git checkout --ours/--theirs`.

use crate::i18n::tr;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, div, px,
};

use crate::icons::icon_b;
use crate::theme::FONT_MONO;
use crate::workbench::{Workbench, chrome_button};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Unresolved,
    Ours,
    Theirs,
    Both,
    BothReversed,
}

#[derive(Clone, Debug)]
pub enum Seg {
    Text(Vec<String>),
    Conflict {
        ours: Vec<String>,
        theirs: Vec<String>,
        base: Option<Vec<String>>,
        label_ours: String,
        label_theirs: String,
        choice: Choice,
    },
}

#[derive(Clone, Debug)]
pub struct ConflictView {
    pub path: String,
    pub segments: Vec<Seg>,
    pub loading: bool,
    pub error: Option<String>,
    pub dirty: bool,
}

impl ConflictView {
    pub fn parse(path: &str, text: &str) -> Self {
        let mut segments = Vec::new();
        let mut plain: Vec<String> = Vec::new();
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next() {
            if let Some(label) = line.strip_prefix("<<<<<<< ") {
                if !plain.is_empty() {
                    segments.push(Seg::Text(std::mem::take(&mut plain)));
                }
                let mut ours = Vec::new();
                let mut base: Option<Vec<String>> = None;
                let mut theirs = Vec::new();
                let mut label_theirs = String::new();
                let mut state = 0; // 0 ours, 1 base, 2 theirs
                for l in lines.by_ref() {
                    if l.starts_with("|||||||") && state == 0 {
                        state = 1;
                        base = Some(Vec::new());
                        continue;
                    }
                    if l == "=======" && state < 2 {
                        state = 2;
                        continue;
                    }
                    if let Some(lt) = l.strip_prefix(">>>>>>> ") {
                        label_theirs = lt.to_string();
                        break;
                    }
                    match state {
                        0 => ours.push(l.to_string()),
                        1 => base.as_mut().unwrap().push(l.to_string()),
                        _ => theirs.push(l.to_string()),
                    }
                }
                segments.push(Seg::Conflict {
                    ours,
                    theirs,
                    base,
                    label_ours: label.to_string(),
                    label_theirs,
                    choice: Choice::Unresolved,
                });
            } else {
                plain.push(line.to_string());
            }
        }
        if !plain.is_empty() {
            segments.push(Seg::Text(plain));
        }
        Self {
            path: path.to_string(),
            segments,
            loading: false,
            error: None,
            dirty: false,
        }
    }

    pub fn unresolved(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| {
                matches!(
                    s,
                    Seg::Conflict {
                        choice: Choice::Unresolved,
                        ..
                    }
                )
            })
            .count()
    }

    pub fn conflicts(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| matches!(s, Seg::Conflict { .. }))
            .count()
    }

    /// Serialize: resolved blocks emit the chosen text; unresolved keep their markers.
    pub fn render_text(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        for s in &self.segments {
            match s {
                Seg::Text(lines) => out.extend(lines.iter().cloned()),
                Seg::Conflict {
                    ours,
                    theirs,
                    base,
                    label_ours,
                    label_theirs,
                    choice,
                } => match choice {
                    Choice::Ours => out.extend(ours.iter().cloned()),
                    Choice::Theirs => out.extend(theirs.iter().cloned()),
                    Choice::Both => {
                        out.extend(ours.iter().cloned());
                        out.extend(theirs.iter().cloned());
                    }
                    Choice::BothReversed => {
                        out.extend(theirs.iter().cloned());
                        out.extend(ours.iter().cloned());
                    }
                    Choice::Unresolved => {
                        out.push(format!("<<<<<<< {label_ours}"));
                        out.extend(ours.iter().cloned());
                        if let Some(b) = base {
                            out.push("||||||| base".into());
                            out.extend(b.iter().cloned());
                        }
                        out.push("=======".into());
                        out.extend(theirs.iter().cloned());
                        out.push(format!(">>>>>>> {label_theirs}"));
                    }
                },
            }
        }
        out.join("\n") + "\n"
    }
}

impl Workbench {
    pub(crate) fn open_conflict(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else { return };
        self.conflict = Some(ConflictView {
            path: path.clone(),
            segments: Vec::new(),
            loading: true,
            error: None,
            dirty: false,
        });
        self.work_diff = None;
        cx.notify();
        let p2 = path.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let full = cli.workdir().join(&p2);
                    std::fs::read_to_string(&full).map_err(|e| anyhow::anyhow!("{e}"))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(cv) = this.conflict.as_mut().filter(|c| c.path == path) {
                    match res {
                        Ok(text) => *cv = ConflictView::parse(&path, &text),
                        Err(e) => {
                            cv.loading = false;
                            cv.error = Some(format!("{e:#}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn conflict_choose(&mut self, ix: usize, choice: Choice, cx: &mut Context<Self>) {
        if let Some(cv) = self.conflict.as_mut() {
            let mut n = 0;
            for s in cv.segments.iter_mut() {
                if let Seg::Conflict { choice: c, .. } = s {
                    if n == ix {
                        *c = choice;
                        cv.dirty = true;
                        break;
                    }
                    n += 1;
                }
            }
            cx.notify();
        }
    }

    /// Keyboard: act on the first unresolved block.
    pub(crate) fn conflict_choose_current(&mut self, choice: Choice, cx: &mut Context<Self>) {
        let Some(cv) = self.conflict.as_ref() else { return };
        let mut n = 0;
        for s in &cv.segments {
            if let Seg::Conflict { choice: c, .. } = s {
                if *c == Choice::Unresolved {
                    self.conflict_choose(n, choice, cx);
                    return;
                }
                n += 1;
            }
        }
        self.toast("没有未解决的冲突块", cx);
    }

    pub(crate) fn conflict_save(&mut self, mark_resolved: bool, cx: &mut Context<Self>) {
        let Some(cv) = self.conflict.clone() else { return };
        if mark_resolved && cv.unresolved() > 0 {
            self.toast(tf!("还有 {} 个冲突块未解决", cv.unresolved()), cx);
            return;
        }
        let text = cv.render_text();
        let path = cv.path.clone();
        let what = if mark_resolved {
            tf!("解决冲突 {}", path)
        } else {
            tf!("保存 {}", path)
        };
        if mark_resolved {
            self.conflict = None;
            self.work_diff = None;
        } else if let Some(c) = self.conflict.as_mut() {
            c.dirty = false;
        }
        self.run_git(
            what,
            move |cli| {
                std::fs::write(cli.workdir().join(&path), text)?;
                if mark_resolved {
                    cli.stage(&[&path])?;
                    Ok(tr("已标记为已解决").into())
                } else {
                    Ok(tr("已写入工作区").into())
                }
            },
            cx,
        );
    }

    pub(crate) fn conflict_take_whole(&mut self, ours: bool, cx: &mut Context<Self>) {
        let Some(cv) = self.conflict.clone() else { return };
        let path = cv.path.clone();
        self.conflict = None;
        self.run_git(
            tf!(
                "{} 整个文件用{}",
                path,
                if ours { tr("我们的") } else { tr("他们的") }
            ),
            move |cli| {
                cli.run(&["checkout", if ours { "--ours" } else { "--theirs" }, "--", &path])?;
                cli.stage(&[&path])?;
                Ok(tr("已标记为已解决").into())
            },
            cx,
        );
    }

    pub(crate) fn render_conflict_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let Some(cv) = self.conflict.clone() else {
            return div().into_any_element();
        };
        let total = cv.conflicts();
        let left = cv.unresolved();
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
                chrome_button("cf-close", &t, "caret-left", tr("返回（Esc）"), false)
                    .on_click(cx.listener(|this, _, _, cx| this.close_diff(cx))),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .child(cv.path.clone()),
            )
            .child(
                div()
                    .px(px(7.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .bg(if left == 0 { t.cyan_16 } else { t.mag_soft })
                    .text_color(if left == 0 { t.cyan_deep } else { t.mag_deep })
                    .child(tf!("{} / {} 未解决", left, total)),
            )
            .child(div().ml_auto())
            .child(
                pill(&t, "cf-ours-all", tr("整个文件用我们的"), false)
                    .on_click(cx.listener(|this, _, _, cx| this.conflict_take_whole(true, cx))),
            )
            .child(
                pill(&t, "cf-theirs-all", tr("整个文件用他们的"), false)
                    .on_click(cx.listener(|this, _, _, cx| this.conflict_take_whole(false, cx))),
            )
            .child(
                pill(&t, "cf-save", tr("保存"), cv.dirty)
                    .on_click(cx.listener(|this, _, _, cx| this.conflict_save(false, cx))),
            )
            .child(
                div()
                    .id("cf-resolve")
                    .px(px(10.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(if left == 0 { t.cyan } else { t.ink_13 })
                    .text_color(t.surface)
                    .text_size(px(11.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tr("写入文件并 git add（⌘S）"))
                            .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.conflict_save(true, cx)))
                    .child(tr("保存并标记已解决")),
            );

        let mut body = div()
            .id("cf-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .font_family(FONT_MONO)
            .text_size(px(11.5));
        if cv.loading {
            body = body.child(div().p(px(12.)).text_color(t.faint).child(tr("加载中…")));
        }
        if let Some(e) = &cv.error {
            body = body.child(div().p(px(12.)).text_color(t.mag_deep).child(e.clone()));
        }
        let mut cix = 0usize;
        let mut line_no = 1usize;
        for seg in &cv.segments {
            match seg {
                Seg::Text(lines) => {
                    for l in lines {
                        body = body.child(code_line(&t, line_no, l, None));
                        line_no += 1;
                    }
                }
                Seg::Conflict {
                    ours,
                    theirs,
                    base,
                    label_ours,
                    label_theirs,
                    choice,
                } => {
                    let ix = cix;
                    cix += 1;
                    let resolved = *choice != Choice::Unresolved;
                    let mut block = div()
                        .flex()
                        .flex_col()
                        .my(px(4.))
                        .mx(px(6.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if resolved { t.line } else { t.mag });
                    // header with actions
                    let act = |id: (&'static str, usize), label: &'static str, on: bool, ch: Choice| {
                        div()
                            .id(id)
                            .px(px(8.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .text_size(px(11.))
                            .cursor_pointer()
                            .border_1()
                            .border_color(if on { t.cyan } else { t.line })
                            .text_color(if on { t.cyan_deep } else { t.ink })
                            .when(on, |d| d.bg(t.cyan_16))
                            .hover(move |s| s.bg(t.ink_05))
                            .on_click(cx.listener(move |this, _, _, cx| this.conflict_choose(ix, ch, cx)))
                            .child(label)
                    };
                    block = block.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(8.))
                            .py(px(4.))
                            .bg(if resolved { t.ink_05 } else { t.mag_soft })
                            .text_size(px(11.))
                            .child(icon_b(
                                "git-merge",
                                px(12.),
                                if resolved { t.muted } else { t.mag_deep },
                            ))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(tf!("冲突 {}", ix + 1)),
                            )
                            .child(div().text_color(t.faint).child(tf!(
                                "我们的 {} · 他们的 {}{}",
                                label_ours,
                                label_theirs,
                                if base.is_some() { tr(" · 含 base") } else { "" }
                            )))
                            .child(div().ml_auto())
                            .child(act(
                                ("cf-o", ix),
                                tr("用我们的 ⌥1"),
                                *choice == Choice::Ours,
                                Choice::Ours,
                            ))
                            .child(act(
                                ("cf-t", ix),
                                tr("用他们的 ⌥2"),
                                *choice == Choice::Theirs,
                                Choice::Theirs,
                            ))
                            .child(act(
                                ("cf-b", ix),
                                tr("两者 ⌥3"),
                                *choice == Choice::Both,
                                Choice::Both,
                            ))
                            .child(act(
                                ("cf-br", ix),
                                tr("两者(反序)"),
                                *choice == Choice::BothReversed,
                                Choice::BothReversed,
                            ))
                            .when(resolved, |d| {
                                d.child(act(("cf-undo", ix), tr("撤销"), false, Choice::Unresolved))
                            }),
                    );
                    match choice {
                        Choice::Unresolved => {
                            block = block.child(
                                div()
                                    .px(px(8.))
                                    .py(px(2.))
                                    .text_size(px(10.5))
                                    .text_color(t.cyan_deep)
                                    .child(tr("我们的（HEAD）")),
                            );
                            for l in ours {
                                block = block.child(code_line(&t, 0, l, Some(t.cyan_16)));
                            }
                            if let Some(b) = base {
                                block = block.child(
                                    div()
                                        .px(px(8.))
                                        .py(px(2.))
                                        .text_size(px(10.5))
                                        .text_color(t.faint)
                                        .child("base"),
                                );
                                for l in b {
                                    block = block.child(code_line(&t, 0, l, Some(t.ink_05)));
                                }
                            }
                            block = block.child(
                                div()
                                    .px(px(8.))
                                    .py(px(2.))
                                    .text_size(px(10.5))
                                    .text_color(t.mag_deep)
                                    .child(tr("他们的")),
                            );
                            for l in theirs {
                                block = block.child(code_line(&t, 0, l, Some(t.mag_soft)));
                            }
                        }
                        _ => {
                            let chosen: Vec<&String> = match choice {
                                Choice::Ours => ours.iter().collect(),
                                Choice::Theirs => theirs.iter().collect(),
                                Choice::Both => ours.iter().chain(theirs.iter()).collect(),
                                Choice::BothReversed => theirs.iter().chain(ours.iter()).collect(),
                                Choice::Unresolved => unreachable!(),
                            };
                            for l in chosen {
                                block = block.child(code_line(&t, line_no, l, None));
                                line_no += 1;
                            }
                        }
                    }
                    body = body.child(block);
                }
            }
        }
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
}

fn pill(
    t: &crate::theme::Theme,
    id: &'static str,
    label: &'static str,
    on: bool,
) -> gpui::Stateful<gpui::Div> {
    let t2 = *t;
    div()
        .id(id)
        .px(px(8.))
        .py(px(2.))
        .rounded(px(4.))
        .text_size(px(11.5))
        .cursor_pointer()
        .border_1()
        .border_color(if on { t.cyan } else { t.line })
        .text_color(if on { t.cyan_deep } else { t.ink })
        .hover(move |s| s.bg(t2.ink_05))
        .child(label)
}

fn code_line(t: &crate::theme::Theme, no: usize, text: &str, bg: Option<gpui::Rgba>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .h(px(19.))
        .when_some(bg, |d, b| d.bg(b))
        .child(
            div()
                .w(px(44.))
                .flex_none()
                .text_right()
                .pr(px(8.))
                .text_color(t.faint)
                .child(if no > 0 { no.to_string() } else { String::new() }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(text.to_string()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_render_roundtrip() {
        let text = "a\n<<<<<<< HEAD\nours1\n||||||| base\nb0\n=======\ntheirs1\n>>>>>>> feature\nz\n";
        let mut cv = ConflictView::parse("f.txt", text);
        assert_eq!(cv.conflicts(), 1);
        assert_eq!(
            cv.render_text(),
            "a\n<<<<<<< HEAD\nours1\n||||||| base\nb0\n=======\ntheirs1\n>>>>>>> feature\nz\n"
        );
        if let Seg::Conflict { choice, .. } = &mut cv.segments[1] {
            *choice = Choice::Both;
        }
        assert_eq!(cv.render_text(), "a\nours1\ntheirs1\nz\n");
        assert_eq!(cv.unresolved(), 0);
    }
}
