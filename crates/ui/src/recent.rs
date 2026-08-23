//! Recent repositories + open-repository flow (multi-repo, 05 §1). The list lives
//! in `~/.sluice/recent.json`; switching repositories replaces the window root
//! with a fresh Workbench (gpui `Window::replace_root`).

use crate::i18n::tr;
use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use serde::{Deserialize, Serialize};

use crate::icons::{icon_b, icon_f};
use crate::overlays::Overlay;
use crate::workbench::Workbench;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentRepo {
    pub path: PathBuf,
    pub name: String,
    pub last_opened: i64,
}

fn store() -> PathBuf {
    std::env::var_os("SLUICE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sluice")
        .join("recent.json")
}

pub fn load() -> Vec<RecentRepo> {
    std::fs::read_to_string(store())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn remember(path: &Path) {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut list: Vec<RecentRepo> = load().into_iter().filter(|r| r.path != canon).collect();
    let name = canon
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| canon.display().to_string());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    list.insert(
        0,
        RecentRepo {
            path: canon,
            name,
            last_opened: now,
        },
    );
    list.truncate(20);
    if let Some(parent) = store().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(store(), text);
    }
}

pub fn forget(path: &Path) {
    let list: Vec<RecentRepo> = load().into_iter().filter(|r| r.path != path).collect();
    if let Ok(text) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(store(), text);
    }
}

impl Workbench {
    pub(crate) fn open_recent(&mut self, cx: &mut Context<Self>) {
        self.recent = load();
        self.overlay = Some(Overlay::Recent);
        cx.notify();
    }

    /// Native folder picker → switch.
    pub(crate) fn pick_repository(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr("打开仓库").into()),
        });
        cx.spawn_in(_window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                cx.update(|window, cx| {
                    this.update(cx, |this, cx| this.switch_repository(path, window, cx))
                        .ok();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Replace this window's root with a Workbench on `path`.
    pub(crate) fn switch_repository(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match sluice_domain::Repo::open(&path) {
            Ok(repo) => {
                remember(&path);
                // Old IPC endpoint must not keep pointing at this window.
                if let Some(cli) = self.repo.cli.as_ref() {
                    sluice_bridge::ipc::remove_endpoint(cli.workdir());
                }
                let ipc_repo = repo
                    .cli
                    .as_ref()
                    .map(|c| c.workdir().to_path_buf())
                    .unwrap_or_else(|| path.clone());
                let (tx, rx) = async_channel::unbounded();
                let _ = sluice_bridge::ipc::serve(ipc_repo, tx);
                let title = format!(
                    "{} — {}",
                    repo.info.name,
                    repo.info.head.branch.as_deref().unwrap_or("detached HEAD")
                );
                window.set_window_title(&title);
                window.replace_root(cx, |window, cx| {
                    let wb = cx.new(|cx| {
                        let mut w = Workbench::new(repo, window, cx);
                        w.attach_ipc(rx, cx);
                        w
                    });
                    gpui_component::Root::new(gpui::AnyView::from(wb), window, cx)
                });
            }
            Err(e) => {
                self.toast(
                    tf!("打开失败：{}", format!("{e:#}").lines().next().unwrap_or("")),
                    cx,
                );
            }
        }
    }

    pub(crate) fn render_recent(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let list = self.recent.clone();
        let current = self.repo.cli.as_ref().map(|c| c.workdir().to_path_buf());
        let mut rows = div()
            .id("recent-list")
            .max_h(px(360.))
            .overflow_y_scroll()
            .py(px(4.));
        if list.is_empty() {
            rows = rows.child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child(tr("还没有最近仓库。")),
            );
        }
        for (i, r) in list.iter().enumerate() {
            let is_cur = current.as_ref() == Some(&r.path);
            let p1 = r.path.clone();
            let p2 = r.path.clone();
            rows = rows.child(
                div()
                    .id(("recent", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mx(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(is_cur, |d| d.bg(t.sel))
                    .hover(move |s| s.bg(if is_cur { t.sel } else { t.ink_05 }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.overlay = None;
                        if !is_cur {
                            this.switch_repository(p1.clone(), window, cx);
                        }
                    }))
                    .child(icon_f("folder", px(14.), t.muted))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(r.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(t.faint)
                                    .truncate()
                                    .child(r.path.display().to_string()),
                            ),
                    )
                    .when(is_cur, |d| {
                        d.child(div().text_size(px(11.)).text_color(t.faint).child(tr("当前")))
                    })
                    .child(
                        div()
                            .id(("recent-forget", i))
                            .w(px(20.))
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.ink_08))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                forget(&p2);
                                this.recent = load();
                                cx.notify();
                            }))
                            .child(icon_b("x", px(11.), t.faint)),
                    ),
            );
        }
        div()
            .w(px(520.))
            .flex()
            .flex_col()
            .child(self.panel_header(&t, tr("仓库"), tf!("{} 个最近仓库", list.len()), cx))
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
                            .text_size(px(11.5))
                            .text_color(t.faint)
                            .child(tr("⌘O 直接打开文件夹 · ⌘⇧O 此列表")),
                    )
                    .child(div().ml_auto())
                    .child(
                        div()
                            .id("recent-open")
                            .px(px(16.))
                            .py(px(5.))
                            .bg(t.cyan)
                            .text_color(t.surface)
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.cyan_deep))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.overlay = None;
                                this.pick_repository(window, cx);
                            }))
                            .child(tr("打开文件夹…")),
                    ),
            )
    }
}

/// Persisted user settings (`~/.sluice/settings.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    pub telemetry: bool,
    /// Background `git fetch` every `fetch_minutes` (0 = off).
    pub fetch_minutes: u32,
    pub rail_expanded: bool,
    /// "zh" (default) or "en".
    pub lang: String,
    /// "idea" (default) or "vscode".
    pub keymap: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: false,
            telemetry: false,
            fetch_minutes: 5,
            rail_expanded: false,
            lang: "zh".into(),
            keymap: "idea".into(),
        }
    }
}

fn settings_path() -> PathBuf {
    store().with_file_name("settings.json")
}

pub fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    if let Some(parent) = settings_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(settings_path(), text);
    }
}

/// Recent commit messages (last 50, newest first) — `~/.sluice/commit-messages.json`.
fn messages_path() -> PathBuf {
    store().with_file_name("commit-messages.json")
}

pub fn load_messages() -> Vec<String> {
    std::fs::read_to_string(messages_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn remember_message(msg: &str) {
    let msg = msg.trim();
    if msg.is_empty() {
        return;
    }
    let mut list: Vec<String> = load_messages().into_iter().filter(|m| m != msg).collect();
    list.insert(0, msg.to_string());
    list.truncate(50);
    if let Some(parent) = messages_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(messages_path(), text);
    }
}
