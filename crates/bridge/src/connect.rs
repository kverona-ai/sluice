//! One-click AI tool hookup (03 §2 "zero configuration"): detect installed AI
//! CLIs, tell whether Sluice is already registered as an MCP server in each,
//! and register it — preferring the tool's own `mcp add` CLI command, falling
//! back to a backed-up JSON/TOML edit. Only the `sluice mcp serve` stdio entry
//! is written; no API keys, no hooks yet (hooks arrive with provenance).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

pub const SERVER_NAME: &str = "sluice";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigFormat {
    /// `{"mcpServers": {name: {command, args}}}` (+ `"type": "local"` for Copilot CLI)
    JsonMcpServers,
    /// opencode: `{"mcp": {name: {type: "local", command: [..], enabled: true}}}`
    JsonOpencode,
    /// Z Code: `{"mcp": {"servers": {name: {command, args, env}}}}`
    JsonZcode,
    /// `[mcp_servers.name] command = ".." args = [..]`
    TomlMcpServers,
}

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub bin: &'static str,
    /// User-level config file (relative to $HOME).
    pub config_rel: &'static str,
    pub format: ConfigFormat,
    /// Official registration command template; `{cmd}` / `{args}` get expanded.
    /// Preferred over editing files when the binary supports it.
    pub cli_add: Option<&'static [&'static str]>,
    pub cli_list: Option<&'static [&'static str]>,
    /// Whether Sluice may edit the config file itself when no CLI command exists or it
    /// fails. `false` for files the tool continuously rewrites (Claude Code's
    /// `~/.claude.json`, Grok Build's `config.toml`) — those are CLI-only.
    pub file_edit: bool,
    /// Extra `"type"` value some JSON formats require inside the entry.
    pub json_type: Option<&'static str>,
}

pub const TOOLS: &[ToolSpec] = &[
    // ~/.claude.json is a live state file (sessions, OAuth, trust): CLI only.
    ToolSpec {
        id: "claude-code",
        name: "Claude Code",
        bin: "claude",
        config_rel: ".claude.json",
        format: ConfigFormat::JsonMcpServers,
        cli_add: Some(&[
            "mcp",
            "add",
            "--transport",
            "stdio",
            "--scope",
            "user",
            SERVER_NAME,
            "--",
            "{cmd}",
            "{args}",
        ]),
        cli_list: Some(&["mcp", "list"]),
        file_edit: false,
        json_type: None,
    },
    // codex mcp add has no --scope flag: always the user-level config.toml.
    ToolSpec {
        id: "codex",
        name: "Codex CLI",
        bin: "codex",
        config_rel: ".codex/config.toml",
        format: ConfigFormat::TomlMcpServers,
        cli_add: Some(&["mcp", "add", SERVER_NAME, "--", "{cmd}", "{args}"]),
        cli_list: Some(&["mcp", "list"]),
        file_edit: true,
        json_type: None,
    },
    // Gemini defaults to scope=project — pass user explicitly.
    ToolSpec {
        id: "gemini",
        name: "Gemini CLI",
        bin: "gemini",
        config_rel: ".gemini/settings.json",
        format: ConfigFormat::JsonMcpServers,
        cli_add: Some(&[
            "mcp",
            "add",
            "--scope",
            "user",
            SERVER_NAME,
            "{cmd}",
            "--",
            "{args}",
        ]),
        cli_list: Some(&["mcp", "list"]),
        file_edit: true,
        json_type: None,
    },
    // Qwen Code (Gemini CLI fork) defaults to scope=user; same syntax.
    ToolSpec {
        id: "qwen",
        name: "Qwen Code",
        bin: "qwen",
        config_rel: ".qwen/settings.json",
        format: ConfigFormat::JsonMcpServers,
        cli_add: Some(&[
            "mcp",
            "add",
            "--scope",
            "user",
            SERVER_NAME,
            "{cmd}",
            "--",
            "{args}",
        ]),
        cli_list: Some(&["mcp", "list"]),
        file_edit: true,
        json_type: None,
    },
    // Copilot CLI: user-level mcp-config.json, entries carry "type": "local".
    ToolSpec {
        id: "copilot",
        name: "Copilot CLI",
        bin: "copilot",
        config_rel: ".copilot/mcp-config.json",
        format: ConfigFormat::JsonMcpServers,
        cli_add: Some(&["mcp", "add", SERVER_NAME, "--", "{cmd}", "{args}"]),
        cli_list: Some(&["mcp", "list"]),
        file_edit: true,
        json_type: Some("local"),
    },
    // opencode's `mcp add` is interactive only: file edit, partial merge.
    ToolSpec {
        id: "opencode",
        name: "opencode",
        bin: "opencode",
        config_rel: ".config/opencode/opencode.json",
        format: ConfigFormat::JsonOpencode,
        cli_add: None,
        cli_list: Some(&["mcp", "list"]),
        file_edit: true,
        json_type: None,
    },
    // Kimi Code CLI (current) — no `kimi mcp` subcommand; MCP-only file.
    // (legacy kimi-cli used ~/.kimi/mcp.json; resolved in `config_path`.)
    ToolSpec {
        id: "kimi",
        name: "Kimi Code",
        bin: "kimi",
        config_rel: ".kimi-code/mcp.json",
        format: ConfigFormat::JsonMcpServers,
        cli_add: None,
        cli_list: None,
        file_edit: true,
        json_type: None,
    },
    // Grok Build rewrites config.toml at runtime ([hints]…): CLI only.
    ToolSpec {
        id: "grok-build",
        name: "Grok Build",
        bin: "grok",
        config_rel: ".grok/config.toml",
        format: ConfigFormat::TomlMcpServers,
        cli_add: Some(&["mcp", "add", SERVER_NAME, "--", "{cmd}", "{args}"]),
        cli_list: Some(&["mcp", "list"]),
        file_edit: false,
        json_type: None,
    },
    // Z Code is a desktop app with a CLI config dir; file edit only.
    ToolSpec {
        id: "zcode",
        name: "Z Code",
        bin: "zcode",
        config_rel: ".zcode/cli/config.json",
        format: ConfigFormat::JsonZcode,
        cli_add: None,
        cli_list: None,
        file_edit: true,
        json_type: None,
    },
    // DeepSeek Harness: MCP registration not documented upstream yet — deliberately absent.
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Registration {
    NotInstalled,
    /// Installed, config file absent or without a sluice entry.
    Installed,
    /// A `sluice` MCP entry exists (command shown).
    Registered(String),
}

#[derive(Clone, Debug)]
pub struct ToolStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub config_path: PathBuf,
    pub state: Registration,
}

