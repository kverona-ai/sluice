//! Desktop side of the mobile sync channel (02 §5.4 / §5.7, 05 §7.1): the
//! `Backend` the `sluice-sync` server calls from its session threads, the UI
//! glue that publishes the read model, the pairing / devices panel and the
//! routing of device decisions into the propose-and-confirm queue.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window,
    canvas, div, fill, point, px, size,
};
use gpui_component::input::Input;
use sluice_backend_cli::GitCli;
use sluice_core::diff::{DiffOptions, FileDiff, LineKind};
use sluice_core::{GitReader, LogQuery, Oid};
use sluice_sync::proto::*;
use sluice_sync::server::{Backend, DecisionOutcome, DecisionRequest};
use sluice_sync::store::PairedDevice;
use sluice_sync::{PairingPayload, Status, SyncServer};

use crate::i18n::tr;
use crate::icons::icon_b;
use crate::overlays::Overlay;
use crate::theme::FONT_MONO;
use crate::workbench::Workbench;

/// What session threads hand to the UI thread.
pub enum SyncInbound {
    Decide {
        req: DecisionRequest,
        reply: mpsc::Sender<DecisionOutcome>,
    },
    /// Devices / sessions / relay state changed — refresh the panel.
    Changed,
}

/// Shared with the server's session threads; the UI refreshes it after every
/// reload and the threads read it without touching gpui.
pub struct SyncShared {
    repo_view: Mutex<Option<RepoView>>,
    queue: Mutex<Vec<ReviewItem>>,
    reader: Mutex<Option<Arc<dyn GitReader>>>,
    cli: Mutex<Option<Arc<GitCli>>>,
    tx: async_channel::Sender<SyncInbound>,
}

pub struct SyncHost {
    pub server: SyncServer,
    pub shared: Arc<SyncShared>,
}

/// Where the desktop keeps its channel state: `~/.sluice` (or `SLUICE_CONFIG_HOME/.sluice`).
pub fn config_dir() -> PathBuf {
    std::env::var_os("SLUICE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sluice")
}

/// Start the channel. Returns None (and logs) when the listener cannot bind.
pub fn start(app_version: &str) -> Option<(SyncHost, async_channel::Receiver<SyncInbound>)> {
    if std::env::var_os("SLUICE_NO_SYNC").is_some() {
        return None;
    }
    let (tx, rx) = async_channel::unbounded();
    let shared = Arc::new(SyncShared {
        repo_view: Mutex::new(None),
        queue: Mutex::new(Vec::new()),
        reader: Mutex::new(None),
        cli: Mutex::new(None),
        tx,
    });
    match SyncServer::start(&config_dir(), shared.clone(), app_version) {
        Ok(server) => {
            if let Some(p) = server.status().lan_port {
                tracing::info!("sync channel listening on port {p}");
            }
            Some((SyncHost { server, shared }, rx))
        }
        Err(e) => {
            tracing::warn!("sync channel not started: {e:#}");
            None
        }
    }
}

const MAX_PATCH: usize = 256 * 1024;

/// Unified-diff text of a `FileDiff` (what the phone renders with its own viewer).
pub fn unified(d: &FileDiff, path: &str) -> (String, bool) {
    let mut out = String::new();
    out.push_str(&format!(
        "--- a/{}\n+++ b/{}\n",
        d.old_path.as_deref().unwrap_or(path),
        d.new_path.as_deref().unwrap_or(path)
    ));
    if d.binary {
        out.push_str("Binary files differ\n");
        return (out, false);
    }
    for h in &d.hunks {
        out.push_str(&h.header());
        out.push('\n');
        for l in &h.lines {
            let sign = match l.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Deleted => '-',
            };
            out.push(sign);
            out.push_str(&l.text);
            out.push('\n');
            if out.len() > MAX_PATCH {
                out.push_str("… (truncated)\n");
                return (out, true);
            }
        }
    }
    (out, d.truncated)
}

