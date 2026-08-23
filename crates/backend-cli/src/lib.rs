//! sluice-backend-cli — the write / network path of the `GitBackend` (02 §2):
//! every mutating or networked operation shells out to the user's `git` so
//! hooks, signing, credential helpers and enterprise config behave exactly as
//! in the terminal, and every command is echoed to the Console (05 §3/§4).
//!
//! Also hosts `git status --porcelain=v2` parsing (worktree status) and the
//! login-shell PATH resolution GUI processes need on macOS / Linux.

pub mod env;
pub mod status;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use chrono::Local;
use sluice_core::*;

pub use status::parse_porcelain_v2;

#[derive(Clone, Debug)]
pub struct StashEntry {
    /// e.g. `stash@{0}`
    pub id: String,
    pub sha: String,
    pub time: i64,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct FileHistoryEntry {
    pub sha: String,
    pub author: String,
    pub time: i64,
    pub subject: String,
}

#[derive(Clone, Debug)]
pub struct BlameLine {
    pub sha: String,
    pub author: String,
    pub time: i64,
    pub summary: String,
    pub line_no: usize,
    pub text: String,
}

/// Parse `git blame --porcelain` output. Per-commit metadata only appears the
/// first time a commit is seen, so it is cached while scanning.
pub fn parse_blame_porcelain(out: &str) -> Vec<BlameLine> {
    use std::collections::HashMap;
    #[derive(Default, Clone)]
    struct Meta {
        author: String,
        time: i64,
        summary: String,
    }
    let mut metas: HashMap<String, Meta> = HashMap::new();
    let mut lines = Vec::new();
    let mut cur_sha = String::new();
    let mut cur_line = 0usize;
    for raw in out.lines() {
        if let Some(text) = raw.strip_prefix('\t') {
            let m = metas.get(&cur_sha).cloned().unwrap_or_default();
            lines.push(BlameLine {
                sha: cur_sha.clone(),
                author: m.author,
                time: m.time,
                summary: m.summary,
                line_no: cur_line,
                text: text.to_string(),
            });
            continue;
        }
        let mut it = raw.splitn(2, ' ');
        let key = it.next().unwrap_or("");
        let val = it.next().unwrap_or("");
        if key.len() == 40 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
            cur_sha = key.to_string();
            // "<sha> <orig_line> <final_line> [<group_len>]"
            cur_line = val.split(' ').nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
            metas.entry(cur_sha.clone()).or_default();
            continue;
        }
        if let Some(m) = metas.get_mut(&cur_sha) {
            match key {
                "author" => m.author = val.to_string(),
                "author-time" => m.time = val.parse().unwrap_or(0),
                "summary" => m.summary = val.to_string(),
                _ => {}
            }
        }
    }
    lines
}

#[derive(Clone, Debug)]
pub struct SnapshotEntry {
    pub refname: String,
    pub sha: String,
    pub time: i64,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct CliOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl CliOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

#[derive(Clone, Debug, Default)]
pub struct CommitOptions {
    pub amend: bool,
    pub signoff: bool,
    pub no_verify: bool,
    /// `Name <email>` override.
    pub author: Option<String>,
    /// Some(true) = force sign, Some(false) = `--no-gpg-sign`, None = follow config.
    pub sign: Option<bool>,
}

/// Runs `git` inside one working tree.
#[derive(Clone)]
pub struct GitCli {
    workdir: PathBuf,
    git_dir: PathBuf,
    git: PathBuf,
    console: Console,
}

impl GitCli {
    pub fn new(workdir: impl Into<PathBuf>, git_dir: impl Into<PathBuf>, console: Console) -> Result<Self> {
        let git = env::find_git().context("git executable not found (05 §3: git ≥ 2.35 is required)")?;
        Ok(Self {
            workdir: workdir.into(),
            git_dir: git_dir.into(),
            git,
            console,
        })
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.git);
        cmd.current_dir(&self.workdir)
            .args(["-c", "color.ui=never", "-c", "core.quotepath=false"])
            .args(args)
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("PATH", env::login_path())
            .env("SLUICE_REPO", &self.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Desktop-provided credential prompts (05 §3): `sluice askpass` asks the app
        // over IPC; git and ssh both route through it when the app set the env var.
        if let Some(exe) = std::env::var_os("SLUICE_ASKPASS_EXE") {
            cmd.env("GIT_ASKPASS", &exe)
                .env("SSH_ASKPASS", &exe)
                .env("SSH_ASKPASS_REQUIRE", "force");
        }
        cmd
    }

