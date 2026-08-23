//! One-click hook installation (03 §2/§4): make each AI CLI call
//! `sluice hook <tool>` after file edits and at session end so commits can be
//! traced back to sessions. Strategy per tool follows each tool's own merge
//! semantics: append to the single settings file (Claude Code, Gemini CLI,
//! Qwen Code, Kimi Code), or drop a dedicated file where the tool loads a
//! directory / separate file (Codex `hooks.json`, Grok Build and Copilot CLI
//! `hooks/` directories). Idempotent: re-running never duplicates entries.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct HookReport {
    pub tool: &'static str,
    pub path: PathBuf,
    /// "appended" | "created" | "already" | "unsupported"
    pub outcome: &'static str,
    pub note: String,
}

fn home() -> PathBuf {
    std::env::var_os("SLUICE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn shell_quote(s: &str) -> String {
    if s.contains(' ') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// The command each tool should run.
pub fn hook_command(tool_id: &str) -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "sluice".into());
    format!("{} hook {}", shell_quote(&exe), tool_id)
}

fn read_json(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => {
            Ok(serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?)
        }
        _ => Ok(json!({})),
    }
}

fn write_json(path: &Path, v: &Value, backup: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if backup && path.exists() {
        let b = path.with_extension("json.sluice.bak");
        let _ = std::fs::copy(path, b);
    }
    std::fs::write(path, serde_json::to_string_pretty(v)? + "\n")?;
    Ok(())
}

/// Does any hook under `hooks.<event>` already run our command?
fn has_ours(hooks: &Value, cmd_marker: &str) -> bool {
    serde_json::to_string(hooks)
        .map(|s| s.contains(cmd_marker))
        .unwrap_or(false)
}