fn map_commit(c: &sluice_core::Commit, refs: &[String]) -> LogRow {
    LogRow {
        oid: c.id.to_string(),
        short: c.id.to_string().chars().take(7).collect(),
        subject: c.summary.clone(),
        author: c.author.name.clone(),
        at: c.author.time.timestamp(),
        parents: c.parents.iter().map(|p| p.to_string()).collect(),
        refs: refs.to_vec(),
        ai_badge: c.agent.is_ai().then(|| c.agent.label().to_string()),
    }
}

impl Backend for SyncShared {
    fn repo_view(&self) -> Option<RepoView> {
        self.repo_view.lock().unwrap().clone()
    }

    fn queue(&self) -> Vec<ReviewItem> {
        self.queue.lock().unwrap().clone()
    }

    fn decide(&self, req: DecisionRequest) -> DecisionOutcome {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send_blocking(SyncInbound::Decide { req, reply: reply_tx })
            .is_err()
        {
            return DecisionOutcome {
                outcome: "failed".into(),
                detail: "desktop UI is not running".into(),
            };
        }
        reply_rx
            .recv_timeout(Duration::from_secs(170))
            .unwrap_or(DecisionOutcome {
                outcome: "failed".into(),
                detail: "the desktop did not finish in time".into(),
            })
    }

    fn log(&self, offset: u32, limit: u32) -> (u32, Vec<LogRow>) {
        let Some(reader) = self.reader.lock().unwrap().clone() else {
            return (0, Vec::new());
        };
        let want = (offset as usize).saturating_add(limit as usize).saturating_add(1);
        let query = LogQuery {
            limit: want,
            ..Default::default()
        };
        let Ok(commits) = reader.log(&query) else {
            return (0, Vec::new());
        };
        let refs = reader.refs().unwrap_or_default();
        let rows = commits
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|c| {
                let names: Vec<String> = refs
                    .iter()
                    .filter(|r| r.target == c.id)
                    .map(|r| r.short_name.clone())
                    .collect();
                map_commit(c, &names)
            })
            .collect();
        (commits.len() as u32, rows)
    }

    fn commit(&self, oid: &str) -> Option<CommitDetail> {
        let reader = self.reader.lock().unwrap().clone()?;
        let id = Oid::new(oid);
        let snap = sluice_domain::DetailSnapshot::load(&*reader, &id).ok()?;
        let c = &snap.detail.commit;
        let (subject, body) = match snap.detail.message.split_once('\n') {
            Some((s, b)) => (s.to_string(), b.trim().to_string()),
            None => (snap.detail.message.clone(), String::new()),
        };
        Some(CommitDetail {
            oid: c.id.to_string(),
            short: c.id.to_string().chars().take(7).collect(),
            subject,
            body,
            author: c.author.name.clone(),
            at: c.author.time.timestamp(),
            parents: c.parents.iter().map(|p| p.to_string()).collect(),
            files: snap
                .changes
                .iter()
                .map(|f| ChangedFile {
                    path: f.path.clone(),
                    status: f.kind.mark().to_string(),
                    additions: f.additions.unwrap_or(0),
                    deletions: f.deletions.unwrap_or(0),
                })
                .collect(),
            ai_badge: c.agent.is_ai().then(|| c.agent.label().to_string()),
        })
    }

    fn diff(&self, oid: &str, path: &str) -> anyhow::Result<(String, bool)> {
        let reader = self
            .reader
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no repository open"))?;
        let opts = DiffOptions::default();
        let d = if oid.is_empty() {
            // Uncommitted: HEAD → worktree (what a commit proposal would include).
            let unstaged = sluice_domain::diff_work_file(&*reader, path, None, false, &opts)?;
            if unstaged.hunks.is_empty() {
                sluice_domain::diff_work_file(&*reader, path, None, true, &opts)?
            } else {
                unstaged
            }
        } else {
            let id = Oid::new(oid);
            sluice_domain::diff_commit_file(&*reader, &id, path, None, &opts)?
        };
        Ok(unified(&d, path))
    }

    fn on_devices_changed(&self) {
        let _ = self.tx.send_blocking(SyncInbound::Changed);
    }
}