    fn log(&self, kind: ConsoleKind, args: &[&str], started: Instant, out: &CliOutput, summary: String) {
        self.console.log(ConsoleEntry {
            at: Local::now(),
            kind,
            command: format!("git {}", shell_join(args)),
            duration_ms: started.elapsed().as_millis(),
            exit_code: Some(out.status),
            summary,
            stderr: out.stderr.clone(),
        });
    }

    /// Run a write / network command; echoed to the Console at command level.
    pub fn run(&self, args: &[&str]) -> Result<CliOutput> {
        self.run_inner(args, None, ConsoleKind::Write)
    }

    /// Run a read command (echoed only at verbose level).
    pub fn run_read(&self, args: &[&str]) -> Result<CliOutput> {
        self.run_inner(args, None, ConsoleKind::Read)
    }

    pub fn run_with_stdin(&self, args: &[&str], stdin: &[u8]) -> Result<CliOutput> {
        self.run_inner(args, Some(stdin), ConsoleKind::Write)
    }

    fn run_inner(&self, args: &[&str], stdin: Option<&[u8]>, kind: ConsoleKind) -> Result<CliOutput> {
        let started = Instant::now();
        let mut cmd = self.command(args);
        if kind == ConsoleKind::Read {
            cmd.env("GIT_OPTIONAL_LOCKS", "0");
        }
        let out = if let Some(input) = stdin {
            cmd.stdin(Stdio::piped());
            let mut child = cmd
                .spawn()
                .with_context(|| format!("spawning git {}", shell_join(args)))?;
            {
                use std::io::Write;
                let mut pipe = child.stdin.take().expect("piped stdin");
                pipe.write_all(input)?;
            }
            child.wait_with_output()?
        } else {
            cmd.output()
                .with_context(|| format!("spawning git {}", shell_join(args)))?
        };
        let result = CliOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        };
        let summary = if result.ok() {
            first_line(&result.stdout_str()).unwrap_or_default()
        } else {
            first_line(&result.stderr).unwrap_or_else(|| format!("exit {}", result.status))
        };
        self.log(kind, args, started, &result, summary);
        Ok(result)
    }

    fn expect_ok(&self, out: CliOutput, what: &str) -> Result<CliOutput> {
        if out.ok() {
            Ok(out)
        } else {
            bail!(
                "{what} failed (exit {}):\n{}",
                out.status,
                if out.stderr.is_empty() {
                    out.stdout_str()
                } else {
                    out.stderr
                }
            )
        }
    }

    // ----- reads -----------------------------------------------------------

    /// `git status --porcelain=v2 -z --branch --untracked-files=all`
    pub fn status(&self) -> Result<WorkStatus> {
        let out = self.run_read(&[
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ])?;
        let out = self.expect_ok(out, "git status")?;
        let mut status = parse_porcelain_v2(&out.stdout);
        status.in_progress = detect_in_progress(&self.git_dir);
        Ok(status)
    }

    pub fn version(&self) -> Result<String> {
        let out = self.run_read(&["--version"])?;
        Ok(out
            .stdout_str()
            .trim()
            .trim_start_matches("git version ")
            .to_string())
    }

    // ----- staging -----------------------------------------------------------

    pub fn stage(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["add", "-A", "--"];
        args.extend(paths);
        self.expect_ok(self.run(&args)?, "git add").map(|_| ())
    }

    pub fn unstage(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["restore", "--staged", "--"];
        args.extend(paths);
        self.expect_ok(self.run(&args)?, "git restore --staged")
            .map(|_| ())
    }

    /// `git add -N` so a new file can be partially staged (05 §5).
    pub fn intent_to_add(&self, paths: &[&str]) -> Result<()> {
        let mut args = vec!["add", "-N", "--"];
        args.extend(paths);
        self.expect_ok(self.run(&args)?, "git add -N").map(|_| ())
    }

    /// Apply a partial patch to the index (line-level staging). `reverse` unstages.
    pub fn apply_cached(&self, patch: &str, reverse: bool) -> Result<()> {
        let mut args = vec!["apply", "--cached", "--unidiff-zero", "--whitespace=nowarn"];
        if reverse {
            args.push("-R");
        }
        args.push("-");
        self.expect_ok(
            self.run_with_stdin(&args, patch.as_bytes())?,
            "git apply --cached",
        )
        .map(|_| ())
    }

