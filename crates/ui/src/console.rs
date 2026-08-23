//! Console tab (prototype screen 10): every git command Sluice ran, with
//! duration, exit code and a one-line summary; verbose mode adds the gix
//! read path as equivalent commands (05 §4).

use crate::i18n::tr;
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px,
};
use sluice_core::ConsoleKind;

use crate::theme::FONT_MONO;
use crate::workbench::{Workbench, section_label};

impl Workbench {
    pub(crate) fn render_console(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let verbose = self.console_verbose;
        let filter = self.console_filter;
        let mut entries = self.repo.console.entries();
        entries.retain(|e| verbose || e.kind != ConsoleKind::Read);
        if let Some(k) = filter {
            entries.retain(|e| e.kind == k);
        }
        let total = entries.len();
        let chip = |on: bool, label: &'static str| {
            div()
                .px(px(9.))
                .py(px(2.))
                .text_size(px(12.))
                .border_1()
                .border_color(if on { t.cyan } else { t.line })
                .when(on, |d| d.bg(t.cyan_soft).text_color(t.cyan_deep))
                .when(!on, |d| d.text_color(t.muted))
                .hover(move |s| s.border_color(t.cyan))
                .child(label)
        };
        let bar = div()
            .px(px(12.))
            .py(px(7.))
            .border_b_1()
            .border_color(t.line_soft)
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_none()
            .child(section_label(&t, "Console"))
            .child(
                div()
                    .id("c-all")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.console_filter = None;
                        cx.notify();
                    }))
                    .child(chip(filter.is_none(), tr("全部"))),
            )
            .child(
                div()
                    .id("c-write")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.console_filter = Some(ConsoleKind::Write);
                        cx.notify();
                    }))
                    .child(chip(filter == Some(ConsoleKind::Write), tr("写操作"))),
            )
            .child(
                div()
                    .id("c-read")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.console_filter = Some(ConsoleKind::Read);
                        this.console_verbose = true;
                        cx.notify();
                    }))
                    .child(chip(filter == Some(ConsoleKind::Read), tr("读操作"))),
            )
            .child(
                div()
                    .id("c-ai")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.console_filter = Some(ConsoleKind::Ai);
                        cx.notify();
                    }))
                    .child(chip(filter == Some(ConsoleKind::Ai), "AI")),
            )
            .child(
                div()
                    .id("c-verbose")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.console_verbose = !this.console_verbose;
                        cx.notify();
                    }))
                    .child(chip(verbose, tr("详细（含 gix 读等价命令）"))),
            )
            .child(
                div()
                    .ml_auto()
                    .text_size(px(11.5))
                    .text_color(t.muted)
                    .child(tf!("{} 条", total)),
            )
            .child(
                div()
                    .id("c-clear")
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(t.cyan_deep)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.repo.console.clear();
                        cx.notify();
                    }))
                    .child(tr("清空")),
            )
            .child(
                div()
                    .id("c-dock")
                    .ml(px(10.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(t.cyan_deep)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_console_dock(cx)))
                    .child(if self.settings.console_docked {
                        tr("并回页签")
                    } else {
                        tr("拆分到底部")
                    }),
            );

        let mut list = div()
            .id("console-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(4.));
        for (i, e) in entries.iter().rev().enumerate() {
            let (tag, color) = match e.kind {
                ConsoleKind::Read => (tr("读"), t.muted),
                ConsoleKind::Write => (tr("写"), t.cyan_deep),
                ConsoleKind::Ai => ("AI", t.mag_deep),
            };
            let ok = e.exit_code.unwrap_or(0) == 0;
            list = list.child(
                div()
                    .id(("c", i))
                    .px(px(12.))
                    .py(px(5.))
                    .border_b_1()
                    .border_color(t.line_soft)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(10.))
                            .font_family(FONT_MONO)
                            .text_size(px(11.5))
                            .child(
                                div()
                                    .w(px(62.))
                                    .flex_none()
                                    .text_color(t.faint)
                                    .child(e.at.format("%H:%M:%S").to_string()),
                            )
                            .child(
                                div()
                                    .w(px(22.))
                                    .flex_none()
                                    .text_center()
                                    .text_size(px(10.5))
                                    .border_1()
                                    .border_color(color)
                                    .text_color(color)
                                    .child(tag),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(t.ink)
                                    .child(e.command.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(t.faint)
                                    .child(format!("{}ms", e.duration_ms)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(48.))
                                    .text_right()
                                    .text_color(if ok { t.faint } else { t.mag_deep })
                                    .child(format!("exit {}", e.exit_code.unwrap_or(-1))),
                            ),
                    )
                    .when(!e.summary.is_empty(), |d| {
                        d.child(
                            div()
                                .pl(px(94.))
                                .text_size(px(12.))
                                .text_color(t.muted)
                                .child(e.summary.clone()),
                        )
                    })
                    .when(!ok && !e.stderr.is_empty(), |d| {
                        d.child(
                            div()
                                .pl(px(94.))
                                .font_family(FONT_MONO)
                                .text_size(px(11.))
                                .text_color(t.mag_deep)
                                .child(e.stderr.lines().take(6).collect::<Vec<_>>().join("\n")),
                        )
                    }),
            );
        }
        if entries.is_empty() {
            list = list.child(
                div()
                    .p(px(16.))
                    .text_color(t.muted)
                    .child(tr("暂无记录。每一条写操作都会在这里还原为等价 git 命令。")),
            );
        }
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(bar)
            .child(list)
    }
}
