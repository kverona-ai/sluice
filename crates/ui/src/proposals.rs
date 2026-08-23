//! The propose-and-confirm queue (03 §3): proposals arrive over IPC from
//! `sluice mcp serve`, wait here for a human, and are executed only on Accept.
//! Also hosts the askpass dialog fed by `sluice askpass`.

use std::sync::mpsc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use gpui_component::input::Input;
use sluice_backend_cli::CommitOptions;
use sluice_bridge::ipc::{Decision, Inbound, Proposal, ProposalKind};

use crate::icons::icon_b;
use crate::overlays::Overlay;
use crate::theme::FONT_MONO;
use crate::workbench::Workbench;

pub struct PendingProposal {
    pub proposal: Proposal,
    pub reply: Option<mpsc::Sender<Decision>>,
    pub busy: bool,
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
                        this.proposals.push(PendingProposal {
                            proposal,
                            reply: Some(reply),
                            busy: false,
                        });
                        this.repo.console.note("proposal", format!("{client}: {title}"));
                        this.toast(format!("收到 {client} 的提议：{title}（⌘⇧P 查看队列）"), cx);
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
        let Some(entry) = self.proposals.get_mut(ix) else {
            return;
        };
        if entry.busy {
            return;
        }
        let Some(reply) = entry.reply.take() else { return };
        if !accept {
            let _ = reply.send(Decision::Rejected {
                reason: "rejected by the user in Sluice".into(),
            });
            let title = entry.proposal.kind.title();
            self.proposals.remove(ix);
            self.repo.console.note("proposal rejected", title.clone());
            self.toast(format!("已拒绝：{title}"), cx);
            cx.notify();
            return;
        }
        entry.busy = true;
        let id = entry.proposal.id;
        let kind = entry.proposal.kind.clone();
        let Some(cli) = self.repo.cli.clone() else {
            let _ = reply.send(Decision::Rejected {
                reason: "bare repository".into(),
            });
            return;
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res: anyhow::Result<String> = cx
                .background_spawn(async move {
                    match kind {
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
                let decision = match &res {
                    Ok(detail) => Decision::Accepted {
                        detail: detail.clone(),
                    },
                    Err(e) => Decision::Rejected {
                        reason: format!("execution failed: {e:#}"),
                    },
                };
                let _ = reply.send(decision);
                if let Some(pos) = this.proposals.iter().position(|p| p.proposal.id == id) {
                    let title = this.proposals[pos].proposal.kind.title();
                    this.proposals.remove(pos);
                    match res {
                        Ok(detail) => {
                            this.repo
                                .console
                                .note("proposal accepted", format!("{title} → {detail}"));
                            this.toast(format!("已执行：{title} · {detail}"), cx);
                        }
                        Err(e) => {
                            let msg = format!("{e:#}");
                            this.toast(format!("执行失败：{}", msg.lines().next().unwrap_or("")), cx)
                        }
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
                    "队列为空。AI 工具通过 MCP 的 propose_commit / propose_branch / propose_push 提交提议后会出现在这里，由你决定是否执行。",
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
                                    .child("接受后由 Sluice 用你的 git 执行；拒绝会把结果告知调用方"),
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
                                    .child("拒绝"),
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
                                    .child(if busy { "执行中…" } else { "接受并执行" }),
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
                "待确认队列",
                format!("{n} 条提议 · AI 只能提议，放行由你点下"),
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
                .child(self.panel_header(&t, "git 需要凭据", "来自 git / ssh 的 askpass 请求".into(), cx))
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
                        .child(div().text_size(px(11.)).text_color(t.faint).child("输入不会被 Sluice 保存；推荐使用系统 credential helper / ssh-agent 以免反复询问。")),
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
                                .child("取消"),
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
                                .child("确定"),
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
