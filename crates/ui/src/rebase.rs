//! Interactive rebase planner (M3, 05 §6): pick / reword / squash / fixup / drop,
//! reorder, then run git with the plan-driven editors. Conflicts fall into the
//! in-progress banner (continue / skip / abort).

use crate::i18n::tr;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, div, px,
};
use gpui_component::input::Input;
use sluice_bridge::rebase::{PlanItem, RebaseAction, RebasePlan};
use sluice_core::Oid;

use crate::icons::icon_b;
use crate::overlays::Overlay;
use crate::theme::FONT_MONO;
use crate::workbench::Workbench;

#[derive(Clone, Debug)]
pub struct RebaseDraft {
    /// `None` = `--root`.
    pub base: Option<String>,
    pub items: Vec<PlanItem>,
    pub selected: usize,
    pub loading: bool,
}

impl Workbench {
    /// Start planning an interactive rebase that replays `from` (inclusive) .. HEAD.
    pub(crate) fn open_rebase_from(&mut self, from: Oid, cx: &mut Context<Self>) {
        let Some(cli) = self.repo.cli.clone() else {
            self.toast("裸仓库没有工作区", cx);
            return;
        };
        let base = format!("{}~1", from.as_str());
        self.rebase = Some(RebaseDraft {
            base: Some(base.clone()),
            items: Vec::new(),
            selected: 0,
            loading: true,
        });
        self.overlay = Some(Overlay::Rebase);
        cx.notify();
        cx.spawn(async move |this, cx| {
            // If `from` is a root commit `~1` does not resolve: fall back to --root.
            let res = cx
                .background_spawn(async move {
                    match cli.rebase_range(Some(&base)) {
                        Ok(v) => Ok((Some(base), v)),
                        Err(_) => cli.rebase_range(None).map(|v| (None, v)),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(d) = this.rebase.as_mut() {
                    d.loading = false;
                    match res {
                        Ok((base, rows)) => {
                            d.base = base;
                            d.items = rows
                                .into_iter()
                                .map(|(sha, subject)| PlanItem {
                                    sha,
                                    subject,
                                    action: RebaseAction::Pick,
                                    message: None,
                                })
                                .collect();
                            d.selected = d.items.len().saturating_sub(1);
                        }
                        Err(e) => this.toast(tf!("读取 rebase 范围失败：{}", format!("{e:#}")), cx),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn rebase_move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(d) = self.rebase.as_mut()
            && !d.items.is_empty()
        {
            let n = d.items.len() as i32;
            d.selected = ((d.selected as i32 + delta).clamp(0, n - 1)) as usize;
            cx.notify();
        }
    }

    pub(crate) fn rebase_reorder(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(d) = self.rebase.as_mut() {
            let i = d.selected as i32;
            let j = i + delta;
            if i >= 0 && j >= 0 && (j as usize) < d.items.len() && (i as usize) < d.items.len() {
                d.items.swap(i as usize, j as usize);
                d.selected = j as usize;
                cx.notify();
            }
        }
    }

    pub(crate) fn rebase_cycle_action(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.rebase.as_mut()
            && let Some(it) = d.items.get_mut(d.selected)
        {
            it.action = it.action.next();
            cx.notify();
        }
    }

    pub(crate) fn rebase_start(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.rebase.clone() else { return };
        if d.items.is_empty() {
            return;
        }
        // The first item cannot be squash/fixup (nothing before it).
        if matches!(d.items[0].action, RebaseAction::Squash | RebaseAction::Fixup) {
            self.toast("第一个提交不能是 squash / fixup", cx);
            return;
        }
        let Some(cli) = self.repo.cli.clone() else { return };
        let mut plan = RebasePlan {
            items: d.items.clone(),
            pending_messages: Vec::new(),
        };
        plan.prepare_messages();
        let plan_path = cli.workdir().join(".git").join("sluice").join("rebase-plan.json");
        let exe = std::env::current_exe().unwrap_or_else(|_| "sluice".into());
        let base = d.base.clone();
        self.overlay = None;
        self.rebase = None;
        self.run_git(
            tr("交互式 rebase").to_string(),
            move |cli| {
                let snap = cli.snapshot_create("before interactive rebase")?;
                plan.save(&plan_path)?;
                let out = cli.rebase_interactive(base.as_deref(), &exe, &plan_path);
                let _ = std::fs::remove_file(&plan_path);
                let out = out?;
                let last = out.stderr.lines().last().unwrap_or("").trim().to_string();
                Ok(format!(
                    "{}{}",
                    if snap.is_some() { tr("已快照 · ") } else { "" },
                    if last.is_empty() {
                        tr("完成").into()
                    } else {
                        last
                    }
                ))
            },
            cx,
        );
    }

    pub(crate) fn render_rebase(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let Some(d) = self.rebase.clone() else {
            return div().into_any_element();
        };
        let selected = d.selected;
        let mut rows = div()
            .id("rebase-rows")
            .max_h(px(380.))
            .overflow_y_scroll()
            .py(px(4.));
        if d.loading {
            rows = rows.child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_color(t.faint)
                    .text_size(px(12.5))
                    .child(tr("加载中…")),
            );
        }
        for (i, it) in d.items.iter().enumerate() {
            let is_sel = i == selected;
            let action = it.action;
            let color = match action {
                RebaseAction::Pick => t.muted,
                RebaseAction::Reword => t.cyan_deep,
                RebaseAction::Squash | RebaseAction::Fixup => t.yellow,
                RebaseAction::Drop => t.mag_deep,
            };
            rows = rows.child(
                div()
                    .id(("rb-row", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mx(px(8.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(is_sel, |x| x.bg(t.sel))
                    .hover(move |s| s.bg(if is_sel { t.sel } else { t.ink_05 }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.rebase.as_mut() {
                            d.selected = i;
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(22.))
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .font_family(FONT_MONO)
                            .child(format!("{}", i + 1)),
                    )
                    .child(
                        div()
                            .id(("rb-act", i))
                            .w(px(72.))
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(color)
                            .text_color(color)
                            .text_size(px(11.))
                            .text_center()
                            .cursor_pointer()
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(tr(
                                    "点击切换：pick → reword → squash → fixup → drop（Space）",
                                ))
                                .build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(d) = this.rebase.as_mut() {
                                    d.selected = i;
                                }
                                this.rebase_cycle_action(cx);
                            }))
                            .child(action.label_zh()),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(t.cyan_deep)
                            .child(it.sha[..8].to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .when(action == RebaseAction::Drop, |x| {
                                x.line_through().text_color(t.faint)
                            })
                            .child(
                                it.message
                                    .as_ref()
                                    .and_then(|m| m.lines().next().map(str::to_string))
                                    .unwrap_or_else(|| it.subject.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id(("rb-up", i))
                            .w(px(18.))
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.ink_08))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(d) = this.rebase.as_mut() {
                                    d.selected = i;
                                }
                                this.rebase_reorder(-1, cx);
                            }))
                            .child(icon_b("caret-up", px(11.), t.muted)),
                    )
                    .child(
                        div()
                            .id(("rb-down", i))
                            .w(px(18.))
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.ink_08))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(d) = this.rebase.as_mut() {
                                    d.selected = i;
                                }
                                this.rebase_reorder(1, cx);
                            }))
                            .child(icon_b("caret-down", px(11.), t.muted)),
                    ),
            );
        }
        let sel_item = d.items.get(selected).cloned();
        let show_editor = sel_item
            .as_ref()
            .is_some_and(|it| matches!(it.action, RebaseAction::Reword | RebaseAction::Squash));
        div()
            .w(px(640.))
            .flex()
            .flex_col()
            .child(self.panel_header(
                &t,
                tr("交互式 rebase"),
                tf!("{} 个提交 · 基点 {}", d.items.len(), d.base
                        .clone()
                        .map(|b| match b.split_once('~') {
                            Some((sha, n)) => format!("{}~{n}", sha.chars().take(8).collect::<String>()),
                            None => b.chars().take(10).collect::<String>(),
                        })
                        .unwrap_or_else(|| "--root".into())),
                cx,
            ))
            .child(
                div().px(px(16.)).pt(px(8.)).pb(px(2.)).text_size(px(11.5)).text_color(t.faint).child(
                    tr("↑↓ 选择 · Space 切换动作 · ⌥↑ / ⌥↓ 调整顺序 · Enter 开始。开始前自动创建时光机快照；冲突时用进行中横幅 continue / abort。"),
                ),
            )
            .child(rows)
            .when(show_editor, |x| {
                x.child(
                    div()
                        .mx(px(16.))
                        .my(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(div().text_size(px(11.)).text_color(t.faint).child(tr("新的提交信息（reword / squash 结果）")))
                        .child(
                            div()
                                .border_1()
                                .border_color(t.line)
                                .bg(t.surface)
                                .rounded(px(6.))
                                .px(px(9.))
                                .py(px(3.))
                                .text_size(px(12.5))
                                .child(Input::new(&self.rebase_msg).appearance(false)),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line_soft)
                    .child(div().text_size(px(11.5)).text_color(t.faint).child(tr("等价：git rebase -i，由 sluice seq-editor / editor 按计划执行")))
                    .child(div().ml_auto())
                    .child(
                        div()
                            .id("rb-cancel")
                            .px(px(14.))
                            .py(px(5.))
                            .border_1()
                            .border_color(t.line)
                            .rounded(px(4.))
                            .text_size(px(12.5))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.ink_05))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.overlay = None;
                                this.rebase = None;
                                cx.notify();
                            }))
                            .child(tr("取消")),
                    )
                    .child(
                        div()
                            .id("rb-start")
                            .px(px(16.))
                            .py(px(5.))
                            .bg(t.cyan)
                            .text_color(t.surface)
                            .rounded(px(4.))
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.cyan_deep))
                            .on_click(cx.listener(|this, _, _, cx| this.rebase_start(cx)))
                            .child(tr("开始 rebase")),
                    ),
            )
            .into_any_element()
    }
}
