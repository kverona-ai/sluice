//! Reverse AI calls: reuse whatever AI CLI the user already has and is logged
//! into, in headless mode, to draft a commit message — no API key, no
//! configuration. The staged diff is sent to the tool on the user's own
//! account (a first-use notice is shown in the UI).
//!
//! Tool table verified against each CLI's own docs (2026-08). Three input
//! modes cover the ecosystem:
//! - `Stdin`: the whole input (instruction + diff) is piped to stdin
//! - `ArgWithStdin`: the instruction rides the prompt flag, the diff is piped
//!   (the documented pattern for the Gemini-CLI family and Grok Build)
//! - `PromptArg`: everything goes into one argument — for tools whose stdin is
//!   ignored or unspecified when a prompt flag is present (Copilot CLI is
//!   documented as mutually exclusive). Capped well under the Windows 32 KB
//!   command-line limit.
//!
//! Aider is deliberately NOT in this table: its default `--auto-commits` can
//! perform a real `git commit` as a side effect, which violates "AI proposes,
//! humans decide".

use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use sluice_backend_cli::env::{find_executable, login_path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Stdin,
    ArgWithStdin,
    PromptArg,
}

pub struct ToolSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub bin: &'static str,
    pub args: &'static [&'static str],
    pub mode: InputMode,
}

/// Detection priority: the four deeply-integrated tools first, then the rest.
pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        id: "claude-code",
        name: "Claude Code",
        bin: "claude",
        args: &["-p", "--output-format", "text"],
        mode: InputMode::Stdin,
    },
    ToolSpec {
        id: "codex",
        name: "Codex CLI",
        bin: "codex",
        args: &["exec", "--skip-git-repo-check", "-"],
        mode: InputMode::Stdin,
    },
    // `git diff | grok -p "..."` is the documented headless pattern.
    ToolSpec {
        id: "grok-build",
        name: "Grok Build",
        bin: "grok",
        args: &["--no-auto-update", "-p"],
        mode: InputMode::ArgWithStdin,
    },
    // Developer preview; headless profile syntax may still change upstream.
    ToolSpec {
        id: "dsh",
        name: "DeepSeek Harness",
        bin: "dsh",
        args: &["--profile", "headless"],
        mode: InputMode::Stdin,
    },
    ToolSpec {
        id: "gemini",
        name: "Gemini CLI",
        bin: "gemini",
        args: &["-p"],
        mode: InputMode::ArgWithStdin,
    },
    // Both the current Kimi Code and the legacy kimi-cli accept `--print` with the prompt on stdin.
    ToolSpec {
        id: "kimi",
        name: "Kimi Code",
        bin: "kimi",
        args: &["--print"],
        mode: InputMode::Stdin,
    },
    // Official fork of Gemini CLI — same flag family.
    ToolSpec {
        id: "qwen",
        name: "Qwen Code",
        bin: "qwen",
        args: &["-p"],
        mode: InputMode::ArgWithStdin,
    },
    // Agentic runner; prompt must be the `run` argument.
    ToolSpec {
        id: "opencode",
        name: "opencode",
        bin: "opencode",
        args: &["run"],
        mode: InputMode::PromptArg,
    },
    // Copilot CLI documents stdin and --prompt as mutually exclusive.
    ToolSpec {
        id: "copilot",
        name: "Copilot CLI",
        bin: "copilot",
        args: &["-s", "--no-ask-user", "-p"],
        mode: InputMode::PromptArg,
    },
    // Z.ai ships no official standalone headless CLI; this matches the community
    // wrapper's flags. GLM users more commonly route through claude/codex with a
    // custom base URL, which the entries above already cover.
    ToolSpec {
        id: "zcode",
        name: "Z Code",
        bin: "zcode",
        args: &["--print", "--prompt"],
        mode: InputMode::PromptArg,
    },
];

/// First installed tool in priority order: (id, display name).
pub fn detect_tool() -> Option<(String, String)> {
    TOOLS
        .iter()
        .find(|t| find_executable(t.bin).is_some())
        .map(|t| (t.id.to_string(), t.name.to_string()))
}

/// All installed tools (for a future provider picker in Settings).
pub fn detect_tools() -> Vec<(String, String)> {
    TOOLS
        .iter()
        .filter(|t| find_executable(t.bin).is_some())
        .map(|t| (t.id.to_string(), t.name.to_string()))
        .collect()
}

