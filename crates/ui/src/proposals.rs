//! The propose-and-confirm queue (03 §3, 05 §7.1): proposals arrive over IPC from
//! `sluice mcp serve`, wait here for a human — on this desktop or on a paired
//! phone through the sync channel — and are executed only on Accept. Every
//! decision is appended to `<common-dir>/sluice/audit.log` with who decided
//! (本机 / 设备名) and a proposal expires when the repository moved away from
//! the baseline it was made against. Also hosts the askpass dialog fed by
//! `sluice askpass`.

use crate::i18n::tr;
use std::sync::mpsc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use gpui_component::input::Input;
use sluice_backend_cli::CommitOptions;
use sluice_bridge::ipc::{Decision, Inbound, Proposal, ProposalKind};
use sluice_sync::proto::{DecisionRecord, DeviceInfo, DomainEvent, ReviewItem};
use sluice_sync::server::DecisionOutcome;

use crate::icons::icon_b;
use crate::overlays::Overlay;
use crate::theme::FONT_MONO;
use crate::workbench::Workbench;

pub struct PendingProposal {
    pub proposal: Proposal,
    pub reply: Option<mpsc::Sender<Decision>>,
    pub busy: bool,
    /// Fingerprint of HEAD + the files the proposal touches, taken on arrival
    /// (05 §7.1 基线指纹); empty until computed.
    pub baseline: String,
    /// Unified diff of what a commit proposal would include (for reviewers on a phone).
    pub patch: Option<String>,
    /// Files touched (commit proposals), computed with the baseline.
    pub files: Vec<String>,
    /// Set when a device is waiting for the outcome of its decision.
    pub on_done: Option<mpsc::Sender<DecisionOutcome>>,
}

/// Who released / rejected a proposal (05 §7.1 决定人).
#[derive(Clone, Debug, PartialEq)]
pub enum DecisionSource {
    Desktop,
    Device(DeviceInfo),
}

impl DecisionSource {
    pub fn key(&self) -> String {
        match self {
            DecisionSource::Desktop => "desktop".into(),
            DecisionSource::Device(d) => format!("device:{}", d.id),
        }
    }
    pub fn name(&self) -> String {
        match self {
            DecisionSource::Desktop => "desktop".into(),
            DecisionSource::Device(d) => d.name.clone(),
        }
    }
}

impl PendingProposal {
    pub fn kind_key(&self) -> &'static str {
        match self.proposal.kind {
            ProposalKind::Commit { .. } => "commit",
            ProposalKind::Branch { .. } => "branch",
            ProposalKind::Push { .. } => "push",
        }
    }

    /// The read-model row a paired device sees.
    pub fn review_item(&self) -> ReviewItem {
        let detail = match &self.proposal.kind {
            ProposalKind::Commit { message, .. } => message.clone(),
            ProposalKind::Branch { name, checkout } => {
                format!("{name}{}", if *checkout { " (checkout)" } else { "" })
            }
            ProposalKind::Push { set_upstream } => {
                if *set_upstream {
                    "push -u".into()
                } else {
                    "push".into()
                }
            }
        };
        ReviewItem {
            id: self.proposal.id,
            client: self.proposal.client.clone(),
            kind: self.kind_key().to_string(),
            title: self.proposal.kind.title(),
            detail,
            files: self.files.clone(),
            patch: self.patch.clone(),
            received_at: self.proposal.received_at,
            version: self.baseline.clone(),
            state: if self.busy { "busy" } else { "pending" }.to_string(),
        }
    }
}