/// `<common-dir>` of the repo (worktrees point at the main repo's git dir).
pub fn common_dir(info: &sluice_core::RepoInfo) -> PathBuf {
    let marker = info.git_dir.join("commondir");
    if let Ok(rel) = std::fs::read_to_string(&marker) {
        let rel = rel.trim();
        let p = info.git_dir.join(rel);
        if let Ok(c) = p.canonicalize() {
            return c;
        }
        return p;
    }
    info.git_dir.clone()
}

pub fn short_hash<T: Hash>(v: &T) -> String {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Workbench {
    /// Wire the channel into the workbench (called once by the app).
    pub fn attach_sync(
        &mut self,
        host: SyncHost,
        rx: async_channel::Receiver<SyncInbound>,
        cx: &mut Context<Self>,
    ) {
        // Scripting / test hook: keep the current pairing payload in a file so a
        // second machine can `sluice pair "$(cat file)"` without the QR.
        if let Some(path) = std::env::var_os("SLUICE_SYNC_PAIRING_FILE")
            && let Ok(p) = host.server.begin_pairing()
        {
            let _ = std::fs::write(path, p.encode());
        }
        self.sync = Some(host);
        self.sync_refresh_status();
        self.sync_publish();
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let ok = this.update(cx, |this, cx| match msg {
                    SyncInbound::Decide { req, reply } => this.decide_from_device(req, reply, cx),
                    SyncInbound::Changed => {
                        this.sync_refresh_status();
                        cx.notify();
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Refresh the read model the phones see and push it to connected devices.
    /// Cheap; call after log / changes reloads and queue changes.
    pub fn sync_publish(&mut self) {
        let Some(host) = &self.sync else { return };
        let info = &self.repo.info;
        let head_id = info.head.id.as_ref().map(|o| o.to_string()).unwrap_or_default();
        let head_subject = self
            .log
            .as_ref()
            .and_then(|l| {
                let id = info.head.id.as_ref()?;
                l.commits.iter().find(|c| &c.id == id).map(|c| c.summary.clone())
            })
            .unwrap_or_default();
        let (ahead, behind, changed) = match &self.changes {
            Some(c) => (c.status.ahead, c.status.behind, c.status.entries.len()),
            None => (info.head.ahead, info.head.behind, 0),
        };
        let view = RepoView {
            path: info
                .workdir
                .as_ref()
                .unwrap_or(&info.git_dir)
                .to_string_lossy()
                .to_string(),
            name: info.name.clone(),
            branch: info
                .head
                .branch
                .clone()
                .unwrap_or_else(|| "detached HEAD".to_string()),
            head_short: head_id.chars().take(7).collect(),
            head_subject,
            ahead: ahead as u32,
            behind: behind as u32,
            changed_files: changed as u32,
            vcs: if info.vcs.is_jj() { "jj" } else { "git" }.to_string(),
            updated_at: sluice_sync::pairing::now(),
        };
        let queue: Vec<ReviewItem> = self.proposals.iter().map(|p| p.review_item()).collect();
        *host.shared.repo_view.lock().unwrap() = Some(view);
        *host.shared.queue.lock().unwrap() = queue;
        *host.shared.reader.lock().unwrap() = Some(self.repo.reader.clone());
        *host.shared.cli.lock().unwrap() = self.repo.cli.clone();
        host.server.broadcast_state();
    }

    pub fn sync_event(&self, event: DomainEvent) {
        if let Some(host) = &self.sync {
            host.server.broadcast_event(event);
        }
    }

    fn sync_refresh_status(&mut self) {
        if let Some(host) = &self.sync {
            self.sync_status = host.server.status();
            self.sync_devices = host.server.devices();
            self.sync_audit = sluice_sync::store::recent_audit(&common_dir(&self.repo.info), 8);
        }
    }

    fn decide_from_device(
        &mut self,
        req: DecisionRequest,
        reply: mpsc::Sender<DecisionOutcome>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.proposals.iter().position(|p| p.proposal.id == req.id) else {
            let _ = reply.send(DecisionOutcome {
                outcome: "unknown".into(),
                detail: "the proposal is no longer in the queue".into(),
            });
            return;
        };
        let entry = &self.proposals[ix];
        if !entry.baseline.is_empty() && !req.version.is_empty() && req.version != entry.baseline {
            let _ = reply.send(DecisionOutcome {
                outcome: "expired".into(),
                detail: "the repository changed since this item was shown — refresh and review again".into(),
            });
            return;
        }
        let source = crate::proposals::DecisionSource::Device(req.device.clone());
        self.decide_proposal_from(ix, req.accept, source, req.note.clone(), Some(reply), cx);
    }

    // ------------------------------------------------------------------ panel

    pub fn open_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_refresh_status();
        if let Some(host) = &self.sync {
            let relay = host.server.status().relay.unwrap_or_default();
            self.relay_input
                .update(cx, |s, cx| s.set_value(relay, window, cx));
            if self.sync_status.pairing.is_none()
                && let Ok(p) = host.server.begin_pairing()
            {
                self.sync_qr = sluice_sync::pairing::qr_matrix(&p.encode()).ok();
                self.sync_status.pairing = Some(p);
            } else if let Some(p) = &self.sync_status.pairing {
                self.sync_qr = sluice_sync::pairing::qr_matrix(&p.encode()).ok();
            }
        }
        self.overlay = Some(Overlay::Devices);
        cx.notify();
        // Tick once a second while the panel is open (countdown + status).
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let open = this
                    .update(cx, |this, cx| {
                        let open = this.overlay == Some(Overlay::Devices);
                        if open {
                            this.sync_refresh_status();
                            cx.notify();
                        }
                        open
                    })
                    .unwrap_or(false);
                if !open {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn sync_new_code(&mut self, cx: &mut Context<Self>) {
        if let Some(host) = &self.sync
            && let Ok(p) = host.server.begin_pairing()
        {
            if let Some(path) = std::env::var_os("SLUICE_SYNC_PAIRING_FILE") {
                let _ = std::fs::write(path, p.encode());
            }
            self.sync_qr = sluice_sync::pairing::qr_matrix(&p.encode()).ok();
            self.sync_status.pairing = Some(p);
        }
        cx.notify();
    }

    pub(crate) fn sync_revoke(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(host) = &self.sync
            && host.server.revoke(&id)
        {
            self.repo.console.note("devices", format!("revoked device {id}"));
            self.toast(tr("已撤销该设备的配对"), cx);
        }
        self.sync_refresh_status();
        cx.notify();
    }

    pub(crate) fn sync_save_relay(&mut self, cx: &mut Context<Self>) {
        let relay = self.relay_input.read(cx).value().to_string();
        if let Some(host) = &self.sync {
            host.server
                .set_relay(Some(relay.clone()).filter(|r| !r.trim().is_empty()));
            self.toast(
                if relay.trim().is_empty() {
                    tr("已关闭中继兜底")
                } else {
                    tr("中继地址已保存（重新生成二维码后生效）")
                },
                cx,
            );
        }
        self.sync_new_code(cx);
    }

    pub(crate) fn sync_toggle_enabled(&mut self, cx: &mut Context<Self>) {
        if let Some(host) = &self.sync {
            let enabled = !host.server.status().enabled;
            host.server.set_enabled(enabled);
        }
        self.sync_refresh_status();
        cx.notify();
    }

    pub(crate) fn render_devices(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let status: Status = self.sync_status.clone();
        let devices: Vec<PairedDevice> = self.sync_devices.clone();
        let pairing: Option<PairingPayload> = status.pairing.clone();
        let qr = self.sync_qr.clone();
        let audit = self.sync_audit.clone();
        let available = self.sync.is_some();
        let desktop_name = self
            .sync
            .as_ref()
            .map(|h| h.server.desktop_name())
            .unwrap_or_default();

        let mut left = div().flex().flex_col().gap(px(10.)).w(px(300.));
        match (&pairing, &qr) {
            (Some(p), Some((n, cells))) => {
                let n = *n;
                let cells = cells.clone();
                let fg = t.ink;
                let bg = t.paper;
                let side = 236.;
                let pay = p.encode();
                let secs = p.seconds_left();
                left = left
                    .child(
                        div()
                            .w(px(side + 16.))
                            .h(px(side + 16.))
                            .p(px(8.))
                            .rounded(px(8.))
                            .bg(bg)
                            .border_1()
                            .border_color(t.line)
                            .child(
                                canvas(
                                    move |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        let cell = bounds.size.width / n as f32;
                                        for y in 0..n {
                                            for x in 0..n {
                                                if cells[y * n + x] {
                                                    let origin = point(
                                                        bounds.origin.x + cell * x as f32,
                                                        bounds.origin.y + cell * y as f32,
                                                    );
                                                    window.paint_quad(fill(
                                                        gpui::Bounds {
                                                            origin,
                                                            size: size(cell + px(0.4), cell + px(0.4)),
                                                        },
                                                        fg,
                                                    ));
                                                }
                                            }
                                        }
                                    },
                                )
                                .w(px(side))
                                .h(px(side)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(if secs > 0 {
                                tf!("一次性配对码 · {}:{} 后失效", secs / 60, format!("{:02}", secs % 60))
                            } else {
                                tr("配对码已失效").to_string()
                            })
                            .child(
                                div()
                                    .id("sync-new-code")
                                    .ml_auto()
                                    .cursor_pointer()
                                    .text_color(t.cyan_deep)
                                    .child(tr("重新生成"))
                                    .on_click(cx.listener(|this, _, _, cx| this.sync_new_code(cx))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .px(px(8.))
                                    .py(px(5.))
                                    .rounded(px(6.))
                                    .bg(t.surface)
                                    .border_1()
                                    .border_color(t.line)
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.5))
                                    .text_color(t.muted)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(pay.clone()),
                            )
                            .child(
                                div()
                                    .id("sync-copy")
                                    .px(px(8.))
                                    .py(px(5.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(t.line)
                                    .cursor_pointer()
                                    .text_size(px(12.))
                                    .child(tr("复制"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(pay.clone()));
                                        this.toast(tr("配对串已复制（手机输入或 sluice pair 使用）"), cx);
                                    })),
                            ),
                    )
                    .child(
                        div().text_size(px(11.5)).text_color(t.faint).child(tr(
                            "用 Sluice 手机端扫码配对；没有手机时可在另一台机器运行 `sluice pair <配对串>`。同网直连优先，配置中继后可在外网兜底；全程端到端加密，放行指令由设备签名。",
                        )),
                    );
            }
            _ => {
                left = left.child(div().py(px(20.)).text_size(px(12.5)).text_color(t.muted).child(
                    if available {
                        tr("正在生成配对码…")
                    } else {
                        tr("同步通道未启动（端口不可用或被 SLUICE_NO_SYNC 关闭）")
                    },
                ));
            }
        }

        // ---- right: channel status, devices, relay, audit ----
        let mut right = div().flex().flex_col().gap(px(10.)).flex_1().min_w(px(320.));
        let lan = if status.lan_addrs.is_empty() {
            status
                .lan_port
                .map(|p| format!("port {p}"))
                .unwrap_or_else(|| "—".into())
        } else {
            status.lan_addrs.join("  ")
        };
        let relay_line = match (&status.relay, status.relay_connected) {
            (Some(r), true) => tf!("中继 {} · 已连接", r),
            (Some(r), false) => tf!("中继 {} · 未连接", r),
            (None, _) => tr("未配置中继（仅同网直连）").to_string(),
        };
        let enabled = status.enabled;
        right = right.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(10.))
                .rounded(px(8.))
                .border_1()
                .border_color(t.line)
                .bg(t.surface)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_size(px(12.5))
                                .child(tr("通道状态")),
                        )
                        .child(
                            div()
                                .id("sync-toggle")
                                .ml_auto()
                                .px(px(8.))
                                .py(px(3.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(t.line)
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .text_color(if enabled { t.cyan_deep } else { t.muted })
                                .child(if enabled {
                                    tr("已启用 · 点击停用")
                                } else {
                                    tr("已停用 · 点击启用")
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.sync_toggle_enabled(cx))),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(t.muted)
                        .font_family(FONT_MONO)
                        .child(tf!("LAN {}", lan)),
                )
                .child(div().text_size(px(11.5)).text_color(t.muted).child(relay_line))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .mt(px(4.))
                        .child(div().flex_1().child(gpui_component::Sizable::small(
                            Input::new(&self.relay_input).appearance(true),
                        )))
                        .child(
                            div()
                                .id("sync-relay-save")
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(t.line)
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .child(tr("保存中继"))
                                .on_click(cx.listener(|this, _, _, cx| this.sync_save_relay(cx))),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(t.faint)
                        .child(tr("中继只转发密文（host:port；`sluice relay serve` 可自托管）。")),
                ),
        );

        // devices
        let sessions = status.sessions.clone();
        let mut dev_list = div().flex().flex_col().gap(px(4.));
        if devices.is_empty() {
            dev_list = dev_list.child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(tr("还没有配对的设备。")),
            );
        }
        for d in devices {
            let online = sessions
                .iter()
                .find(|s| s.device.id == d.id)
                .map(|s| s.via.clone());
            let id = d.id.clone();
            let seen = chrono::DateTime::from_timestamp(d.last_seen, 0)
                .map(|x| x.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
                .unwrap_or_default();
            dev_list = dev_list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(t.line)
                    .child(icon_b(
                        "device-mobile",
                        px(14.),
                        if online.is_some() { t.cyan_deep } else { t.muted },
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(format!("{} · {}", d.name, d.platform)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(t.faint)
                                    .font_family(FONT_MONO)
                                    .child(match &online {
                                        Some(via) => format!("{} · online via {}", d.id, via),
                                        None => format!("{} · last seen {} via {}", d.id, seen, d.last_via),
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("revoke-{}", d.id).into()))
                            .ml_auto()
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(t.mag_deep)
                            .child(tr("撤销"))
                            .on_click(cx.listener(move |this, _, _, cx| this.sync_revoke(id.clone(), cx))),
                    ),
            );
        }
        right = right.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_size(px(12.5))
                        .child(tr("已配对设备")),
                )
                .child(dev_list),
        );

        // audit
        let mut audit_list = div().flex().flex_col().gap(px(3.));
        if audit.is_empty() {
            audit_list = audit_list.child(
                div()
                    .text_size(px(11.5))
                    .text_color(t.faint)
                    .child(tr("还没有放行记录（写入 <common-dir>/sluice/audit.log）")),
            );
        }
        for r in audit {
            let when = chrono::DateTime::from_timestamp(r.at, 0)
                .map(|x| x.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
                .unwrap_or_default();
            let color = match r.decision.as_str() {
                "approve" => t.cyan_deep,
                "reject" => t.mag_deep,
                _ => t.muted,
            };
            audit_list = audit_list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(11.5))
                    .child(div().text_color(color).w(px(52.)).child(r.decision.clone()))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(r.title.clone()),
                    )
                    .child(div().text_color(t.muted).child(r.source_label()))
                    .child(div().text_color(t.faint).font_family(FONT_MONO).child(when)),
            );
        }
        right = right.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_size(px(12.5))
                        .child(tr("最近放行 / 驳回（含来源设备）")),
                )
                .child(audit_list),
        );

        div()
            .flex()
            .flex_col()
            .child(self.panel_header(
                &t,
                tr("移动端 · 配对与设备"),
                tf!("{} · 同网直连优先，中继兜底，端到端加密", desktop_name),
                cx,
            ))
            .child(
                div()
                    .id("devices-body")
                    .flex()
                    .gap(px(18.))
                    .px(px(14.))
                    .py(px(12.))
                    .max_h(px(560.))
                    .overflow_y_scroll()
                    .child(left)
                    .child(right),
            )
    }
}

/// Where the desktop writes the audit log for this repo.
pub fn audit_path(info: &sluice_core::RepoInfo) -> PathBuf {
    common_dir(info).join("sluice").join("audit.log")
}