const PROMPT_ZH: &str = "你是资深工程师。根据下面的 staged diff 与仓库最近提交信息的风格，写一条 git 提交信息：\
第一行 ≤ 72 字符的 subject（遵循 Conventional Commits，如 feat/fix/docs/refactor），空一行后写简短 body（要点列表，说明为什么）。\
只输出提交信息本身，不要解释，不要代码块，不要执行任何命令。";
const PROMPT_EN: &str = "You are a senior engineer. Write a git commit message for the staged diff below, matching the style of the recent subjects: \
a subject line ≤ 72 chars (Conventional Commits: feat/fix/docs/refactor…), a blank line, then a short bullet body saying why. \
Output only the commit message. No explanation, no code fences, and do not run any commands.";

/// Diff budget for stdin-carried payloads (05 §5).
const MAX_DIFF_BYTES: usize = 64 * 1024;
/// Budget when everything must fit into a single argv entry (Windows caps the
/// whole command line at ~32 KB).
const MAX_ARG_BYTES: usize = 24 * 1024;

fn truncate_at_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn build_instruction(recent_subjects: &[String]) -> String {
    let zh = recent_subjects
        .iter()
        .filter(|s| s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
        .count()
        * 10
        >= recent_subjects.len().max(1) * 7;
    let mut out = String::from(if zh { PROMPT_ZH } else { PROMPT_EN });
    out.push_str("\n\nRecent subjects:\n");
    for s in recent_subjects.iter().take(30) {
        out.push_str("- ");
        out.push_str(s);
        out.push('\n');
    }
    out
}

/// Blocking: run the tool headless. Called on the background executor.
pub fn draft_commit_message(tool_id: &str, staged_diff: &str, recent_subjects: &[String]) -> Result<String> {
    let instruction = build_instruction(recent_subjects);
    run_tool(tool_id, &instruction, staged_diff)
}

/// Run a provider headless with `instruction` + a diff-like `payload` using the tool's
/// input mode; returns the trimmed text output.
fn run_tool(tool_id: &str, instruction: &str, payload: &str) -> Result<String> {
    let spec = TOOLS
        .iter()
        .find(|t| t.id == tool_id)
        .ok_or_else(|| anyhow::anyhow!("unknown AI tool {tool_id}"))?;
    let diff_budget = if spec.mode == InputMode::PromptArg {
        MAX_ARG_BYTES.saturating_sub(instruction.len() + 64)
    } else {
        MAX_DIFF_BYTES
    };
    let diff = truncate_at_boundary(payload, diff_budget);
    let truncated = diff.len() < payload.len();
    let diff_block = format!(
        "\nDiff:\n{diff}{}",
        if truncated {
            "\n… (diff truncated)\n"
        } else {
            "\n"
        }
    );

    let exe = find_executable(spec.bin).with_context(|| format!("{} not found on PATH", spec.bin))?;
    let mut cmd = Command::new(exe);
    cmd.env("PATH", login_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let stdin_payload: Option<String> = match spec.mode {
        InputMode::Stdin => {
            cmd.args(spec.args).stdin(Stdio::piped());
            Some(format!("{instruction}{diff_block}"))
        }
        InputMode::ArgWithStdin => {
            cmd.args(spec.args).arg(instruction).stdin(Stdio::piped());
            Some(diff_block)
        }
        InputMode::PromptArg => {
            cmd.args(spec.args)
                .arg(format!("{instruction}{diff_block}"))
                .stdin(Stdio::null());
            None
        }
    };
    let mut child = cmd.spawn().with_context(|| format!("spawning {}", spec.bin))?;
    if let Some(payload) = stdin_payload {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin.write_all(payload.as_bytes())?;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            bail!("{} timed out after 120s", spec.bin);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "{} exited with {}: {}",
            spec.bin,
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
        bail!("{} returned an empty message", spec.bin);
    }
    Ok(text)
}

const REVIEW_PROMPT_ZH: &str = "你是资深代码评审者。下面是一个 Pull Request 的标题与完整 diff。请写一条简明的评审意见（Markdown）：先一句总体判断，再按严重程度列出最多 8 条具体问题（文件:行、风险、建议），最后给出是否可合并的建议。只输出评审文本，不要执行任何命令。";
const REVIEW_PROMPT_EN: &str = "You are a senior code reviewer. Below are a pull request title and its full diff. Write a concise review (Markdown): one-line overall verdict, then up to 8 concrete findings ordered by severity (file:line, risk, suggestion), then a merge recommendation. Output only the review text; do not run any commands.";

/// Draft a PR review comment from a patch (same provider table / input modes as commit drafts).
pub fn draft_review(tool_id: &str, title: &str, patch: &str) -> Result<String> {
    let zh = title.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        || crate::i18n::lang() == crate::i18n::Lang::Zh;
    let instruction = format!(
        "{}\n\nTitle: {}\n",
        if zh { REVIEW_PROMPT_ZH } else { REVIEW_PROMPT_EN },
        title
    );
    run_tool(tool_id, &instruction, patch)
}
