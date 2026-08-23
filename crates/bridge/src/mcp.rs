//! Minimal MCP server, stdio transport (newline-delimited JSON-RPC 2.0).
//! Read-only tools: repo_status · list_changes · get_diff · log_query.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};
use sluice_backend_cli::GitCli;
use sluice_backend_gix::GixReader;
use sluice_core::diff::{DiffOptions, diff_bytes};
use sluice_core::*;

const PROTOCOL_VERSION: &str = "2025-06-18";
/// get_diff output cap (05 §7.2: oversized output is truncated, never streamed).
const MAX_DIFF_OUT: usize = 128 * 1024;

pub struct McpServer {
    repo_path: PathBuf,
}

impl McpServer {
    pub fn new(repo_path: PathBuf) -> Self {
        Self { repo_path }
    }

    /// Blocking serve loop over stdin/stdout. Returns when stdin closes.
    pub fn serve(&self) -> Result<()> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let msg: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("mcp: bad json: {e}");
                    continue;
                }
            };
            let Some(method) = msg.get("method").and_then(Value::as_str) else {
                continue;
            };
            let id = msg.get("id").cloned();
            // Notifications (no id) need no response.
            let Some(id) = id else { continue };
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let result = self.dispatch(method, &params);
            let response = match result {
                Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
                Err(e) => {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": format!("{e:#}")}})
                }
            };
            let mut out = stdout.lock();
            serde_json::to_writer(&mut out, &response)?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
        Ok(())
    }

    fn dispatch(&self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "sluice", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Read-only view of the git repository Sluice has open. Write operations are proposals reviewed by a human in Sluice; destructive git operations are not exposed at all."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools_list()),
            "tools/call" => self.tool_call(params),
            _ => anyhow::bail!("method {method} not supported"),
        }
    }

    fn reader(&self) -> Result<GixReader> {
        GixReader::discover(&self.repo_path, Console::new())
    }

    fn tool_call(&self, params: &Value) -> Result<Value> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let value = match name {
            "repo_status" => self.repo_status()?,
            "list_changes" => self.list_changes(&args)?,
            "get_diff" => self.get_diff(&args)?,
            "log_query" => self.log_query(&args)?,
            other => anyhow::bail!("unknown tool {other}"),
        };
        Ok(json!({
            "content": [{"type": "text", "text": serde_json::to_string_pretty(&value)?}],
            "structuredContent": value,
            "isError": false
        }))
    }

    fn cli(&self) -> Result<Option<GitCli>> {
        let reader = self.reader()?;
        let info = reader.info()?;
        Ok(match info.workdir {
            Some(wd) => Some(GitCli::new(wd, info.git_dir, Console::new())?),
            None => None,
        })
    }

    fn repo_status(&self) -> Result<Value> {
        let reader = self.reader()?;
        let info = reader.info()?;
        let (entries, in_progress) = match self.cli()? {
            Some(cli) => {
                let st = cli.status()?;
                (Some(st.entries), st.in_progress)
            }
            None => (None, None),
        };
        let split = |pred: &dyn Fn(&StatusEntry) -> bool| -> Vec<String> {
            entries
                .iter()
                .flatten()
                .filter(|e| pred(e))
                .map(|e| e.path.clone())
                .collect()
        };
        Ok(json!({
            "root": info.workdir.as_ref().unwrap_or(&info.git_dir),
            "branch": info.head.branch,
            "detached": info.head.detached,
            "upstream": info.head.upstream,
            "ahead": info.head.ahead,
            "behind": info.head.behind,
            "staged": split(&|e| e.is_staged() && e.conflict.is_none()),
            "unstaged": split(&|e| e.unstaged.is_some() && !e.untracked && e.conflict.is_none()),
            "untracked": split(&|e| e.untracked),
            "conflicts": split(&|e| e.conflict.is_some()),
            "in_progress_op": in_progress.map(|op| format!("{op:?}").to_lowercase()),
        }))
    }

    fn list_changes(&self, args: &Value) -> Result<Value> {
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("all");
        let Some(cli) = self.cli()? else {
            anyhow::bail!("bare repository has no worktree")
        };
        let st = cli.status()?;
        let entries: Vec<Value> = st
            .entries
            .iter()
            .filter(|e| match kind {
                "staged" => e.is_staged(),
                "unstaged" => e.unstaged.is_some() && !e.untracked,
                "untracked" => e.untracked,
                "conflicts" => e.conflict.is_some(),
                _ => true,
            })
            .map(|e| {
                json!({
                    "path": e.path,
                    "old_path": e.old_path,
                    "staged": e.staged.map(|k| k.mark()),
                    "unstaged": e.unstaged.map(|k| k.mark()),
                    "untracked": e.untracked,
                    "conflict": e.conflict,
                })
            })
            .collect();
        Ok(json!({"entries": entries}))
    }

    fn get_diff(&self, args: &Value) -> Result<Value> {
        let scope = args.get("scope").and_then(Value::as_str).unwrap_or("unstaged");
        let reader = self.reader()?;
        let paths: Option<Vec<String>> = args
            .get("paths")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect());
        let opts = DiffOptions {
            context: args.get("context").and_then(Value::as_u64).unwrap_or(3) as usize,
            ..Default::default()
        };
        let mut out = String::new();
        let mut files = Vec::new();
        let mut push_file = |path: &str, old: &[u8], new: &[u8]| {
            let d = diff_bytes(old, new, &opts);
            if d.is_empty() && !d.binary {
                return;
            }
            files.push(json!({"path": path, "additions": d.additions(), "deletions": d.deletions(), "binary": d.binary}));
            out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
            for h in &d.hunks {
                out.push_str(&h.header());
                out.push('\n');
                for l in &h.lines {
                    let sign = match l.kind {
                        sluice_core::diff::LineKind::Context => ' ',
                        sluice_core::diff::LineKind::Added => '+',
                        sluice_core::diff::LineKind::Deleted => '-',
                    };
                    out.push(sign);
                    out.push_str(&l.text);
                    out.push('\n');
                }
            }
        };
        match scope {
            "commit" => {
                let rev = args.get("rev").and_then(Value::as_str).unwrap_or("HEAD");
                let commits = reader.log(&LogQuery {
                    limit: 1,
                    ..Default::default()
                })?;
                let id = if rev == "HEAD" {
                    commits
                        .first()
                        .map(|c| c.id.clone())
                        .ok_or_else(|| anyhow::anyhow!("empty repository"))?
                } else {
                    Oid::new(rev)
                };
                for ch in reader.commit_changes(&id)? {
                    if paths
                        .as_ref()
                        .is_some_and(|ps| !ps.iter().any(|p| ch.path.starts_with(p)))
                    {
                        continue;
                    }
                    let old = reader
                        .blob(
                            &BlobRev::ParentOf(id.clone()),
                            ch.old_path.as_deref().unwrap_or(&ch.path),
                        )?
                        .unwrap_or_default();
                    let new = reader
                        .blob(&BlobRev::Commit(id.clone()), &ch.path)?
                        .unwrap_or_default();
                    push_file(&ch.path, &old, &new);
                }
            }
            staged_or_unstaged => {
                let staged = staged_or_unstaged == "staged";
                let Some(cli) = self.cli()? else {
                    anyhow::bail!("bare repository has no worktree")
                };
                let st = cli.status()?;
                for e in st
                    .entries
                    .iter()
                    .filter(|e| if staged { e.is_staged() } else { e.is_unstaged() })
                {
                    if paths
                        .as_ref()
                        .is_some_and(|ps| !ps.iter().any(|p| e.path.starts_with(p)))
                    {
                        continue;
                    }
                    let (old_rev, new_rev) = if staged {
                        (BlobRev::Head, BlobRev::Index)
                    } else {
                        (BlobRev::Index, BlobRev::Worktree)
                    };
                    let old = reader
                        .blob(&old_rev, e.old_path.as_deref().unwrap_or(&e.path))?
                        .unwrap_or_default();
                    let new = reader.blob(&new_rev, &e.path)?.unwrap_or_default();
                    push_file(&e.path, &old, &new);
                }
            }
        }
        let truncated = out.len() > MAX_DIFF_OUT;
        if truncated {
            let mut end = MAX_DIFF_OUT;
            while end > 0 && !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            out.push_str("\n… (truncated)\n");
        }
        Ok(json!({"diff": out, "truncated": truncated, "files": files}))
    }

    fn log_query(&self, args: &Value) -> Result<Value> {
        let reader = self.reader()?;
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50).min(500) as usize;
        let text = args.get("text").and_then(Value::as_str).map(str::to_lowercase);
        let author = args.get("author").and_then(Value::as_str).map(str::to_lowercase);
        // Over-fetch so post-filtering still fills `limit`.
        let commits = reader.log(&LogQuery {
            limit: limit.max(200) * 4,
            ..Default::default()
        })?;
        let out: Vec<Value> = commits
            .iter()
            .filter(|c| {
                text.as_ref()
                    .is_none_or(|t| c.summary.to_lowercase().contains(t) || c.id.as_str().starts_with(t))
                    && author
                        .as_ref()
                        .is_none_or(|a| c.author.name.to_lowercase().contains(a))
            })
            .take(limit)
            .map(|c| {
                json!({
                    "sha": c.id.as_str(),
                    "subject": c.summary,
                    "author": c.author.name,
                    "date": c.author.time.to_rfc3339(),
                    "agent": if c.agent.is_ai() { Some(c.agent.label()) } else { None },
                    "parents": c.parents.iter().map(|p| p.short(12)).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({"commits": out}))
    }
}

fn tools_list() -> Value {
    let paths_prop = json!({"type": "array", "items": {"type": "string"}, "description": "restrict to these path prefixes"});
    json!({"tools": [
        {
            "name": "repo_status",
            "description": "Repository overview: branch, upstream, ahead/behind, staged/unstaged/untracked/conflicted paths, any in-progress operation.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "list_changes",
            "description": "Working-tree changes. kind: all | staged | unstaged | untracked | conflicts.",
            "inputSchema": {"type": "object", "properties": {"kind": {"type": "string", "enum": ["all", "staged", "unstaged", "untracked", "conflicts"]}}, "additionalProperties": false}
        },
        {
            "name": "get_diff",
            "description": "Unified diff. scope: staged (index vs HEAD) | unstaged (worktree vs index) | commit (rev vs its first parent). Large output is truncated.",
            "inputSchema": {"type": "object", "properties": {
                "scope": {"type": "string", "enum": ["staged", "unstaged", "commit"]},
                "rev": {"type": "string", "description": "commit sha when scope=commit (default HEAD)"},
                "paths": paths_prop,
                "context": {"type": "integer", "minimum": 0, "maximum": 40}
            }, "additionalProperties": false}
        },
        {
            "name": "log_query",
            "description": "Recent commits with agent attribution. Filters: text (subject or sha prefix), author. limit ≤ 500.",
            "inputSchema": {"type": "object", "properties": {
                "text": {"type": "string"},
                "author": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500}
            }, "additionalProperties": false}
        }
    ]})
}