    /// Discard worktree changes of tracked files (caller is responsible for the safety snapshot).
    pub fn discard(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["checkout", "--"];
        args.extend(paths);
        self.expect_ok(self.run(&args)?, "git checkout --").map(|_| ())
    }

    // ----- commit --------------------------------------------------------------

    pub fn commit(&self, message: &str, opts: &CommitOptions) -> Result<String> {
        let dir = tempfile::tempdir()?;
        let msg_path = dir.path().join("SLUICE_MSG");
        std::fs::write(&msg_path, message)?;
        let msg_arg = msg_path.to_string_lossy().into_owned();
        let mut args: Vec<&str> = vec!["commit", "-F", &msg_arg];
        if opts.amend {
            args.push("--amend");
        }
        if opts.signoff {
            args.push("--signoff");
        }
        if opts.no_verify {
            args.push("--no-verify");
        }
        let author_arg;
        if let Some(a) = &opts.author {
            author_arg = format!("--author={a}");
            args.push(&author_arg);
        }
        match opts.sign {
            Some(true) => args.push("-S"),
            Some(false) => args.push("--no-gpg-sign"),
            None => {}
        }
        let out = self.expect_ok(self.run(&args)?, "git commit")?;
        let head = self.run_read(&["rev-parse", "HEAD"])?;
        let _ = out;
        Ok(head.stdout_str().trim().to_string())
    }

    // ----- network ---------------------------------------------------------------

    pub fn fetch(&self, remote: Option<&str>, prune: bool) -> Result<CliOutput> {
        let mut args = vec!["fetch"];
        if prune {
            args.push("--prune");
        }
        if let Some(r) = remote {
            args.push(r);
        }
        self.expect_ok(self.run(&args)?, "git fetch")
    }

    pub fn pull(&self, rebase: Option<bool>) -> Result<CliOutput> {
        let mut args = vec!["pull"];
        match rebase {
            Some(true) => args.push("--rebase"),
            Some(false) => args.push("--no-rebase"),
            None => {}
        }
        self.expect_ok(self.run(&args)?, "git pull")
    }

    pub fn push(
        &self,
        remote: Option<&str>,
        branch: Option<&str>,
        set_upstream: bool,
        force_with_lease: bool,
    ) -> Result<CliOutput> {
        let mut args = vec!["push"];
        if set_upstream {
            args.push("-u");
        }
        if force_with_lease {
            args.push("--force-with-lease");
        }
        if let Some(r) = remote {
            args.push(r);
            if let Some(b) = branch {
                args.push(b);
            }
        }
        self.expect_ok(self.run(&args)?, "git push")
    }

    // ----- branches (M3) ---------------------------------------------------

    /// `git checkout <target>`; on dirty-tree failure the caller may retry with `smart` =
    /// stash → checkout → stash pop (IDEA "Smart Checkout", 05 §6).
    pub fn checkout(&self, target: &str) -> Result<CliOutput> {
        self.expect_ok(self.run(&["checkout", target])?, "git checkout")
    }

    pub fn smart_checkout(&self, target: &str) -> Result<CliOutput> {
        self.expect_ok(
            self.run(&["stash", "push", "-u", "-m", "sluice: smart checkout"])?,
            "git stash push",
        )?;
        let out = self.run(&["checkout", target])?;
        if !out.ok() {
            // restore and report
            let _ = self.run(&["stash", "pop"]);
            return self.expect_ok(out, "git checkout");
        }
        self.expect_ok(self.run(&["stash", "pop"])?, "git stash pop")?;
        Ok(out)
    }

    pub fn branch_create(&self, name: &str, from: Option<&str>, checkout: bool) -> Result<()> {
        if checkout {
            let mut args = vec!["checkout", "-b", name];
            if let Some(f) = from {
                args.push(f);
            }
            self.expect_ok(self.run(&args)?, "git checkout -b").map(|_| ())
        } else {
            let mut args = vec!["branch", name];
            if let Some(f) = from {
                args.push(f);
            }
            self.expect_ok(self.run(&args)?, "git branch").map(|_| ())
        }
    }

    pub fn branch_delete(&self, name: &str, force: bool) -> Result<()> {
        let flag = if force { "-D" } else { "-d" };
        self.expect_ok(self.run(&["branch", flag, name])?, "git branch delete")
            .map(|_| ())
    }

    pub fn merge(&self, branch: &str, no_ff: bool) -> Result<CliOutput> {
        let mut args = vec!["merge"];
        if no_ff {
            args.push("--no-ff");
        }
        args.push(branch);
        self.expect_ok(self.run(&args)?, "git merge")
    }