/// `$SLUICE_CONFIG_HOME` overrides the home directory (tests / dry runs).
fn home() -> PathBuf {
    std::env::var_os("SLUICE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the config file, honoring the legacy kimi-cli layout when only it exists.
pub fn config_path(spec: &ToolSpec) -> PathBuf {
    let h = home();
    if spec.id == "kimi" && !h.join(".kimi-code").exists() && h.join(".kimi").exists() {
        return h.join(".kimi/mcp.json");
    }
    h.join(spec.config_rel)
}

fn tool_path(spec: &ToolSpec) -> Option<PathBuf> {
    which::which_in(
        spec.bin,
        Some(sluice_backend_cli::env::login_path()),
        std::env::current_dir().ok()?,
    )
    .ok()
}

/// Command Sluice registers: the running executable's absolute path (works for GUI-
/// launched tools without a shell PATH) + `mcp serve`. `--repo` is intentionally
/// omitted so the server follows the client's working directory.
pub fn server_command() -> (String, Vec<String>) {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "sluice".into());
    (exe, vec!["mcp".into(), "serve".into()])
}

pub fn status_all() -> Vec<ToolStatus> {
    TOOLS.iter().map(status).collect()
}

pub fn status(spec: &ToolSpec) -> ToolStatus {
    let config_path = config_path(spec);
    let state = if tool_path(spec).is_none() {
        Registration::NotInstalled
    } else {
        match read_registration(&config_path, spec.format) {
            Some(cmd) => Registration::Registered(cmd),
            None => Registration::Installed,
        }
    };
    ToolStatus {
        id: spec.id,
        name: spec.name,
        config_path,
        state,
    }
}

fn read_registration(path: &Path, format: ConfigFormat) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    match format {
        ConfigFormat::JsonMcpServers => {
            let v: Value = serde_json::from_str(&text).ok()?;
            let e = v.get("mcpServers")?.get(SERVER_NAME)?;
            Some(
                e.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
            )
        }
        ConfigFormat::JsonZcode => {
            let v: Value = serde_json::from_str(&text).ok()?;
            let e = v.get("mcp")?.get("servers")?.get(SERVER_NAME)?;
            Some(
                e.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
            )
        }
        ConfigFormat::JsonOpencode => {
            let v: Value = serde_json::from_str(&text).ok()?;
            let e = v.get("mcp")?.get(SERVER_NAME)?;
            Some(
                e.get("command")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
                    .unwrap_or_else(|| "?".into()),
            )
        }
        ConfigFormat::TomlMcpServers => {
            let doc: toml_edit::DocumentMut = text.parse().ok()?;
            let e = doc.get("mcp_servers")?.get(SERVER_NAME)?;
            Some(
                e.get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("?")
                    .to_string(),
            )
        }
    }
}

