//! The root view: window chrome (custom macOS title bar with the segmented
//! Local Changes / Log / Console control), the left tool rail, and the active
//! workspace. Layout and tokens follow `SluiceDesktop.dc.html` (mac variant).

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, ClickEvent, Context, FocusHandle, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Render, ScrollStrategy, StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, actions,
    div, px,
};
use sluice_core::*;
use sluice_domain::RepoStore;

use crate::icons::{icon, icon16};
use crate::theme::{FONT_BODY, FONT_HEADING, Theme};

actions!(
    workbench,
    [
        MoveUp,
        MoveDown,
        SelectFirst,
        SelectLast,
        PageUp,
        PageDown,
        ShowLog,
        ShowChanges,
        ShowConsole,
        Refresh
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Log,
    Console,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Changes => "Local Changes",
            Tab::Log => "Log",
            Tab::Console => "Console",
        }
    }
}

pub struct Workbench {
    pub store: RepoStore,
    pub theme: Theme,
    pub tab: Tab,
    pub selected: usize,
    pub detail: Option<Arc<(CommitDetail, Vec<FileChange>)>>,
    pub detail_error: Option<String>,
    /// `full_name` of the ref the log is filtered to (None = all refs, IDEA default).
    pub selected_ref: Option<String>,
    pub ai_only: bool,
    pub rx: bool,
    pub cc: bool,
    pub scroll: UniformListScrollHandle,
    pub focus: FocusHandle,
    pub status: Option<String>,
}

impl Workbench {
    pub fn new(store: RepoStore, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let mut this = Workbench {
            store,
            theme: Theme::light(),
            tab: Tab::Log,
            selected: 0,
            detail: None,
            detail_error: None,
            selected_ref: None,
            ai_only: false,
            rx: false,
            cc: false,
            scroll: UniformListScrollHandle::new(),
            focus,
            status: None,
        };
        this.load_detail();
        this
    }

    pub fn title(&self) -> String {
        let branch = self
            .store
            .info
            .head
            .branch
            .clone()
            .unwrap_or_else(|| "detached HEAD".into());
        format!("{} — {}", self.store.info.name, branch)
    }

    fn load_detail(&mut self) {
        if self.store.commits.is_empty() {
            self.detail = None;
            return;
        }
        match self.store.detail(self.selected) {
            Ok(d) => {
                self.detail = Some(d);
                self.detail_error = None;
            }
            Err(e) => {
                self.detail = None;
                self.detail_error = Some(format!("{e:#}"));
            }
        }
    }

