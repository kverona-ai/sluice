//! jujutsu adapter (05 §8): the jj CLI with templates / `--summary` output. jj has no
//! staging area — the working copy *is* a commit — so "changes" are the working-copy
//! diff against its parent, "commit" is `jj commit` / `jj describe`, and the operation
//! log feeds the time machine. Read results are echoed to the Console like git.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use chrono::Local;
use sluice_core::*;

use crate::env;

#[derive(Clone)]
pub struct JjCli {
    workdir: PathBuf,
    jj: PathBuf,
    console: Console,
}

#[derive(Clone, Debug, Default)]
pub struct JjWorkingCopy {
    pub change_id: String,
    pub commit_id: String,
    pub description: String,
    pub parent_change_id: String,
    pub conflicts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct JjOp {
    pub id: String,
    pub time: String,
    pub description: String,
    pub current: bool,
}

pub fn find_jj() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SLUICE_JJ_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    env::find_executable("jj")
}

impl JjCli {
    pub fn new(workdir: PathBuf, console: Console) -> Result<Self> {
        let jj = find_jj().context("jj not found on PATH")?;
        Ok(Self { workdir, jj, console })
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    fn run_inner(&self, args: &[&str], kind: ConsoleKind) -> Result<String> {
        let started = Instant::now();
        let out = Command::new(&self.jj)
            .args(args)
            .current_dir(&self.workdir)
            .env("PATH", env::login_path())
            .env("NO_COLOR", "1")
            .env("JJ_CONFIG", std::env::var_os("JJ_CONFIG").unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("spawning jj {}", args.join(" ")))?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        self.console.log(ConsoleEntry {
            at: Local::now(),
            kind,
            command: format!("jj {}", args.join(" ")),
            duration_ms: started.elapsed().as_millis(),
            exit_code: out.status.code(),
            summary: if out.status.success() {
                stdout.lines().next().unwrap_or("").to_string()
            } else {
                stderr.lines().next().unwrap_or("").to_string()
            },
            stderr: stderr.clone(),
        });
        if !out.status.success() {
            bail!("jj {}: {}", args.join(" "), stderr);
        }
        Ok(stdout)
    }

    pub fn read(&self, args: &[&str]) -> Result<String> {
        self.run_inner(args, ConsoleKind::Read)
    }

    pub fn write(&self, args: &[&str]) -> Result<String> {
        self.run_inner(args, ConsoleKind::Write)
    }

    /// Working-copy commit (@) metadata + conflicted paths.
    pub fn working_copy(&self) -> Result<JjWorkingCopy> {
        let out = self.read(&[
            "log",
            "-r",
            "@",
            "--no-graph",
            "-T",
            "change_id.short() ++ \"\\x1f\" ++ commit_id.short() ++ \"\\x1f\" ++ parents.map(|c| c.change_id().short()).join(\",\") ++ \"\\x1f\" ++ description",
        ])?;
        let mut wc = parse_working_copy(&out);
        wc.conflicts = self
            .read(&["resolve", "--list"])
            .map(|s| parse_conflicts(&s))
            .unwrap_or_default();
        Ok(wc)
    }

    /// `jj diff --summary` of the working copy against its parent.
    pub fn summary(&self) -> Result<Vec<StatusEntry>> {
        let out = self.read(&["diff", "--summary"])?;
        Ok(parse_summary(&out))
    }

    /// File contents at a revision (`@-` = parent of the working copy).
    pub fn file_show(&self, rev: &str, path: &str) -> Result<Vec<u8>> {
        let out = Command::new(&self.jj)
            .args(["file", "show", "-r", rev, path])
            .current_dir(&self.workdir)
            .env("PATH", env::login_path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        if !out.status.success() {
            bail!(
                "jj file show {rev} {path}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out.stdout)
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        self.write(&["commit", "-m", message])
    }

    pub fn describe(&self, message: &str) -> Result<String> {
        self.write(&["describe", "-m", message])
    }

    pub fn new_change(&self) -> Result<String> {
        self.write(&["new"])
    }

    pub fn squash_into_parent(&self) -> Result<String> {
        self.write(&["squash"])
    }

    pub fn op_log(&self, limit: usize) -> Result<Vec<JjOp>> {
        let n = limit.to_string();
        let out = self.read(&[
            "op",
            "log",
            "--no-graph",
            "-n",
            &n,
            "-T",
            "id.short() ++ \"\\x1f\" ++ time.end().format(\"%Y-%m-%d %H:%M:%S\") ++ \"\\x1f\" ++ description ++ \"\\n\"",
        ])?;
        Ok(parse_op_log(&out))
    }

    pub fn undo(&self) -> Result<String> {
        self.write(&["undo"])
    }

    pub fn op_restore(&self, op: &str) -> Result<String> {
        self.write(&["op", "restore", op])
    }

    pub fn git_push(&self) -> Result<String> {
        self.write(&["git", "push", "--tracked"])
    }

    pub fn git_fetch(&self) -> Result<String> {
        self.write(&["git", "fetch"])
    }

    /// change id for a git commit (short), if jj knows it.
    pub fn change_id_of(&self, commit: &str) -> Result<String> {
        let out = self.read(&["log", "-r", commit, "--no-graph", "-T", "change_id.short()"])?;
        Ok(out.trim().to_string())
    }
}

pub fn parse_working_copy(s: &str) -> JjWorkingCopy {
    let line = s.lines().next().unwrap_or("");
    let mut it = line.split('\u{1f}');
    JjWorkingCopy {
        change_id: it.next().unwrap_or("").trim().to_string(),
        commit_id: it.next().unwrap_or("").trim().to_string(),
        parent_change_id: it.next().unwrap_or("").trim().to_string(),
        description: it.next().unwrap_or("").trim().to_string(),
        conflicts: Vec::new(),
    }
}

pub fn parse_conflicts(s: &str) -> Vec<String> {
    s.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() {
                return None;
            }
            Some(l.split_whitespace().next().unwrap_or(l).to_string())
        })
        .collect()
}

/// `M path` / `A path` / `D path` / `R old => new` (jj prints `R {old => new}` or `R old => new`).
pub fn parse_summary(s: &str) -> Vec<StatusEntry> {
    let mut v = Vec::new();
    for line in s.lines() {
        let line = line.trim_end();
        if line.len() < 3 {
            continue;
        }
        let code = line.chars().next().unwrap_or('M');
        let rest = line[1..].trim();
        let (path, old_path) = if code == 'R' || code == 'C' {
            let cleaned = rest.trim_start_matches('{').trim_end_matches('}');
            match cleaned.split_once(" => ") {
                Some((a, b)) => (b.trim().to_string(), Some(a.trim().to_string())),
                None => (cleaned.to_string(), None),
            }
        } else {
            (rest.to_string(), None)
        };
        let kind = match code {
            'A' => ChangeKind::Added,
            'D' => ChangeKind::Deleted,
            'R' => ChangeKind::Renamed,
            'C' => ChangeKind::Copied,
            _ => ChangeKind::Modified,
        };
        v.push(StatusEntry {
            path,
            old_path,
            staged: None,
            unstaged: Some(kind),
            untracked: false,
            conflict: None,
            submodule: false,
        });
    }
    v
}

pub fn parse_op_log(s: &str) -> Vec<JjOp> {
    let mut v = Vec::new();
    for (i, line) in s.lines().enumerate() {
        let mut it = line.split('\u{1f}');
        let id = it.next().unwrap_or("").trim().to_string();
        if id.is_empty() {
            continue;
        }
        v.push(JjOp {
            id,
            time: it.next().unwrap_or("").trim().to_string(),
            description: it.next().unwrap_or("").trim().to_string(),
            current: i == 0,
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_summary_and_ops() {
        let s = parse_summary("M src/lib.rs\nA docs/new.md\nD old.txt\nR {a.rs => b.rs}\n");
        assert_eq!(s.len(), 4);
        assert_eq!(s[3].path, "b.rs");
        assert_eq!(s[3].old_path.as_deref(), Some("a.rs"));
        assert_eq!(s[2].unstaged, Some(ChangeKind::Deleted));
        let ops = parse_op_log(
            "abc123\u{1f}2026-08-23 10:00:00\u{1f}commit 1234\nxyz\u{1f}2026-08-22 09:00:00\u{1f}snapshot working copy\n",
        );
        assert_eq!(ops.len(), 2);
        assert!(ops[0].current);
        assert_eq!(ops[1].description, "snapshot working copy");
        let wc = parse_working_copy("kqmz\u{1f}0f12\u{1f}rlvk\u{1f}wip: thing\n");
        assert_eq!(wc.change_id, "kqmz");
        assert_eq!(wc.description, "wip: thing");
    }
}