    pub fn rebase_onto(&self, upstream: &str) -> Result<CliOutput> {
        self.expect_ok(self.run(&["rebase", upstream])?, "git rebase")
    }

    /// continue / abort / skip for the operation in progress.
    pub fn op_step(&self, op: InProgressOp, step: &str) -> Result<CliOutput> {
        let sub = match op {
            InProgressOp::Merge => match step {
                "abort" => vec!["merge", "--abort"],
                _ => vec!["merge", "--continue"],
            },
            InProgressOp::Rebase => vec![
                "rebase",
                match step {
                    "abort" => "--abort",
                    "skip" => "--skip",
                    _ => "--continue",
                },
            ],
            InProgressOp::CherryPick => vec![
                "cherry-pick",
                match step {
                    "abort" => "--abort",
                    "skip" => "--skip",
                    _ => "--continue",
                },
            ],
            InProgressOp::Revert => vec![
                "revert",
                match step {
                    "abort" => "--abort",
                    "skip" => "--skip",
                    _ => "--continue",
                },
            ],
            InProgressOp::Bisect => vec!["bisect", "reset"],
        };
        self.expect_ok(self.run(&sub)?, "git operation step")
    }

    // ----- history ops -----------------------------------------------------

    pub fn cherry_pick(&self, sha: &str, record_origin: bool) -> Result<CliOutput> {
        let mut args = vec!["cherry-pick"];
        if record_origin {
            args.push("-x");
        }
        args.push(sha);
        self.expect_ok(self.run(&args)?, "git cherry-pick")
    }

    pub fn revert(&self, sha: &str) -> Result<CliOutput> {
        self.expect_ok(self.run(&["revert", "--no-edit", sha])?, "git revert")
    }

    /// mode: "soft" | "mixed" | "hard". Callers snapshot first for hard resets.
    pub fn reset(&self, mode: &str, target: &str) -> Result<CliOutput> {
        let flag = format!("--{mode}");
        self.expect_ok(self.run(&["reset", &flag, target])?, "git reset")
    }

    // ----- stash -----------------------------------------------------------

    pub fn stash_list(&self) -> Result<Vec<StashEntry>> {
        let out = self.run_read(&["stash", "list", "--format=%gd%x1f%H%x1f%ct%x1f%gs"])?;
        let out = self.expect_ok(out, "git stash list")?;
        let mut v = Vec::new();
        for line in out.stdout_str().lines() {
            let parts: Vec<&str> = line.split('\x1f').collect();
            if parts.len() >= 4 {
                v.push(StashEntry {
                    id: parts[0].to_string(),
                    sha: parts[1].to_string(),
                    time: parts[2].parse().unwrap_or(0),
                    message: parts[3].to_string(),
                });
            }
        }
        Ok(v)
    }

    pub fn stash_push(&self, message: &str, include_untracked: bool, keep_index: bool) -> Result<()> {
        let mut args = vec!["stash", "push"];
        if include_untracked {
            args.push("-u");
        }
        if keep_index {
            args.push("--keep-index");
        }
        if !message.is_empty() {
            args.push("-m");
            args.push(message);
        }
        self.expect_ok(self.run(&args)?, "git stash push").map(|_| ())
    }

    pub fn stash_apply(&self, id: &str, pop: bool) -> Result<()> {
        let sub = if pop { "pop" } else { "apply" };
        self.expect_ok(self.run(&["stash", sub, id])?, "git stash apply")
            .map(|_| ())
    }

    pub fn stash_drop(&self, id: &str) -> Result<()> {
        self.expect_ok(self.run(&["stash", "drop", id])?, "git stash drop")
            .map(|_| ())
    }

    // ----- safety-net snapshots (time machine v1, 05 §6) -------------------

    /// Capture worktree + index as a stash-like commit under refs/sluice/snapshots/.
    /// Returns None when there is nothing to snapshot (clean tree).
    pub fn snapshot_create(&self, label: &str) -> Result<Option<String>> {
        let out = self.expect_ok(self.run(&["stash", "create", label])?, "git stash create")?;
        let sha = out.stdout_str().trim().to_string();
        if sha.is_empty() {
            return Ok(None);
        }
        let slug: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .chars()
            .take(24)
            .collect();
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let refname = format!("refs/sluice/snapshots/{ts}-{slug}");
        self.expect_ok(self.run(&["update-ref", &refname, &sha])?, "git update-ref")?;
        Ok(Some(sha))
    }