    pub fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.store.commits.is_empty() {
            return;
        }
        let ix = ix.min(self.store.commits.len() - 1);
        if ix != self.selected || self.detail.is_none() {
            self.selected = ix;
            self.load_detail();
        }
        self.scroll_into_view(ix);
        cx.notify();
    }

    /// `ScrollStrategy` has no "nearest": scroll only when the row is outside the viewport.
    fn scroll_into_view(&self, ix: usize) {
        let state = self.scroll.0.borrow();
        let Some(item) = state.last_item_size else {
            drop(state);
            self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
            return;
        };
        let item_h = item.item.height;
        let viewport_h = state.base_handle.bounds().size.height;
        let offset_y = -state.base_handle.offset().y;
        drop(state);
        if item_h <= px(0.) || viewport_h <= px(0.) {
            return;
        }
        let first = (offset_y / item_h).floor() as usize;
        let visible = ((viewport_h / item_h).floor() as usize).max(1);
        let last = first + visible - 1;
        if ix < first {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        } else if ix > last {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Bottom);
        }
    }

    fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.store.commits.len();
        if n == 0 {
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, n as isize - 1) as usize;
        self.select(next, cx);
    }

    pub fn pick_ref(&mut self, full_name: Option<String>, cx: &mut Context<Self>) {
        let tips = match &full_name {
            None => Vec::new(),
            Some(name) => self
                .store
                .refs
                .iter()
                .filter(|r| &r.full_name == name)
                .map(|r| r.target.clone())
                .collect(),
        };
        match self.store.set_tips(tips) {
            Ok(()) => {
                self.selected_ref = full_name;
                self.selected = 0;
                self.detail = None;
                self.load_detail();
                self.scroll.scroll_to_item(0, ScrollStrategy::Top);
                self.status = None;
            }
            Err(e) => self.status = Some(format!("过滤失败：{e:#}")),
        }
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        match self.store.reload() {
            Ok(()) => {
                self.detail = None;
                self.load_detail();
                self.status = None;
            }
            Err(e) => self.status = Some(format!("刷新失败：{e:#}")),
        }
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    // ----- chrome ---------------------------------------------------------

    fn render_titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let subtitle = self.title();
        let tabs = [Tab::Changes, Tab::Log, Tab::Console];
        let active = self.tab;
        div()
            .id("titlebar")
            .relative()
            .h(px(54.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(18.))
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.line_soft)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .on_click(|ev: &ClickEvent, window, _| {
                if ev.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            // centered title (absolute, drawn below the interactive groups)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .font_family(FONT_HEADING)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(13.5))
                                    .line_height(px(17.))
                                    .child("sluice"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .line_height(px(13.))
                                    .text_color(t.muted)
                                    .child(subtitle),
                            ),
                    ),
            )
            // left: traffic-light reserve + nav icons
            .child(div().w(px(56.)).flex_none())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .flex_none()
                    .child(icon16("sidebar-simple", t.muted))
                    .child(icon16("caret-left", t.muted))
                    .child(icon16("caret-right", t.faint)),
            )
            // right: segmented control + actions
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .flex_none()
                    .child(
                        div()
                            .flex()
                            .gap(px(2.))
                            .p(px(2.))
                            .rounded(px(9.))
                            .bg(t.ink_08)
                            .children(tabs.into_iter().enumerate().map(|(ix, tab)| {
                                let on = tab == active;
                                div()
                                    .id(("tab", ix))
                                    .px(px(14.))
                                    .py(px(3.))
                                    .rounded(px(7.))
                                    .text_size(px(12.5))
                                    .text_color(t.ink)
                                    .cursor_pointer()
                                    .when(on, |d| {
                                        d.bg(t.surface)
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .shadow_sm()
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                                    .child(tab.label())
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(13.))
                            .child(icon16("sparkle", t.mag))
                            .child(
                                div()
                                    .id("refresh")
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                                    .child(icon16("arrow-clockwise", t.muted)),
                            )
                            .child(icon16("magnifying-glass", t.muted))
                            .child(icon16("dots-three-circle", t.muted)),
                    ),
            )
    }

    fn render_rail(&self) -> impl IntoElement {
        let t = self.theme;
        div()
            .w(px(34.))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .py(px(10.))
            .bg(t.ink_05)
            .border_r_1()
            .border_color(t.line_55)
            .child(icon16("git-branch", t.cyan))
            .child(icon16("git-commit", t.muted))
            .child(icon16("git-merge", t.muted))
            .child(icon16("arrow-line-down", t.muted))
            .child(icon16("arrow-line-up", t.muted))
            .child(icon16("star", t.muted))
            .child(icon16("clock-counter-clockwise", t.muted))
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.))
                    .child(icon16("sparkle", t.mag))
                    .child(icon16("terminal-window", t.muted))
                    .child(icon16("gear", t.muted)),
            )
    }

    fn render_placeholder(&self, title: &'static str, body: &'static str) -> impl IntoElement {
        let t = self.theme;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .child(
                div()
                    .font_family(FONT_HEADING)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_size(px(18.))
                    .child(title),
            )
            .child(div().text_size(px(12.5)).text_color(t.muted).child(body))
    }
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let body = match self.tab {
            Tab::Log => self.render_log(cx).into_any_element(),
            Tab::Changes => self
                .render_placeholder(
                    "Local Changes",
                    "变更树 / 行级暂存 / 提交面板 —— M2 交付（05 §5）",
                )
                .into_any_element(),
            Tab::Console => self
                .render_placeholder("Console", "git 命令回显 —— 写操作接入后启用（05 §4）")
                .into_any_element(),
        };
        div()
            .key_context("Workbench")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_by(-1, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_by(1, cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.move_by(-20, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.move_by(20, cx)))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.select(0, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| {
                let n = this.store.commits.len();
                this.select(n.saturating_sub(1), cx)
            }))
            .on_action(cx.listener(|this, _: &ShowLog, _, cx| this.set_tab(Tab::Log, cx)))
            .on_action(cx.listener(|this, _: &ShowChanges, _, cx| this.set_tab(Tab::Changes, cx)))
            .on_action(cx.listener(|this, _: &ShowConsole, _, cx| this.set_tab(Tab::Console, cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.paper)
            .text_color(t.ink)
            .font_family(FONT_BODY)
            .text_size(px(13.))
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_rail())
                    .child(body),
            )
    }
}

/// Small helper used by workspaces: an uppercase section label (11px, faint).
pub fn section_label(t: &Theme, text: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .text_color(t.faint)
        .child(text.into().to_uppercase())
}

/// Agent badge: 15×15 box with a one-glyph mark in the agent tone.
pub fn agent_badge(t: &Theme, agent: Agent) -> impl IntoElement {
    let tone = t.agent_tone(agent);
    div()
        .flex_none()
        .w(px(15.))
        .h(px(15.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(tone)
        .text_color(tone)
        .font_family(crate::theme::FONT_MONO)
        .text_size(px(9.5))
        .line_height(px(13.))
        .child(agent.mark())
}

pub fn tone_icon(name: &'static str, size: f32, color: gpui::Rgba) -> impl IntoElement {
    icon(name, px(size), color)
}

#[allow(dead_code)]
fn _unused(_: &App) {}
