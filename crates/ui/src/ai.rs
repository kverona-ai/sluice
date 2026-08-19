//! Reverse AI calls (03 §5): reuse the user's logged-in AI CLI in headless
//! mode to draft a commit message — no API key, no configuration. The staged
//! diff is sent to the tool; the UI asks for consent the first time (05 §5).

use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use sluice_backend_cli::env::{find_executable, login_path};

/// Detect an installed AI CLI: (id, display name). Fixed preference order (03 §5).
pub fn detect_tool() -> Option<(String, String)> {
    for (bin, id, name) in [
        ("claude", "claude-code", "Claude Code"),
        ("codex", "codex", "Codex CLI"),
        ("grok", "grok-build", "Grok Build"),
        ("dsh", "dsh", "DeepSeek Harness"),
    ] {
        if find_executable(bin).is_some() {
            return Some((id.to_string(), name.to_string()));
        }
    }
    None
}

const PROMPT_ZH: &str = "你是资深工程师。根据下面的 staged diff 与仓库最近提交信息的风格，写一条 git 提交信息：\
第一行 ≤ 72 字符的 subject（遵循 Conventional Commits，如 feat/fix/docs/refactor），空一行后写简短 body（要点列表，说明为什么）。\
只输出提交信息本身，不要解释，不要代码块。";
const PROMPT_EN: &str = "You are a senior engineer. Write a git commit message for the staged diff below, matching the style of the recent subjects: \
a subject line ≤ 72 chars (Conventional Commits: feat/fix/docs/refactor…), a blank line, then a short bullet body saying why. \
Output only the commit message, no explanation, no code fences.";

/// Blocking: run the tool headless with the diff on stdin. Called on the background executor.
pub fn draft_commit_message(tool_id: &str, staged_diff: &str, recent_subjects: &[String]) -> Result<String> {
    let zh = recent_subjects
        .iter()
        .filter(|s| s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
        .count()
        * 10
        >= recent_subjects.len().max(1) * 7;
    let prompt = if zh { PROMPT_ZH } else { PROMPT_EN };
    let mut input = String::new();
    input.push_str(prompt);
    input.push_str("\n\nRecent subjects:\n");
    for s in recent_subjects.iter().take(30) {
        input.push_str("- ");
        input.push_str(s);
        input.push('\n');
    }
    input.push_str("\nStaged diff:\n");
    // 64KB cap (05 §5)
    let diff = if staged_diff.len() > 64 * 1024 {
        format!("{}\n… (diff truncated at 64KB)\n", &staged_diff[..64 * 1024])
    } else {
        staged_diff.to_string()
    };
    input.push_str(&diff);

    let (bin, args): (&str, Vec<&str>) = match tool_id {
        "claude-code" => ("claude", vec!["-p", "--output-format", "text"]),
        "codex" => ("codex", vec!["exec", "--skip-git-repo-check", "-"]),
        "grok-build" => ("grok", vec!["-p"]),
        "dsh" => ("dsh", vec!["--profile", "headless"]),
        other => bail!("unknown AI tool {other}"),
    };
    let exe = find_executable(bin).with_context(|| format!("{bin} not found on PATH"))?;
    let mut child = Command::new(exe)
        .args(&args)
        .env("PATH", login_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {bin}"))?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("piped stdin");
        // For claude -p the prompt is the stdin when no positional prompt is given.
        stdin.write_all(input.as_bytes())?;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            bail!("{bin} timed out after 90s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "{bin} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let text = text
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string();
    if text.is_empty() {
        bail!("{bin} returned an empty message");
    }
    Ok(text)
}
