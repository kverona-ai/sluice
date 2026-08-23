//! Plan-driven interactive rebase (05 §6). The app writes a [`RebasePlan`] JSON
//! file; git is started with `GIT_SEQUENCE_EDITOR="sluice seq-editor"` and
//! `GIT_EDITOR="sluice editor"`, which read the plan instead of opening a UI.

use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RebaseAction {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

impl RebaseAction {
    pub fn keyword(self) -> &'static str {
        match self {
            RebaseAction::Pick => "pick",
            RebaseAction::Reword => "reword",
            RebaseAction::Squash => "squash",
            RebaseAction::Fixup => "fixup",
            RebaseAction::Drop => "drop",
        }
    }
    pub fn next(self) -> Self {
        match self {
            RebaseAction::Pick => RebaseAction::Reword,
            RebaseAction::Reword => RebaseAction::Squash,
            RebaseAction::Squash => RebaseAction::Fixup,
            RebaseAction::Fixup => RebaseAction::Drop,
            RebaseAction::Drop => RebaseAction::Pick,
        }
    }
    pub fn label_zh(self) -> &'static str {
        match self {
            RebaseAction::Pick => "保留",
            RebaseAction::Reword => "改写信息",
            RebaseAction::Squash => "squash",
            RebaseAction::Fixup => "fixup",
            RebaseAction::Drop => "丢弃",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanItem {
    pub sha: String,
    pub subject: String,
    pub action: RebaseAction,
    /// New full message for `reword` (and the kept message of a squash group).
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RebasePlan {
    /// Final todo order (oldest first).
    pub items: Vec<PlanItem>,
    /// Messages still to hand to `sluice editor`, in the order git will ask:
    /// one per `reword`, one per squash group (None = keep git's default).
    #[serde(default)]
    pub pending_messages: Vec<Option<String>>,
}

impl RebasePlan {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Build the editor message queue from the items (call before starting git).
    pub fn prepare_messages(&mut self) {
        let mut q = Vec::new();
        let mut in_squash = false;
        let mut squash_msg: Option<String> = None;
        for it in &self.items {
            match it.action {
                RebaseAction::Reword => {
                    q.push(Some(it.message.clone().unwrap_or_else(|| it.subject.clone())))
                }
                RebaseAction::Squash => {
                    // git opens one editor for the whole squash group (after the last squash)
                    if !in_squash {
                        in_squash = true;
                        squash_msg = None;
                    }
                    if it.message.is_some() {
                        squash_msg = it.message.clone();
                    }
                }
                _ => {}
            }
            if it.action != RebaseAction::Squash && in_squash {
                in_squash = false;
                q.push(squash_msg.take());
            }
        }
        if in_squash {
            q.push(squash_msg.take());
        }
        self.pending_messages = q;
    }

    /// Rewrite git's todo file: plan order + actions; todo lines not in the plan
    /// are kept at the end untouched (merges, labels, comments).
    pub fn rewrite_todo(&self, todo: &str) -> String {
        let mut lines: Vec<&str> = todo.lines().collect();
        let mut out: Vec<String> = Vec::new();
        for it in &self.items {
            if let Some(pos) = lines.iter().position(|l| {
                let mut parts = l.split_whitespace();
                matches!(
                    parts.next(),
                    Some(
                        "pick"
                            | "p"
                            | "reword"
                            | "r"
                            | "edit"
                            | "e"
                            | "squash"
                            | "s"
                            | "fixup"
                            | "f"
                            | "drop"
                            | "d"
                    )
                ) && parts
                    .next()
                    .is_some_and(|sha| it.sha.starts_with(sha) || sha.starts_with(&it.sha))
            }) {
                let line = lines.remove(pos);
                let rest: Vec<&str> = line.split_whitespace().skip(1).collect();
                out.push(format!("{} {}", it.action.keyword(), rest.join(" ")));
            } else {
                // not in git's list (e.g. already dropped) — skip silently
            }
        }
        for l in lines {
            if !l.trim().is_empty() && !l.starts_with('#') {
                out.push(l.to_string());
            }
        }
        out.join("\n") + "\n"
    }
}

/// `sluice seq-editor <todo>`: rewrite the todo file from `$SLUICE_REBASE_PLAN`.
pub fn run_seq_editor(todo_path: &Path) -> Result<()> {
    let plan_path = std::env::var_os("SLUICE_REBASE_PLAN").context("SLUICE_REBASE_PLAN not set")?;
    let plan = RebasePlan::load(Path::new(&plan_path))?;
    let todo = std::fs::read_to_string(todo_path)?;
    std::fs::write(todo_path, plan.rewrite_todo(&todo))?;
    Ok(())
}

/// `sluice editor <file>`: supply the next queued message (or leave git's default).
pub fn run_editor(file: &Path) -> Result<()> {
    let Some(plan_path) = std::env::var_os("SLUICE_REBASE_PLAN") else {
        return Ok(());
    };
    let plan_path = Path::new(&plan_path);
    let mut plan = RebasePlan::load(plan_path)?;
    if plan.pending_messages.is_empty() {
        return Ok(());
    }
    let next = plan.pending_messages.remove(0);
    if let Some(msg) = next {
        std::fs::write(file, msg.trim_end().to_string() + "\n")?;
    }
    plan.save(plan_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_todo_in_plan_order_with_actions() {
        let plan = RebasePlan {
            items: vec![
                PlanItem {
                    sha: "bbbbbbbbbbbb".into(),
                    subject: "b".into(),
                    action: RebaseAction::Pick,
                    message: None,
                },
                PlanItem {
                    sha: "aaaaaaaaaaaa".into(),
                    subject: "a".into(),
                    action: RebaseAction::Squash,
                    message: None,
                },
                PlanItem {
                    sha: "cccccccccccc".into(),
                    subject: "c".into(),
                    action: RebaseAction::Drop,
                    message: None,
                },
            ],
            pending_messages: vec![],
        };
        let todo = "pick aaaaaaa a\npick bbbbbbb b\npick ccccccc c\n\n# Rebase ...\n";
        let out = plan.rewrite_todo(todo);
        assert_eq!(out, "pick bbbbbbb b\nsquash aaaaaaa a\ndrop ccccccc c\n");
    }

    #[test]
    fn message_queue_matches_git_editor_order() {
        let mut plan = RebasePlan {
            items: vec![
                PlanItem {
                    sha: "1".into(),
                    subject: "one".into(),
                    action: RebaseAction::Reword,
                    message: Some("ONE".into()),
                },
                PlanItem {
                    sha: "2".into(),
                    subject: "two".into(),
                    action: RebaseAction::Pick,
                    message: None,
                },
                PlanItem {
                    sha: "3".into(),
                    subject: "three".into(),
                    action: RebaseAction::Squash,
                    message: None,
                },
                PlanItem {
                    sha: "4".into(),
                    subject: "four".into(),
                    action: RebaseAction::Squash,
                    message: Some("merged".into()),
                },
                PlanItem {
                    sha: "5".into(),
                    subject: "five".into(),
                    action: RebaseAction::Pick,
                    message: None,
                },
            ],
            pending_messages: vec![],
        };
        plan.prepare_messages();
        assert_eq!(
            plan.pending_messages,
            vec![Some("ONE".into()), Some("merged".into())]
        );
    }
}
