//! Forge (GitHub / GitLab) integration for PR review (05 §8): metadata, comments,
//! reviews and merges go through the user's logged-in `gh` / `glab`; the PR diff
//! is fetched locally (`refs/pull/<n>/head` / `refs/merge-requests/<n>/head`) and
//! rendered by Sluice's own diff viewer.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Forge {
    GitHub,
    GitLab,
}

impl Forge {
    pub fn label(self) -> &'static str {
        match self {
            Forge::GitHub => "GitHub",
            Forge::GitLab => "GitLab",
        }
    }
    pub fn noun(self) -> &'static str {
        match self {
            Forge::GitHub => "PR",
            Forge::GitLab => "MR",
        }
    }
    pub fn cli(self) -> &'static str {
        match self {
            Forge::GitHub => "gh",
            Forge::GitLab => "glab",
        }
    }
    /// Ref to fetch for a PR head (GitHub: refs/pull/N/head, GitLab: refs/merge-requests/N/head).
    pub fn head_ref(self, n: u64) -> String {
        match self {
            Forge::GitHub => format!("refs/pull/{n}/head"),
            Forge::GitLab => format!("refs/merge-requests/{n}/head"),
        }
    }
}

/// Detect the forge from the `origin` URL (custom hosts: gh/glab configured hosts are honored
/// by the CLIs themselves; here we only classify by well-known host names + env overrides).
pub fn detect(origin_url: &str) -> Option<Forge> {
    match std::env::var("SLUICE_FORGE").ok().as_deref() {
        Some("github") => return Some(Forge::GitHub),
        Some("gitlab") => return Some(Forge::GitLab),
        _ => {}
    }
    let u = origin_url.to_ascii_lowercase();
    if u.contains("github.com")
        || u.contains(
            &std::env::var("SLUICE_GITHUB_HOST")
                .unwrap_or_else(|_| "github.com".into())
                .to_ascii_lowercase(),
        )
    {
        return Some(Forge::GitHub);
    }
    if u.contains("gitlab")
        || u.contains(
            &std::env::var("SLUICE_GITLAB_HOST")
                .unwrap_or_else(|_| "gitlab.com".into())
                .to_ascii_lowercase(),
        )
    {
        return Some(Forge::GitLab);
    }
    None
}

