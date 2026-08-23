//! Session provenance (03 §4): `sluice hook <tool>` receives hook events from
//! AI CLIs on stdin, normalizes them, and appends them to
//! `~/.sluice/provenance/<repo-key>.jsonl`. The app later matches events to
//! commits by touched paths and time window.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProvenanceEvent {
    /// Sluice tool id, e.g. "claude-code".
    pub tool: String,
    pub session_id: String,
    /// Normalized: "tool_use" | "session_end" | "session_start" | "stop" | "other".
    pub event: String,
    pub cwd: String,
    /// Paths touched (relative to the repo root when resolvable).
    pub files: Vec<String>,
    /// Unix seconds.
    pub at: i64,
    /// Tool-specific detail: native tool name, transcript path, etc.
    #[serde(default)]
    pub detail: String,
}

fn home() -> PathBuf {
    std::env::var_os("SLUICE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn store_dir() -> PathBuf {
    home().join(".sluice").join("provenance")
}

pub fn store_file(repo_root: &Path) -> PathBuf {
    store_dir().join(format!("{}.jsonl", crate::ipc::repo_key(repo_root)))
}

/// Walk up from `cwd` to the enclosing repository root (dir containing `.git`).
pub fn repo_root_of(cwd: &Path) -> Option<PathBuf> {
    let mut p = cwd.canonicalize().ok()?;
    loop {
        if p.join(".git").exists() {
            return Some(p);
        }
        if !p.pop() {
            return None;
        }
    }
}

fn get<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| v.get(k))
}

fn str_of(v: &Value, keys: &[&str]) -> Option<String> {
    get(v, keys).and_then(Value::as_str).map(str::to_string)
}

/// Extract file paths from a Codex `apply_patch` body (`*** Update File: path`).
fn paths_from_patch(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|l| {
            l.strip_prefix("*** Update File: ")
                .or_else(|| l.strip_prefix("*** Add File: "))
                .or_else(|| l.strip_prefix("*** Delete File: "))
                .map(|p| p.trim().to_string())
        })
        .collect()
}

/// Normalize one raw hook payload. Handles the snake_case family (Claude Code,
/// Codex hooks, Qwen Code, Kimi Code, Copilot CLI in PascalCase mode), Gemini
/// CLI (Before/After naming) and Grok Build's camelCase.
pub fn normalize(tool: &str, raw: &Value, now: i64) -> ProvenanceEvent {
    let event_name = str_of(raw, &["hook_event_name", "hookEventName", "type"]).unwrap_or_default();
    let session_id = str_of(raw, &["session_id", "sessionId", "thread-id"]).unwrap_or_default();
    let cwd = str_of(raw, &["cwd", "workspaceRoot", "workspace_root"]).unwrap_or_default();
    let tool_name = str_of(raw, &["tool_name", "toolName"]).unwrap_or_default();
    let tool_input = get(raw, &["tool_input", "toolInput", "toolArgs", "tool_args"])
        .cloned()
        .unwrap_or(Value::Null);
    let mut files: Vec<String> = Vec::new();
    for key in ["file_path", "filePath", "path", "notebook_path"] {
        if let Some(p) = tool_input.get(key).and_then(Value::as_str) {
            files.push(p.to_string());
        }
    }
    if let Some(edits) = tool_input.get("edits").and_then(Value::as_array) {
        for e in edits {
            if let Some(p) = e.get("file_path").and_then(Value::as_str) {
                files.push(p.to_string());
            }
        }
    }
    if tool_name == "apply_patch"
        && let Some(cmd) = tool_input
            .get("command")
            .and_then(Value::as_str)
            .or_else(|| tool_input.get("input").and_then(Value::as_str))
    {
        files.extend(paths_from_patch(cmd));
    }
    // Relativize to the repo root when possible.
    if let Some(root) = repo_root_of(Path::new(&cwd)) {
        files = files
            .into_iter()
            .map(|f| {
                let p = Path::new(&f);
                if p.is_absolute() {
                    p.strip_prefix(&root)
                        .map(|r| r.to_string_lossy().to_string())
                        .unwrap_or(f)
                } else {
                    f
                }
            })
            .collect();
    }
    files.sort();
    files.dedup();
    let event = match event_name.as_str() {
        "PostToolUse" | "AfterTool" | "postToolUse" | "tool.execute.after" => "tool_use",
        "SessionEnd" | "sessionEnd" => "session_end",
        "SessionStart" | "sessionStart" => "session_start",
        "Stop" | "agentStop" | "agent-turn-complete" => "stop",
        _ => "other",
    };
    let transcript = str_of(raw, &["transcript_path", "transcriptPath"]).unwrap_or_default();
    let detail = if transcript.is_empty() {
        tool_name.clone()
    } else {
        format!("{tool_name} · transcript {transcript}")
    };
    ProvenanceEvent {
        tool: tool.to_string(),
        session_id,
        event: event.to_string(),
        cwd,
        files,
        at: now,
        detail,
    }
}

