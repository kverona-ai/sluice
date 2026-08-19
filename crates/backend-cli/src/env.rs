//! GUI processes on macOS / Linux don't inherit the user's shell PATH (05 §3):
//! resolve it once through the login shell (3s timeout) and fall back to the
//! usual install locations. Used for `git` and for AI CLI detection.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static LOGIN_PATH: OnceLock<String> = OnceLock::new();

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn common_dirs() -> Vec<String> {
    let mut v = vec![
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    if let Some(h) = home() {
        for rel in [
            ".cargo/bin",
            ".local/bin",
            ".volta/bin",
            ".bun/bin",
            "go/bin",
            ".npm-global/bin",
        ] {
            v.push(h.join(rel).to_string_lossy().into_owned());
        }
        // nvm / fnm: current default node version, if any
        if let Ok(rd) = std::fs::read_dir(h.join(".nvm/versions/node")) {
            let mut versions: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            versions.sort();
            if let Some(latest) = versions.last() {
                v.push(latest.join("bin").to_string_lossy().into_owned());
            }
        }
    }
    v
}

fn shell_path() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut child = Command::new(&shell)
        .args(["-ilc", "printf %s \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// The PATH to use for child processes: login-shell PATH ∪ process PATH ∪ common dirs.
pub fn login_path() -> &'static str {
    LOGIN_PATH.get_or_init(|| {
        let sep = if cfg!(windows) { ';' } else { ':' };
        let mut parts: Vec<String> = Vec::new();
        let mut push = |p: &str| {
            if !p.is_empty() && !parts.iter().any(|x| x == p) {
                parts.push(p.to_string());
            }
        };
        if let Some(p) = shell_path() {
            p.split(sep).for_each(&mut push);
        }
        if let Ok(p) = std::env::var("PATH") {
            p.split(sep).for_each(&mut push);
        }
        common_dirs().iter().for_each(|d| push(d));
        parts.join(&sep.to_string())
    })
}

/// Locate an executable on the resolved PATH.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    which::which_in(name, Some(login_path()), std::env::current_dir().ok()?).ok()
}

pub fn find_git() -> Option<PathBuf> {
    find_executable("git")
}