pub fn cli_path(forge: Forge) -> Option<PathBuf> {
    // Test / dev override (a fake CLI emitting canned JSON).
    if let Some(p) = std::env::var_os("SLUICE_FORGE_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    which::which_in(
        forge.cli(),
        Some(sluice_backend_cli::env::login_path()),
        std::env::current_dir().ok()?,
    )
    .ok()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub head: String,
    pub base: String,
    pub url: String,
    pub draft: bool,
    /// APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED | "" (GitLab: approved / "")
    pub decision: String,
    pub updated_at: String,
    pub additions: u64,
    pub deletions: u64,
    pub body: String,
    /// success | failure | pending | "" (from checks / pipeline)
    pub checks: String,
    pub comments: Vec<PrComment>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PrComment {
    pub author: String,
    pub body: String,
    pub at: String,
    /// "review" | "comment"
    pub kind: String,
    pub state: String,
}

fn run(forge: Forge, cwd: &Path, args: &[&str]) -> Result<String> {
    let exe = cli_path(forge)
        .with_context(|| format!("{} not found on PATH (install + login first)", forge.cli()))?;
    let out = Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .env("PATH", sluice_backend_cli::env::login_path())
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .output()
        .with_context(|| format!("running {}", forge.cli()))?;
    if !out.status.success() {
        bail!(
            "{} {}: {}",
            forge.cli(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn auth_status(forge: Forge, cwd: &Path) -> Result<String> {
    match forge {
        Forge::GitHub => {
            run(forge, cwd, &["auth", "status"]).map(|s| s.lines().next().unwrap_or("").trim().to_string())
        }
        Forge::GitLab => {
            run(forge, cwd, &["auth", "status"]).map(|s| s.lines().next().unwrap_or("").trim().to_string())
        }
    }
}

pub fn list(forge: Forge, cwd: &Path, limit: usize) -> Result<Vec<PullRequest>> {
    let lim = limit.to_string();
    match forge {
        Forge::GitHub => {
            let json = run(
                forge,
                cwd,
                &[
                    "pr",
                    "list",
                    "--state",
                    "open",
                    "--limit",
                    &lim,
                    "--json",
                    "number,title,author,headRefName,baseRefName,url,isDraft,reviewDecision,updatedAt,additions,deletions",
                ],
            )?;
            let v: Vec<Value> = serde_json::from_str(&json)?;
            Ok(v.into_iter()
                .map(|p| PullRequest {
                    number: p["number"].as_u64().unwrap_or(0),
                    title: p["title"].as_str().unwrap_or("").into(),
                    author: p["author"]["login"].as_str().unwrap_or("").into(),
                    head: p["headRefName"].as_str().unwrap_or("").into(),
                    base: p["baseRefName"].as_str().unwrap_or("").into(),
                    url: p["url"].as_str().unwrap_or("").into(),
                    draft: p["isDraft"].as_bool().unwrap_or(false),
                    decision: p["reviewDecision"].as_str().unwrap_or("").into(),
                    updated_at: p["updatedAt"].as_str().unwrap_or("").into(),
                    additions: p["additions"].as_u64().unwrap_or(0),
                    deletions: p["deletions"].as_u64().unwrap_or(0),
                    ..Default::default()
                })
                .collect())
        }
        Forge::GitLab => {
            let json = run(
                forge,
                cwd,
                &["mr", "list", "--output", "json", "--per-page", &lim],
            )?;
            let v: Vec<Value> = serde_json::from_str(&json)?;
            Ok(v.into_iter()
                .map(|p| PullRequest {
                    number: p["iid"].as_u64().unwrap_or(0),
                    title: p["title"].as_str().unwrap_or("").into(),
                    author: p["author"]["username"].as_str().unwrap_or("").into(),
                    head: p["source_branch"].as_str().unwrap_or("").into(),
                    base: p["target_branch"].as_str().unwrap_or("").into(),
                    url: p["web_url"].as_str().unwrap_or("").into(),
                    draft: p["draft"]
                        .as_bool()
                        .or_else(|| p["work_in_progress"].as_bool())
                        .unwrap_or(false),
                    decision: p["detailed_merge_status"].as_str().unwrap_or("").into(),
                    updated_at: p["updated_at"].as_str().unwrap_or("").into(),
                    ..Default::default()
                })
                .collect())
        }
    }
}

/// Body, comments, reviews and check status for one PR.
pub fn view(forge: Forge, cwd: &Path, n: u64) -> Result<PullRequest> {
    let ns = n.to_string();
    match forge {
        Forge::GitHub => {
            let json = run(
                forge,
                cwd,
                &[
                    "pr",
                    "view",
                    &ns,
                    "--json",
                    "number,title,author,headRefName,baseRefName,url,isDraft,reviewDecision,updatedAt,additions,deletions,body,comments,reviews,statusCheckRollup",
                ],
            )?;
            let p: Value = serde_json::from_str(&json)?;
            let mut comments: Vec<PrComment> = p["comments"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| PrComment {
                            author: c["author"]["login"].as_str().unwrap_or("").into(),
                            body: c["body"].as_str().unwrap_or("").into(),
                            at: c["createdAt"].as_str().unwrap_or("").into(),
                            kind: "comment".into(),
                            state: String::new(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Some(a) = p["reviews"].as_array() {
                comments.extend(a.iter().map(|c| PrComment {
                    author: c["author"]["login"].as_str().unwrap_or("").into(),
                    body: c["body"].as_str().unwrap_or("").into(),
                    at: c["submittedAt"].as_str().unwrap_or("").into(),
                    kind: "review".into(),
                    state: c["state"].as_str().unwrap_or("").into(),
                }));
            }
            comments.sort_by(|a, b| a.at.cmp(&b.at));
            let checks = p["statusCheckRollup"]
                .as_array()
                .map(|a| {
                    let states: Vec<String> = a
                        .iter()
                        .map(|c| {
                            c["conclusion"]
                                .as_str()
                                .or_else(|| c["state"].as_str())
                                .unwrap_or("")
                                .to_ascii_lowercase()
                        })
                        .collect();
                    if states
                        .iter()
                        .any(|s| s == "failure" || s == "error" || s == "cancelled")
                    {
                        "failure".to_string()
                    } else if states
                        .iter()
                        .any(|s| s.is_empty() || s == "pending" || s == "in_progress" || s == "queued")
                    {
                        "pending".to_string()
                    } else if states.is_empty() {
                        String::new()
                    } else {
                        "success".to_string()
                    }
                })
                .unwrap_or_default();
            Ok(PullRequest {
                number: p["number"].as_u64().unwrap_or(n),
                title: p["title"].as_str().unwrap_or("").into(),
                author: p["author"]["login"].as_str().unwrap_or("").into(),
                head: p["headRefName"].as_str().unwrap_or("").into(),
                base: p["baseRefName"].as_str().unwrap_or("").into(),
                url: p["url"].as_str().unwrap_or("").into(),
                draft: p["isDraft"].as_bool().unwrap_or(false),
                decision: p["reviewDecision"].as_str().unwrap_or("").into(),
                updated_at: p["updatedAt"].as_str().unwrap_or("").into(),
                additions: p["additions"].as_u64().unwrap_or(0),
                deletions: p["deletions"].as_u64().unwrap_or(0),
                body: p["body"].as_str().unwrap_or("").into(),
                checks,
                comments,
            })
        }
        Forge::GitLab => {
            let json = run(forge, cwd, &["mr", "view", &ns, "--output", "json", "--comments"])?;
            let p: Value = serde_json::from_str(&json)?;
            let comments = p["Notes"]
                .as_array()
                .or_else(|| p["notes"].as_array())
                .map(|a| {
                    a.iter()
                        .filter(|c| !c["system"].as_bool().unwrap_or(false))
                        .map(|c| PrComment {
                            author: c["author"]["username"].as_str().unwrap_or("").into(),
                            body: c["body"].as_str().unwrap_or("").into(),
                            at: c["created_at"].as_str().unwrap_or("").into(),
                            kind: "comment".into(),
                            state: String::new(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let checks = p["head_pipeline"]["status"].as_str().unwrap_or("").to_string();
            Ok(PullRequest {
                number: p["iid"].as_u64().unwrap_or(n),
                title: p["title"].as_str().unwrap_or("").into(),
                author: p["author"]["username"].as_str().unwrap_or("").into(),
                head: p["source_branch"].as_str().unwrap_or("").into(),
                base: p["target_branch"].as_str().unwrap_or("").into(),
                url: p["web_url"].as_str().unwrap_or("").into(),
                draft: p["draft"].as_bool().unwrap_or(false),
                decision: p["detailed_merge_status"].as_str().unwrap_or("").into(),
                updated_at: p["updated_at"].as_str().unwrap_or("").into(),
                body: p["description"].as_str().unwrap_or("").into(),
                checks: match checks.as_str() {
                    "success" => "success".into(),
                    "failed" | "canceled" => "failure".into(),
                    "" => String::new(),
                    _ => "pending".into(),
                },
                comments,
                ..Default::default()
            })
        }
    }
}

pub fn checkout(forge: Forge, cwd: &Path, n: u64) -> Result<String> {
    let ns = n.to_string();
    match forge {
        Forge::GitHub => run(forge, cwd, &["pr", "checkout", &ns]),
        Forge::GitLab => run(forge, cwd, &["mr", "checkout", &ns]),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

pub fn review(forge: Forge, cwd: &Path, n: u64, verdict: ReviewVerdict, body: &str) -> Result<String> {
    let ns = n.to_string();
    match forge {
        Forge::GitHub => {
            let flag = match verdict {
                ReviewVerdict::Approve => "--approve",
                ReviewVerdict::RequestChanges => "--request-changes",
                ReviewVerdict::Comment => "--comment",
            };
            let mut args = vec!["pr", "review", &ns, flag];
            if !body.trim().is_empty() || verdict != ReviewVerdict::Approve {
                args.push("--body");
                args.push(body);
            }
            run(forge, cwd, &args)
        }
        Forge::GitLab => match verdict {
            ReviewVerdict::Approve => {
                if !body.trim().is_empty() {
                    run(forge, cwd, &["mr", "note", &ns, "-m", body])?;
                }
                run(forge, cwd, &["mr", "approve", &ns])
            }
            ReviewVerdict::RequestChanges | ReviewVerdict::Comment => {
                run(forge, cwd, &["mr", "note", &ns, "-m", body])
            }
        },
    }
}

pub fn merge(forge: Forge, cwd: &Path, n: u64, squash: bool) -> Result<String> {
    let ns = n.to_string();
    match forge {
        Forge::GitHub => {
            let mut args = vec!["pr", "merge", &ns, "--delete-branch"];
            args.push(if squash { "--squash" } else { "--merge" });
            run(forge, cwd, &args)
        }
        Forge::GitLab => {
            let mut args = vec!["mr", "merge", &ns, "--yes", "--remove-source-branch"];
            if squash {
                args.push("--squash");
            }
            run(forge, cwd, &args)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_forges() {
        assert_eq!(
            detect("git@github.com:kverona-ai/sluice.git"),
            Some(Forge::GitHub)
        );
        assert_eq!(detect("https://gitlab.com/g/p.git"), Some(Forge::GitLab));
        assert_eq!(detect("https://example.com/x.git"), None);
        assert_eq!(Forge::GitHub.head_ref(12), "refs/pull/12/head");
    }
}