/// Append `{matcher, hooks:[{type:command,command,timeout}]}` under `hooks.<event>`
/// (Claude Code-style schema, also used by Codex hooks.json, Gemini, Qwen, Grok).
fn append_claude_style(
    root: &mut Value,
    event: &str,
    matcher: &str,
    command: &str,
    timeout: u64,
    timeout_key: &str,
) {
    if !root.is_object() {
        *root = json!({});
    }
    let hooks = root.as_object_mut().unwrap().entry("hooks").or_insert(json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let arr = hooks.as_object_mut().unwrap().entry(event).or_insert(json!([]));
    if !arr.is_array() {
        *arr = json!([]);
    }
    arr.as_array_mut().unwrap().push(json!({
        "matcher": matcher,
        "hooks": [{"type": "command", "command": command, timeout_key: timeout}]
    }));
}

pub fn install(tool_id: &str) -> Result<HookReport> {
    let h = home();
    let cmd = hook_command(tool_id);
    let marker = format!("hook {tool_id}");
    let done = |path: PathBuf, outcome: &'static str, note: &str| {
        Ok(HookReport {
            tool: tool_static(tool_id),
            path,
            outcome,
            note: note.into(),
        })
    };
    match tool_id {
        // Single shared settings file — read, append our entries, write back (official merge semantics).
        "claude-code" | "gemini" | "qwen" => {
            let (path, tool_ev, tool_matcher, end_matcher, tkey, t) = match tool_id {
                "claude-code" => (
                    h.join(".claude/settings.json"),
                    "PostToolUse",
                    "Write|Edit|MultiEdit|NotebookEdit",
                    "*",
                    "timeout",
                    10u64,
                ),
                "gemini" => (
                    h.join(".gemini/settings.json"),
                    "AfterTool",
                    "write_file|replace",
                    "*",
                    "timeout",
                    10_000u64,
                ),
                _ => (
                    h.join(".qwen/settings.json"),
                    "PostToolUse",
                    "write_file|replace",
                    "",
                    "timeout",
                    10_000u64,
                ),
            };
            let mut v = read_json(&path)?;
            if v.get("hooks").map(|hk| has_ours(hk, &marker)).unwrap_or(false) {
                return done(path, "already", "已存在 sluice hook 条目");
            }
            append_claude_style(&mut v, tool_ev, tool_matcher, &cmd, t, tkey);
            append_claude_style(&mut v, "SessionEnd", end_matcher, &cmd, t, tkey);
            write_json(&path, &v, true)?;
            done(path, "appended", "在现有 hooks 后追加（保留原有条目，已备份）")
        }
        // Codex: dedicated ~/.codex/hooks.json (merged by Codex with config.toml [hooks]).
        "codex" => {
            let path = h.join(".codex/hooks.json");
            let mut v = read_json(&path)?;
            if v.get("hooks").map(|hk| has_ours(hk, &marker)).unwrap_or(false) {
                return done(path, "already", "已存在 sluice hook 条目");
            }
            append_claude_style(
                &mut v,
                "PostToolUse",
                "apply_patch|Edit|Write",
                &cmd,
                30,
                "timeout",
            );
            append_claude_style(&mut v, "SessionEnd", "*", &cmd, 30, "timeout");
            let existed = path.exists();
            write_json(&path, &v, existed)?;
            done(
                path,
                if existed { "appended" } else { "created" },
                "独立 hooks.json，不碰 config.toml",
            )
        }
        // Grok Build: hooks directory, one file per provider.
        "grok-build" => {
            let path = h.join(".grok/hooks/sluice.json");
            if path.exists() {
                return done(path, "already", "sluice.json 已存在");
            }
            let mut v = json!({});
            append_claude_style(&mut v, "PostToolUse", "Edit|Write|MultiEdit", &cmd, 10, "timeout");
            append_claude_style(&mut v, "SessionEnd", "", &cmd, 10, "timeout");
            write_json(&path, &v, false)?;
            done(path, "created", "目录多文件模式，不改动已有 hook 文件")
        }
        // Copilot CLI: hooks directory, PascalCase events → Claude-compatible payload.
        "copilot" => {
            let path = h.join(".copilot/hooks/sluice.json");
            if path.exists() {
                return done(path, "already", "sluice.json 已存在");
            }
            let entry = |matcher: &str| json!({"type": "command", "bash": cmd, "powershell": cmd, "matcher": matcher, "timeoutSec": 30});
            let v = json!({"version": 1, "hooks": {"PostToolUse": [entry("Edit|Write")], "SessionEnd": [entry("")]}});
            write_json(&path, &v, false)?;
            done(
                path,
                "created",
                "目录多文件模式（PascalCase 事件名 → Claude 兼容载荷）",
            )
        }
        // Kimi Code: single global TOML, [[hooks]] array of tables (exactly 4 keys allowed).
        "kimi" => {
            let path = h.join(".kimi-code/config.toml");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.contains(&marker) {
                return done(path, "already", "已存在 sluice hook 条目");
            }
            let mut doc: toml_edit::DocumentMut = text
                .parse()
                .with_context(|| format!("parsing {}", path.display()))?;
            let mut arr = match doc.get("hooks").and_then(|i| i.as_array_of_tables()).cloned() {
                Some(a) => a,
                None => toml_edit::ArrayOfTables::new(),
            };
            for (event, matcher) in [("PostToolUse", "Edit"), ("SessionEnd", "")] {
                let mut t = toml_edit::Table::new();
                t["event"] = toml_edit::value(event);
                t["matcher"] = toml_edit::value(matcher);
                t["command"] = toml_edit::value(cmd.clone());
                t["timeout"] = toml_edit::value(10);
                arr.push(t);
            }
            doc["hooks"] = toml_edit::Item::ArrayOfTables(arr);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if path.exists() {
                let _ = std::fs::copy(&path, path.with_extension("toml.sluice.bak"));
            }
            std::fs::write(&path, doc.to_string())?;
            done(path, "appended", "追加 [[hooks]] 表（已备份）")
        }
        "opencode" => done(
            h.join(".config/opencode/plugin/sluice.ts"),
            "unsupported",
            "opencode 走 JS 插件机制，需单独的转发插件（计划中）",
        ),
        "zcode" => done(h.join(".zcode"), "unsupported", "Z Code 未提供 hooks"),
        other => bail!("unknown tool {other}"),
    }
}

fn tool_static(id: &str) -> &'static str {
    crate::connect::TOOLS
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.name)
        .unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_is_idempotent_and_preserves_existing() {
        let mut v = serde_json::json!({"theme":"x","hooks":{"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}});
        append_claude_style(
            &mut v,
            "PostToolUse",
            "Edit",
            "/bin/sluice hook claude-code",
            10,
            "timeout",
        );
        assert_eq!(v["hooks"]["PostToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(v["theme"], "x");
        assert!(has_ours(&v["hooks"], "hook claude-code"));
        assert!(!has_ours(&v["hooks"], "hook codex"));
    }
}
