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
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