/// Append an event to the store for the repository enclosing `cwd`.
pub fn record(ev: &ProvenanceEvent) -> Result<PathBuf> {
    let root = repo_root_of(Path::new(&ev.cwd)).unwrap_or_else(|| PathBuf::from(&ev.cwd));
    let file = store_file(&root);
    std::fs::create_dir_all(store_dir())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .with_context(|| format!("opening {}", file.display()))?;
    use std::io::Write;
    writeln!(f, "{}", serde_json::to_string(ev)?)?;
    Ok(file)
}

/// Load events for a repository (tail-capped to the last ~2 MB).
pub fn load(repo_root: &Path) -> Vec<ProvenanceEvent> {
    let Ok(text) = std::fs::read_to_string(store_file(repo_root)) else {
        return Vec::new();
    };
    let tail = if text.len() > 2 * 1024 * 1024 {
        &text[text.len() - 2 * 1024 * 1024..]
    } else {
        &text[..]
    };
    tail.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Events plausibly behind a commit: same files, within `window_secs` before the
/// commit time (and a short grace after). Grouped per (tool, session).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMatch {
    pub tool: String,
    pub session_id: String,
    pub events: usize,
    pub files: Vec<String>,
    pub first_at: i64,
    pub last_at: i64,
    pub detail: String,
}

pub fn matches_for_commit(
    events: &[ProvenanceEvent],
    files: &[String],
    commit_at: i64,
    window_secs: i64,
) -> Vec<SessionMatch> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<(String, String), SessionMatch> = BTreeMap::new();
    for ev in events {
        if ev.at > commit_at + 300 || ev.at < commit_at - window_secs {
            continue;
        }
        let hit: Vec<String> = ev
            .files
            .iter()
            .filter(|f| files.iter().any(|c| c == *f))
            .cloned()
            .collect();
        if hit.is_empty() {
            continue;
        }
        let e = map
            .entry((ev.tool.clone(), ev.session_id.clone()))
            .or_insert_with(|| SessionMatch {
                tool: ev.tool.clone(),
                session_id: ev.session_id.clone(),
                events: 0,
                files: Vec::new(),
                first_at: ev.at,
                last_at: ev.at,
                detail: ev.detail.clone(),
            });
        e.events += 1;
        e.first_at = e.first_at.min(ev.at);
        e.last_at = e.last_at.max(ev.at);
        for f in hit {
            if !e.files.contains(&f) {
                e.files.push(f);
            }
        }
    }
    map.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_claude_and_grok_and_codex() {
        let c = normalize(
            "claude-code",
            &json!({"hook_event_name":"PostToolUse","session_id":"s1","cwd":"/nonexistent","tool_name":"Edit","tool_input":{"file_path":"src/a.rs"}}),
            100,
        );
        assert_eq!(c.event, "tool_use");
        assert_eq!(c.files, vec!["src/a.rs"]);
        let g = normalize(
            "grok-build",
            &json!({"hookEventName":"PostToolUse","sessionId":"g1","cwd":"/x","toolName":"Write","toolInput":{"filePath":"b.rs"}}),
            100,
        );
        assert_eq!(g.session_id, "g1");
        assert_eq!(g.files, vec!["b.rs"]);
        let x = normalize(
            "codex",
            &json!({"hook_event_name":"PostToolUse","session_id":"c1","cwd":"/x","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/c.rs\n@@\n*** End Patch"}}),
            100,
        );
        assert_eq!(x.files, vec!["src/c.rs"]);
    }

    #[test]
    fn matching_groups_by_session() {
        let mk = |tool: &str, sid: &str, f: &str, at: i64| ProvenanceEvent {
            tool: tool.into(),
            session_id: sid.into(),
            event: "tool_use".into(),
            cwd: "/".into(),
            files: vec![f.into()],
            at,
            detail: String::new(),
        };
        let evs = vec![
            mk("claude-code", "s", "a.rs", 900),
            mk("claude-code", "s", "b.rs", 950),
            mk("codex", "z", "zzz.rs", 960),
        ];
        let m = matches_for_commit(&evs, &["a.rs".into(), "b.rs".into()], 1000, 3600);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].events, 2);
        assert_eq!(m[0].files.len(), 2);
    }
}