    pub fn snapshot_list(&self) -> Result<Vec<SnapshotEntry>> {
        let out = self.run_read(&[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname)%1f%(objectname)%1f%(creatordate:unix)%1f%(subject)",
            "refs/sluice/snapshots/",
        ])?;
        let out = self.expect_ok(out, "git for-each-ref")?;
        let mut v = Vec::new();
        for line in out.stdout_str().lines() {
            let parts: Vec<&str> = line.split('\x1f').collect();
            if parts.len() >= 4 {
                v.push(SnapshotEntry {
                    refname: parts[0].to_string(),
                    sha: parts[1].to_string(),
                    time: parts[2].parse().unwrap_or(0),
                    message: parts[3].to_string(),
                });
            }
        }
        Ok(v)
    }

    /// Apply a snapshot's changes back onto the worktree (stash-apply semantics).
    pub fn snapshot_apply(&self, sha: &str) -> Result<()> {
        self.expect_ok(self.run(&["stash", "apply", sha])?, "git stash apply (snapshot)")
            .map(|_| ())
    }

    pub fn snapshot_delete(&self, refname: &str) -> Result<()> {
        self.expect_ok(self.run(&["update-ref", "-d", refname])?, "git update-ref -d")
            .map(|_| ())
    }

    // ----- file history / blame (M3) ---------------------------------------

    /// `git log --follow` for one path (rename-aware). Read-only, echoed to the Console.
    pub fn file_history(&self, path: &str, limit: usize) -> Result<Vec<FileHistoryEntry>> {
        let n = limit.to_string();
        let out = self.run_read(&[
            "log",
            "--follow",
            "--format=%H%x1f%an%x1f%ct%x1f%s",
            "-n",
            &n,
            "--",
            path,
        ])?;
        let out = self.expect_ok(out, "git log --follow")?;
        let mut v = Vec::new();
        for line in out.stdout_str().lines() {
            let parts: Vec<&str> = line.split('\x1f').collect();
            if parts.len() >= 4 {
                v.push(FileHistoryEntry {
                    sha: parts[0].to_string(),
                    author: parts[1].to_string(),
                    time: parts[2].parse().unwrap_or(0),
                    subject: parts[3].to_string(),
                });
            }
        }
        Ok(v)
    }

    /// `git blame --porcelain [rev] -- path`. `rev = None` blames the working tree.
    pub fn blame(&self, path: &str, rev: Option<&str>) -> Result<Vec<BlameLine>> {
        let mut args = vec!["blame", "--porcelain", "-w"];
        if let Some(r) = rev {
            args.push(r);
        }
        args.push("--");
        args.push(path);
        let out = self.run_read(&args)?;
        let out = self.expect_ok(out, "git blame")?;
        Ok(parse_blame_porcelain(&out.stdout_str()))
    }

    /// Undo the last (unpushed) commit keeping its changes staged (05 §5).
    pub fn undo_last_commit(&self) -> Result<()> {
        self.expect_ok(self.run(&["reset", "--soft", "HEAD~1"])?, "git reset --soft")
            .map(|_| ())
    }
}

fn first_line(s: &str) -> Option<String> {
    s.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

fn shell_join(args: &[&str]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') || a.contains('"') {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Detect an in-progress operation from the git dir's marker files (05 §6).
pub fn detect_in_progress(git_dir: &Path) -> Option<InProgressOp> {
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        Some(InProgressOp::Rebase)
    } else if git_dir.join("MERGE_HEAD").exists() {
        Some(InProgressOp::Merge)
    } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
        Some(InProgressOp::CherryPick)
    } else if git_dir.join("REVERT_HEAD").exists() {
        Some(InProgressOp::Revert)
    } else if git_dir.join("BISECT_LOG").exists() {
        Some(InProgressOp::Bisect)
    } else {
        None
    }
}

#[cfg(test)]
mod blame_tests {
    use super::parse_blame_porcelain;

    #[test]
    fn blame_porcelain_parses() {
        let out = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 2\nauthor Ada\nauthor-time 1700000000\nsummary first\nfilename f.rs\n\tline one\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 2 2\n\tline two\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 1 3 1\nauthor Bob\nauthor-time 1700000100\nsummary second\nfilename f.rs\n\tline three\n";
        let v = parse_blame_porcelain(out);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].author, "Ada");
        assert_eq!(v[1].author, "Ada");
        assert_eq!(v[1].line_no, 2);
        assert_eq!(v[2].summary, "second");
        assert_eq!(v[2].text, "line three");
    }
}