#[derive(Clone, Debug)]
pub struct RegisterReport {
    pub tool: &'static str,
    pub method: &'static str,
    pub config_path: PathBuf,
    pub backup: Option<PathBuf>,
    pub verified: Option<bool>,
}

/// Register Sluice with one tool. Never touches anything but the `sluice` entry.
pub fn register(spec: &ToolSpec) -> Result<RegisterReport> {
    let Some(bin) = tool_path(spec) else {
        bail!("{} 未安装（PATH 上找不到 {}）", spec.name, spec.bin)
    };
    let (cmd, args) = server_command();
    let config_path = config_path(spec);

    // 1) the tool's own CLI, when it has one — safest for files it also writes (Claude Code).
    if let Some(tpl) = spec.cli_add {
        let mut argv: Vec<String> = Vec::new();
        for a in tpl {
            match *a {
                "{cmd}" => argv.push(cmd.clone()),
                "{args}" => argv.extend(args.iter().cloned()),
                other => argv.push(other.to_string()),
            }
        }
        let out = Command::new(&bin)
            .args(&argv)
            .env("PATH", sluice_backend_cli::env::login_path())
            .output()
            .with_context(|| format!("running {} mcp add", spec.bin))?;
        if out.status.success() || read_registration(&config_path, spec.format).is_some() {
            let verified = spec.cli_list.map(|l| {
                Command::new(&bin)
                    .args(l)
                    .env("PATH", sluice_backend_cli::env::login_path())
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).contains(SERVER_NAME))
                    .unwrap_or(false)
            });
            return Ok(RegisterReport {
                tool: spec.name,
                method: "cli",
                config_path,
                backup: None,
                verified,
            });
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        tracing::warn!("{} mcp add failed: {err}", spec.bin);
        if !spec.file_edit {
            bail!(
                "{} mcp add 失败：{err}（该工具的配置文件由其自身维护，Sluice 不直接改写）",
                spec.bin
            );
        }
        // fall through to the file edit
    }
    if !spec.file_edit {
        bail!("{} 没有可用的注册命令", spec.name);
    }

    // 2) backed-up config edit.
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&config_path).ok();
    let backup = existing.as_ref().map(|text| {
        let b = config_path.with_extension(format!(
            "{}sluice.bak",
            config_path
                .extension()
                .map(|e| format!("{}.", e.to_string_lossy()))
                .unwrap_or_default()
        ));
        let _ = std::fs::write(&b, text);
        b
    });
    let new_text = match spec.format {
        ConfigFormat::JsonMcpServers => {
            let mut v: Value = existing
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or(json!({}));
            if !v.is_object() {
                bail!("{} 不是 JSON 对象", config_path.display());
            }
            let servers = v
                .as_object_mut()
                .unwrap()
                .entry("mcpServers")
                .or_insert(json!({}));
            if !servers.is_object() {
                *servers = json!({});
            }
            servers[SERVER_NAME] = match spec.json_type {
                Some(ty) => json!({"type": ty, "command": cmd, "args": args}),
                None => json!({"command": cmd, "args": args}),
            };
            serde_json::to_string_pretty(&v)? + "\n"
        }
        ConfigFormat::JsonZcode => {
            let mut v: Value = existing
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or(json!({}));
            if !v.is_object() {
                bail!("{} 不是 JSON 对象", config_path.display());
            }
            let mcp = v.as_object_mut().unwrap().entry("mcp").or_insert(json!({}));
            if !mcp.is_object() {
                *mcp = json!({});
            }
            let servers = mcp.as_object_mut().unwrap().entry("servers").or_insert(json!({}));
            if !servers.is_object() {
                *servers = json!({});
            }
            servers[SERVER_NAME] = json!({"command": cmd, "args": args, "env": {}});
            serde_json::to_string_pretty(&v)? + "\n"
        }
        ConfigFormat::JsonOpencode => {
            let mut v: Value = existing
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or(json!({}));
            if !v.is_object() {
                bail!("{} 不是 JSON 对象", config_path.display());
            }
            let mcp = v.as_object_mut().unwrap().entry("mcp").or_insert(json!({}));
            if !mcp.is_object() {
                *mcp = json!({});
            }
            let mut full = vec![cmd.clone()];
            full.extend(args.iter().cloned());
            mcp[SERVER_NAME] = json!({"type": "local", "command": full, "enabled": true});
            serde_json::to_string_pretty(&v)? + "\n"
        }
        ConfigFormat::TomlMcpServers => {
            let mut doc: toml_edit::DocumentMut = existing.as_deref().unwrap_or("").parse()?;
            if doc.get("mcp_servers").is_none() {
                doc["mcp_servers"] = toml_edit::table();
            }
            let mut entry = toml_edit::Table::new();
            entry["command"] = toml_edit::value(cmd.clone());
            let mut arr = toml_edit::Array::new();
            for a in &args {
                arr.push(a.as_str());
            }
            entry["args"] = toml_edit::value(arr);
            doc["mcp_servers"][SERVER_NAME] = toml_edit::Item::Table(entry);
            doc.to_string()
        }
    };
    std::fs::write(&config_path, new_text).with_context(|| format!("writing {}", config_path.display()))?;
    Ok(RegisterReport {
        tool: spec.name,
        method: "file",
        config_path,
        backup,
        verified: None,
    })
}