/// Baseline fingerprint + patch + file list of a proposal (blocking; background).
fn snapshot_proposal(
    cli: &sluice_backend_cli::GitCli,
    kind: &ProposalKind,
) -> (String, Option<String>, Vec<String>) {
    let head = cli
        .run_read(&["rev-parse", "HEAD"])
        .map(|o| o.stdout_str().trim().to_string())
        .unwrap_or_default();
    match kind {
        ProposalKind::Commit { paths, .. } => {
            let mut args: Vec<String> = vec!["diff".into(), "HEAD".into(), "--".into()];
            let mut status_args: Vec<String> = vec!["status".into(), "--porcelain".into(), "--".into()];
            if let Some(p) = paths {
                args.extend(p.iter().cloned());
                status_args.extend(p.iter().cloned());
            }
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let patch = cli
                .run_read(&refs)
                .map(|o| o.stdout_str().to_string())
                .unwrap_or_default();
            let refs: Vec<&str> = status_args.iter().map(String::as_str).collect();
            let status = cli
                .run_read(&refs)
                .map(|o| o.stdout_str().to_string())
                .unwrap_or_default();
            let files: Vec<String> = match paths {
                Some(p) => p.clone(),
                None => status
                    .lines()
                    .filter_map(|l| l.get(3..).map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect(),
            };
            let fp = crate::sync::short_hash(&(head.as_str(), patch.as_str(), status.as_str()));
            let mut p = patch;
            if p.len() > 64 * 1024 {
                p.truncate(64 * 1024);
                p.push_str("\n… (truncated)\n");
            }
            (fp, (!p.is_empty()).then_some(p), files)
        }
        _ => (crate::sync::short_hash(&head), None, Vec::new()),
    }
}

impl Workbench {
    /// Start consuming IPC messages (called once by the app after construction).
    pub fn attach_ipc(&mut self, rx: async_channel::Receiver<Inbound>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let ok = this.update(cx, |this, cx| match msg {
                    Inbound::Proposal { proposal, reply } => {
                        let title = proposal.kind.title();
                        let client = proposal.client.clone();
                        let id = proposal.id;
                        let kind = proposal.kind.clone();
                        this.proposals.push(PendingProposal {
                            proposal,
                            reply: Some(reply),
                            busy: false,
                            baseline: String::new(),
                            patch: None,
                            files: Vec::new(),
                            on_done: None,
                        });
                        this.repo.console.note("proposal", format!("{client}: {title}"));
                        this.toast(tf!("收到 {} 的提议：{}（⌘⇧P 查看队列）", client, title), cx);
                        this.sync_publish();
                        if let Some(cli) = this.repo.cli.clone() {
                            cx.spawn(async move |this, cx| {
                                let snap = cx
                                    .background_spawn(async move { snapshot_proposal(&cli, &kind) })
                                    .await;
                                this.update(cx, |this, cx| {
                                    if let Some(p) = this.proposals.iter_mut().find(|p| p.proposal.id == id) {
                                        p.baseline = snap.0;
                                        p.patch = snap.1;
                                        p.files = snap.2;
                                        let item = p.review_item();
                                        let pending = this.proposals.len() as u32;
                                        this.sync_publish();
                                        this.sync_event(DomainEvent::Proposed { item, pending });
                                    }
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                        }
                        cx.notify();
                    }
                    Inbound::Askpass { prompt, reply } => {
                        this.pending_askpass_prompt = Some((prompt, reply));
                        this.overlay = Some(Overlay::Askpass);
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

    pub(crate) fn open_proposals(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Proposals);
        cx.notify();
    }

    pub(crate) fn decide_proposal(&mut self, ix: usize, accept: bool, cx: &mut Context<Self>) {
        self.decide_proposal_from(ix, accept, DecisionSource::Desktop, String::new(), None, cx);
    }

    /// Append to `<common-dir>/sluice/audit.log`, echo to Console, notify devices.
    fn record_decision(&mut self, rec: DecisionRecord) {
        let path = crate::sync::common_dir(&self.repo.info);
        if let Err(e) = sluice_sync::store::append_audit(&path, &rec) {
            self.repo
                .console
                .note("audit", format!("cannot write audit.log: {e:#}"));
        }
        self.repo.console.note(
            "audit",
            format!(
                "{} · {} · by {}{}{}",
                rec.decision,
                rec.title,
                rec.source_label(),
                if rec.note.is_empty() {
                    String::new()
                } else {
                    format!(" · “{}”", rec.note)
                },
                if rec.detail.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", rec.detail)
                }
            ),
        );
        let pending = self.proposals.len() as u32;
        self.sync_event(DomainEvent::Decided {
            id: rec.proposal_id,
            accepted: rec.decision == "approve",
            outcome: if rec.decision == "expire" {
                "expired".into()
            } else if rec.decision == "approve" {
                if rec.commit.is_empty() && rec.detail.starts_with("execution failed") {
                    "failed".into()
                } else {
                    "done".into()
                }
            } else {
                "rejected".into()
            },
            detail: rec.detail.clone(),
            source: rec.source.clone(),
            source_name: rec.source_name.clone(),
            pending,
        });
        self.sync_publish();
    }

    /// Decide a queue item on behalf of `source` (this desktop or a paired device).
    /// `note` is the reviewer's comment; `on_done` gets the outcome once executed.
    pub(crate) fn decide_proposal_from(
        &mut self,
        ix: usize,
        accept: bool,
        source: DecisionSource,
        note: String,
        on_done: Option<mpsc::Sender<DecisionOutcome>>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.proposals.get_mut(ix) else {
            if let Some(d) = on_done {
                let _ = d.send(DecisionOutcome {
                    outcome: "unknown".into(),
                    detail: "no such proposal".into(),
                });
            }
            return;
        };
        if entry.busy {
            if let Some(d) = on_done {
                let _ = d.send(DecisionOutcome {
                    outcome: "failed".into(),
                    detail: "already executing".into(),
                });
            }
            return;
        }
        let Some(reply) = entry.reply.take() else { return };
        let title = entry.proposal.kind.title();
        let kind_key = entry.kind_key().to_string();
        let agent = entry.proposal.client.clone();
        let payload_hash =
            crate::sync::short_hash(&serde_json::to_string(&entry.proposal.kind).unwrap_or_default());
        let id = entry.proposal.id;
        let now = sluice_sync::pairing::now();
        if !accept {
            let _ = reply.send(Decision::Rejected {
                reason: format!(
                    "rejected by the user in Sluice ({}){}",
                    source.name(),
                    if note.is_empty() {
                        String::new()
                    } else {
                        format!(": {note}")
                    }
                ),
            });
            self.proposals.remove(ix);
            self.record_decision(DecisionRecord {
                at: now,
                proposal_id: id,
                agent,
                kind: kind_key,
                title: title.clone(),
                payload_hash,
                decision: "reject".into(),
                source: source.key(),
                source_name: source.name(),
                note,
                detail: String::new(),
                commit: String::new(),
            });
            if let Some(d) = on_done {
                let _ = d.send(DecisionOutcome {
                    outcome: "rejected".into(),
                    detail: "rejected".into(),
                });
            }
            self.toast(
                match &source {
                    DecisionSource::Desktop => tf!("已拒绝：{}", title),
                    DecisionSource::Device(dv) => tf!("已拒绝（来自 {}）：{}", dv.name, title),
                },
                cx,
            );
            cx.notify();
            return;
        }
        entry.busy = true;
        entry.on_done = on_done;
        let kind = entry.proposal.kind.clone();
        let baseline = entry.baseline.clone();
        let Some(cli) = self.repo.cli.clone() else {
            let _ = reply.send(Decision::Rejected {
                reason: "bare repository".into(),
            });
            if let Some(d) = entry.on_done.take() {
                let _ = d.send(DecisionOutcome {
                    outcome: "failed".into(),
                    detail: "bare repository".into(),
                });
            }
            return;
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let kind2 = kind.clone();
            let res: anyhow::Result<String> = cx
                .background_spawn(async move {
                    // 05 §7.1: baseline moved → expired, never executed.
                    if !baseline.is_empty() {
                        let (now_fp, _, _) = snapshot_proposal(&cli, &kind2);
                        if now_fp != baseline {
                            anyhow::bail!("EXPIRED");
                        }
                    }
                    match kind2 {
                        ProposalKind::Commit { message, paths } => {
                            match paths {
                                Some(p) => {
                                    let refs: Vec<&str> = p.iter().map(String::as_str).collect();
                                    cli.stage(&refs)?;
                                }
                                None => {
                                    cli.run(&["add", "-u"])?;
                                }
                            }
                            cli.commit(&message, &CommitOptions::default())?;
                            let out = cli.run_read(&["rev-parse", "--short", "HEAD"])?;
                            Ok(format!("committed {}", out.stdout_str().trim()))
                        }
                        ProposalKind::Branch { name, checkout } => {
                            cli.branch_create(&name, None, checkout)?;
                            Ok(format!("branch {name} created"))
                        }
                        ProposalKind::Push { set_upstream } => {
                            let out = cli.push(None, None, set_upstream, false)?;
                            Ok(out.stderr.lines().last().unwrap_or("pushed").trim().to_string())
                        }
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                let expired = matches!(&res, Err(e) if e.to_string() == "EXPIRED");
                let decision = match &res {
                    Ok(detail) => Decision::Accepted {
                        detail: detail.clone(),
                    },
                    Err(_) if expired => Decision::Rejected {
                        reason: "expired: the repository changed since the proposal was made".into(),
                    },
                    Err(e) => Decision::Rejected {
                        reason: format!("execution failed: {e:#}"),
                    },
                };
                let _ = reply.send(decision);
                if let Some(pos) = this.proposals.iter().position(|p| p.proposal.id == id) {
                    let mut entry = this.proposals.remove(pos);
                    let title = entry.proposal.kind.title();
                    let (outcome, detail, commit) = match &res {
                        Ok(detail) => (
                            "done".to_string(),
                            detail.clone(),
                            detail.strip_prefix("committed ").unwrap_or("").to_string(),
                        ),
                        Err(_) if expired => (
                            "expired".to_string(),
                            "baseline changed".to_string(),
                            String::new(),
                        ),
                        Err(e) => (
                            "failed".to_string(),
                            format!("execution failed: {e:#}"),
                            String::new(),
                        ),
                    };
                    if let Some(d) = entry.on_done.take() {
                        let _ = d.send(DecisionOutcome {
                            outcome: outcome.clone(),
                            detail: detail.clone(),
                        });
                    }
                    this.record_decision(DecisionRecord {
                        at: now,
                        proposal_id: id,
                        agent,
                        kind: kind_key,
                        title: title.clone(),
                        payload_hash,
                        decision: if expired {
                            "expire".into()
                        } else {
                            "approve".into()
                        },
                        source: source.key(),
                        source_name: source.name(),
                        note,
                        detail: detail.clone(),
                        commit,
                    });
                    let by = match &source {
                        DecisionSource::Desktop => String::new(),
                        DecisionSource::Device(dv) => format!("（来自 {}）", dv.name),
                    };
                    match outcome.as_str() {
                        "done" => this.toast(tf!("已执行{}：{} · {}", by, title, detail), cx),
                        "expired" => this.toast(tf!("提议已过期：{} — 仓库状态已变化，未执行", title), cx),
                        _ => this.toast(tf!("执行失败：{}", detail.lines().next().unwrap_or("")), cx),
                    }
                }
                this.refresh(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_proposals(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let n = self.proposals.len();
        let mut list = div()
            .id("proposal-list")
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .py(px(8.))
            .max_h(px(420.))
            .overflow_y_scroll();
        if n == 0 {
            list = list.child(
                div().py(px(18.)).text_size(px(12.5)).text_color(t.muted).text_center().child(
                    tr("队列为空。AI 工具通过 MCP 的 propose_commit / propose_branch / propose_push 提交提议后会出现在这里，由你决定是否执行。"),
                ),
            );
        }
        for (i, p) in self.proposals.iter().enumerate() {
            let when = chrono::DateTime::from_timestamp(p.proposal.received_at, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
                .unwrap_or_default();
            let busy = p.busy;
            let detail: Option<String> = match &p.proposal.kind {
                ProposalKind::Commit { message, paths } => Some(format!(
                    "{}{}",
                    message,
                    paths
                        .as_ref()
                        .map(|ps| format!("\n\n文件：{}", ps.join(", ")))
                        .unwrap_or_default()
                )),
                _ => None,
            };
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
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
                            .child(icon_b("sparkle", px(13.), t.mag))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(13.))
                                    .child(p.proposal.kind.title()),
                            )
                            .child(
                                div()
                                    .ml_auto()
                                    .text_size(px(11.))
                                    .text_color(t.faint)
                                    .child(format!("{} · {}", p.proposal.client, when)),
                            ),
                    )
                    .when_some(detail, |d, text| {
                        d.child(
                            div()
                                .px(px(8.))
                                .py(px(6.))
                                .rounded(px(5.))
                                .bg(t.ink_05)
                                .font_family(FONT_MONO)
                                .text_size(px(11.5))
                                .line_height(px(17.))
                                .whitespace_normal()
                                .child(text),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(t.faint)
                                    .child(tr("接受后由 Sluice 用你的 git 执行；拒绝会把结果告知调用方")),
                            )
                            .child(div().ml_auto())
                            .child(
                                div()
                                    .id(("prop-reject", i))
                                    .px(px(12.))
                                    .py(px(4.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(t.line)
                                    .text_size(px(12.))
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.ink_05))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.decide_proposal(i, false, cx)),
                                    )
                                    .child(tr("拒绝")),
                            )
                            .child(
                                div()
                                    .id(("prop-accept", i))
                                    .px(px(14.))
                                    .py(px(4.))
                                    .rounded(px(4.))
                                    .bg(t.cyan)
                                    .text_color(t.surface)
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .when(busy, |d| d.opacity(0.5))
                                    .hover(move |s| s.bg(t.cyan_deep))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.decide_proposal(i, true, cx)),
                                    )
                                    .child(if busy {
                                        tr("执行中…")
                                    } else {
                                        tr("接受并执行")
                                    }),
                            ),
                    ),
            );
        }
        div()
            .w(px(560.))
            .flex()
            .flex_col()
            .child(self.panel_header(
                &t,
                tr("待确认队列"),
                tf!("{} 条提议 · AI 只能提议，放行由你点下", n),
                cx,
            ))
            .child(list)
    }

    // ----- askpass ------------------------------------------------------------

    pub(crate) fn render_askpass(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = self.theme;
        let (prompt, _) = self.pending_askpass_prompt.as_ref()?;
        let prompt = prompt.clone();
        let input = self.askpass_input.clone();
        Some(
            div()
                .w(px(420.))
                .flex()
                .flex_col()
                .child(self.panel_header(&t, tr("git 需要凭据"), tr("来自 git / ssh 的 askpass 请求").into(), cx))
                .child(
                    div()
                        .px(px(16.))
                        .py(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(div().text_size(px(12.5)).child(prompt))
                        .child(
                            div()
                                .border_1()
                                .border_color(t.cyan)
                                .bg(t.surface)
                                .rounded(px(6.))
                                .px(px(9.))
                                .py(px(3.))
                                .text_size(px(12.5))
                                .child(Input::new(&input).appearance(false)),
                        )
                        .child(div().text_size(px(11.)).text_color(t.faint).child(tr("输入不会被 Sluice 保存；推荐使用系统 credential helper / ssh-agent 以免反复询问。"))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(16.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(t.line_soft)
                        .child(div().ml_auto())
                        .child(
                            div()
                                .id("askpass-cancel")
                                .px(px(14.))
                                .py(px(5.))
                                .border_1()
                                .border_color(t.line)
                                .rounded(px(4.))
                                .text_size(px(12.5))
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.ink_05))
                                .on_click(cx.listener(|this, _, window, cx| this.finish_askpass(false, window, cx)))
                                .child(tr("取消")),
                        )
                        .child(
                            div()
                                .id("askpass-ok")
                                .px(px(16.))
                                .py(px(5.))
                                .bg(t.cyan)
                                .text_color(t.surface)
                                .rounded(px(4.))
                                .text_size(px(12.5))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.cyan_deep))
                                .on_click(cx.listener(|this, _, window, cx| this.finish_askpass(true, window, cx)))
                                .child(tr("确定")),
                        ),
                ),
        )
    }

    pub(crate) fn finish_askpass(&mut self, ok: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, reply)) = self.pending_askpass_prompt.take() {
            let secret = if ok {
                Some(self.askpass_input.read(cx).value().to_string())
            } else {
                None
            };
            let _ = reply.send(secret);
        }
        self.askpass_input.update(cx, |s, cx| s.set_value("", window, cx));
        self.overlay = None;
        cx.notify();
    }

    /// Esc / backdrop: answer "no secret" so git fails fast instead of hanging.
    pub(crate) fn cancel_askpass_silent(&mut self) {
        if let Some((_, reply)) = self.pending_askpass_prompt.take() {
            let _ = reply.send(None);
        }
    }
}