/// Remove the `sluice` entry again (file-edit path only; CLI `mcp remove` where available).
pub fn unregister(spec: &ToolSpec) -> Result<()> {
    let config_path = config_path(spec);
    let Some(text) = std::fs::read_to_string(&config_path).ok() else {
        return Ok(());
    };
    let new_text = match spec.format {
        ConfigFormat::JsonMcpServers | ConfigFormat::JsonOpencode => {
            let mut v: Value = serde_json::from_str(&text)?;
            let key = if spec.format == ConfigFormat::JsonOpencode {
                "mcp"
            } else {
                "mcpServers"
            };
            if let Some(obj) = v.get_mut(key).and_then(Value::as_object_mut) {
                obj.remove(SERVER_NAME);
            }
            serde_json::to_string_pretty(&v)? + "\n"
        }
        ConfigFormat::JsonZcode => {
            let mut v: Value = serde_json::from_str(&text)?;
            if let Some(obj) = v
                .get_mut("mcp")
                .and_then(|m| m.get_mut("servers"))
                .and_then(Value::as_object_mut)
            {
                obj.remove(SERVER_NAME);
            }
            serde_json::to_string_pretty(&v)? + "\n"
        }
        ConfigFormat::TomlMcpServers => {
            let mut doc: toml_edit::DocumentMut = text.parse()?;
            if let Some(t) = doc.get_mut("mcp_servers").and_then(|i| i.as_table_mut()) {
                t.remove(SERVER_NAME);
            }
            doc.to_string()
        }
    };
    std::fs::write(&config_path, new_text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_registration_roundtrip() {
        let text = "[model]\nname = \"x\"\n";
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        doc["mcp_servers"] = toml_edit::table();
        let mut entry = toml_edit::Table::new();
        entry["command"] = toml_edit::value("/usr/local/bin/sluice");
        doc["mcp_servers"][SERVER_NAME] = toml_edit::Item::Table(entry);
        let out = doc.to_string();
        assert!(out.contains("[mcp_servers.sluice]"));
        assert!(out.contains("[model]"));
        let back: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(
            back["mcp_servers"]["sluice"]["command"].as_str(),
            Some("/usr/local/bin/sluice")
        );
    }

    #[test]
    fn json_registration_preserves_siblings() {
        let v: Value =
            serde_json::from_str(r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#).unwrap();
        let mut v = v;
        v["mcpServers"][SERVER_NAME] = json!({"command": "sluice", "args": ["mcp", "serve"]});
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["sluice"]["args"][1], "serve");
    }
}
