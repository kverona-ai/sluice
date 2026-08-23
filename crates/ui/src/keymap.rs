//! Keymap presets + user overrides (05 §11). The IDEA preset is the default;
//! the VS Code preset overrides a handful of chords; `~/.sluice/keymap.json`
//! (`{"ActionName": "cmd-shift-x"}`) wins over both. Later bindings take
//! precedence in gpui, so presets/overrides are simply bound after the base.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{App, KeyBinding};

pub struct Entry {
    pub action: &'static str,
    pub label: &'static str,
    pub keys: &'static [&'static str],
    /// Bound in the `Workbench && !Input` context (single keys that must not steal typing).
    pub nav: bool,
}

/// IDEA preset — the base table (macOS chords first, Windows/Linux `ctrl` variants after).
pub const IDEA: &[Entry] = &[
    Entry {
        action: "MoveUp",
        label: "上移选择",
        keys: &["up", "k"],
        nav: true,
    },
    Entry {
        action: "MoveDown",
        label: "下移选择",
        keys: &["down", "j"],
        nav: true,
    },
    Entry {
        action: "SelectFirst",
        label: "首项",
        keys: &["home"],
        nav: true,
    },
    Entry {
        action: "SelectLast",
        label: "末项",
        keys: &["end"],
        nav: true,
    },
    Entry {
        action: "PageUp",
        label: "上翻页",
        keys: &["pageup"],
        nav: true,
    },
    Entry {
        action: "PageDown",
        label: "下翻页",
        keys: &["pagedown"],
        nav: true,
    },
    Entry {
        action: "Escape",
        label: "关闭弹层 / diff",
        keys: &["escape", "ctrl-["],
        nav: false,
    },
    Entry {
        action: "ToggleSelected",
        label: "切换暂存 / 切换动作",
        keys: &["space"],
        nav: true,
    },
    Entry {
        action: "NextHunk",
        label: "下一处差异",
        keys: &["f7"],
        nav: true,
    },
    Entry {
        action: "PrevHunk",
        label: "上一处差异",
        keys: &["shift-f7"],
        nav: true,
    },
    Entry {
        action: "ShowLog",
        label: "日志",
        keys: &["cmd-9", "alt-9"],
        nav: false,
    },
    Entry {
        action: "ShowChanges",
        label: "本地变更",
        keys: &["cmd-0", "alt-0"],
        nav: false,
    },
    Entry {
        action: "ShowConsole",
        label: "Console",
        keys: &["cmd-6", "alt-6"],
        nav: false,
    },
    Entry {
        action: "Refresh",
        label: "刷新",
        keys: &["cmd-alt-y", "ctrl-alt-y"],
        nav: false,
    },
    Entry {
        action: "FocusSearch",
        label: "搜索提交",
        keys: &["cmd-f", "ctrl-f"],
        nav: false,
    },
    Entry {
        action: "FocusCommit",
        label: "提交面板",
        keys: &["cmd-k", "ctrl-k"],
        nav: false,
    },
    Entry {
        action: "CommitAction",
        label: "提交",
        keys: &["cmd-enter", "ctrl-enter"],
        nav: false,
    },
    Entry {
        action: "StageAll",
        label: "全部暂存",
        keys: &["cmd-alt-a", "ctrl-alt-a"],
        nav: false,
    },
    Entry {
        action: "UnstageAll",
        label: "全部取消暂存",
        keys: &["cmd-alt-u", "ctrl-alt-u"],
        nav: false,
    },
    Entry {
        action: "ToggleSideBySide",
        label: "双栏 / 统一",
        keys: &["cmd-alt-\\\\", "ctrl-alt-\\\\"],
        nav: false,
    },
    Entry {
        action: "OpenBranches",
        label: "分支面板",
        keys: &["ctrl-shift-`", "ctrl-~", "cmd-b", "ctrl-b"],
        nav: false,
    },
    Entry {
        action: "OpenStash",
        label: "Stash 面板",
        keys: &["cmd-5", "ctrl-5"],
        nav: false,
    },
    Entry {
        action: "OpenSnapshots",
        label: "时光机",
        keys: &["cmd-7", "ctrl-7"],
        nav: false,
    },
    Entry {
        action: "OpenSettings",
        label: "设置",
        keys: &["cmd-,"],
        nav: false,
    },
    Entry {
        action: "OpenPush",
        label: "Push 对话框",
        keys: &["cmd-shift-k", "ctrl-shift-k"],
        nav: false,
    },
    Entry {
        action: "ToggleTheme",
        label: "深色 / 浅色",
        keys: &["cmd-shift-t", "ctrl-shift-t"],
        nav: false,
    },
    Entry {
        action: "ToggleLang",
        label: "中文 / English",
        keys: &["cmd-shift-l", "ctrl-shift-l"],
        nav: false,
    },
    Entry {
        action: "OpenUserFilter",
        label: "作者过滤",
        keys: &["cmd-u", "ctrl-u"],
        nav: false,
    },
    Entry {
        action: "OpenFileHistory",
        label: "文件历史",
        keys: &["cmd-shift-h", "ctrl-shift-h"],
        nav: false,
    },
    Entry {
        action: "OpenAiConnect",
        label: "AI 工具接入",
        keys: &["cmd-shift-i", "ctrl-shift-i"],
        nav: false,
    },
    Entry {
        action: "OpenProposals",
        label: "待确认队列",
        keys: &["cmd-shift-p", "ctrl-shift-p"],
        nav: false,
    },
    Entry {
        action: "OpenRepository",
        label: "打开仓库",
        keys: &["cmd-o", "ctrl-o"],
        nav: false,
    },
    Entry {
        action: "OpenRecent",
        label: "最近仓库",
        keys: &["cmd-shift-o", "ctrl-shift-o"],
        nav: false,
    },
    Entry {
        action: "ProposalAccept",
        label: "接受 / 确定",
        keys: &["enter"],
        nav: true,
    },
    Entry {
        action: "RebaseMoveUp",
        label: "rebase：上移",
        keys: &["alt-up"],
        nav: true,
    },
    Entry {
        action: "ConflictOurs",
        label: "冲突：用我们的",
        keys: &["alt-1"],
        nav: true,
    },
    Entry {
        action: "ConflictTheirs",
        label: "冲突：用他们的",
        keys: &["alt-2"],
        nav: true,
    },
    Entry {
        action: "ConflictBoth",
        label: "冲突：两者",
        keys: &["alt-3"],
        nav: true,
    },
    Entry {
        action: "ConflictResolve",
        label: "保存并标记已解决",
        keys: &["cmd-s", "ctrl-s"],
        nav: false,
    },
    Entry {
        action: "RebaseFromSelection",
        label: "交互式 rebase",
        keys: &["cmd-alt-r", "ctrl-alt-r"],
        nav: false,
    },
    Entry {
        action: "RebaseMoveDown",
        label: "rebase：下移",
        keys: &["alt-down"],
        nav: true,
    },
    Entry {
        action: "ProposalReject",
        label: "拒绝",
        keys: &["delete", "backspace"],
        nav: true,
    },
    Entry {
        action: "OpenBlame",
        label: "Blame",
        keys: &["cmd-alt-b", "ctrl-alt-b"],
        nav: false,
    },
    Entry {
        action: "OpenDateFilter",
        label: "日期过滤",
        keys: &["cmd-alt-d", "ctrl-alt-d"],
        nav: false,
    },
    Entry {
        action: "OpenPathFilter",
        label: "路径过滤",
        keys: &["cmd-alt-p", "ctrl-alt-p"],
        nav: false,
    },
    Entry {
        action: "OpenMessageHistory",
        label: "最近提交信息",
        keys: &["cmd-shift-m", "ctrl-shift-m"],
        nav: false,
    },
    Entry {
        action: "OpenWorktrees",
        label: "Worktree 面板",
        keys: &["cmd-shift-w", "ctrl-shift-w"],
        nav: false,
    },
    Entry {
        action: "ToggleConsoleDock",
        label: "Console 拆分到底部 / 并回",
        keys: &["cmd-alt-6", "ctrl-alt-6"],
        nav: false,
    },
];

/// VS Code preset: only the differences from IDEA.
pub const VSCODE: &[Entry] = &[
    Entry {
        action: "ShowLog",
        label: "日志",
        keys: &["cmd-shift-g", "ctrl-shift-g"],
        nav: false,
    },
    Entry {
        action: "ShowChanges",
        label: "本地变更",
        keys: &["cmd-shift-u", "ctrl-shift-u"],
        nav: false,
    },
    Entry {
        action: "ShowConsole",
        label: "Console",
        keys: &["cmd-shift-y", "ctrl-shift-y"],
        nav: false,
    },
    Entry {
        action: "FocusSearch",
        label: "搜索提交",
        keys: &["cmd-f", "ctrl-f"],
        nav: false,
    },
    Entry {
        action: "OpenRepository",
        label: "打开仓库",
        keys: &["cmd-k cmd-o", "ctrl-k ctrl-o"],
        nav: false,
    },
    Entry {
        action: "Refresh",
        label: "刷新",
        keys: &["cmd-r", "ctrl-r"],
        nav: false,
    },
];

const CTX: Option<&str> = Some("Workbench");
const NAV: Option<&str> = Some("Workbench && !Input");

fn make(action: &str, keys: &str, nav: bool) -> Option<KeyBinding> {
    let ctx = if nav { NAV } else { CTX };
    Some(match action {
        "MoveUp" => KeyBinding::new(keys, crate::workbench::MoveUp, ctx),
        "MoveDown" => KeyBinding::new(keys, crate::workbench::MoveDown, ctx),
        "SelectFirst" => KeyBinding::new(keys, crate::workbench::SelectFirst, ctx),
        "SelectLast" => KeyBinding::new(keys, crate::workbench::SelectLast, ctx),
        "PageUp" => KeyBinding::new(keys, crate::workbench::PageUp, ctx),
        "PageDown" => KeyBinding::new(keys, crate::workbench::PageDown, ctx),
        "Escape" => KeyBinding::new(keys, crate::workbench::Escape, ctx),
        "ToggleSelected" => KeyBinding::new(keys, crate::workbench::ToggleSelected, ctx),
        "NextHunk" => KeyBinding::new(keys, crate::workbench::NextHunk, ctx),
        "PrevHunk" => KeyBinding::new(keys, crate::workbench::PrevHunk, ctx),
        "ShowLog" => KeyBinding::new(keys, crate::workbench::ShowLog, ctx),
        "ShowChanges" => KeyBinding::new(keys, crate::workbench::ShowChanges, ctx),
        "ShowConsole" => KeyBinding::new(keys, crate::workbench::ShowConsole, ctx),
        "Refresh" => KeyBinding::new(keys, crate::workbench::Refresh, ctx),
        "FocusSearch" => KeyBinding::new(keys, crate::workbench::FocusSearch, ctx),
        "FocusCommit" => KeyBinding::new(keys, crate::workbench::FocusCommit, ctx),
        "CommitAction" => KeyBinding::new(keys, crate::workbench::CommitAction, ctx),
        "StageAll" => KeyBinding::new(keys, crate::workbench::StageAll, ctx),
        "UnstageAll" => KeyBinding::new(keys, crate::workbench::UnstageAll, ctx),
        "ToggleSideBySide" => KeyBinding::new(keys, crate::workbench::ToggleSideBySide, ctx),
        "OpenBranches" => KeyBinding::new(keys, crate::workbench::OpenBranches, ctx),
        "OpenStash" => KeyBinding::new(keys, crate::workbench::OpenStash, ctx),
        "OpenSnapshots" => KeyBinding::new(keys, crate::workbench::OpenSnapshots, ctx),
        "OpenSettings" => KeyBinding::new(keys, crate::workbench::OpenSettings, ctx),
        "OpenPush" => KeyBinding::new(keys, crate::workbench::OpenPush, ctx),
        "ToggleTheme" => KeyBinding::new(keys, crate::workbench::ToggleTheme, ctx),
        "ToggleLang" => KeyBinding::new(keys, crate::workbench::ToggleLang, ctx),
        "OpenUserFilter" => KeyBinding::new(keys, crate::workbench::OpenUserFilter, ctx),
        "OpenFileHistory" => KeyBinding::new(keys, crate::workbench::OpenFileHistory, ctx),
        "OpenAiConnect" => KeyBinding::new(keys, crate::workbench::OpenAiConnect, ctx),
        "OpenProposals" => KeyBinding::new(keys, crate::workbench::OpenProposals, ctx),
        "OpenRepository" => KeyBinding::new(keys, crate::workbench::OpenRepository, ctx),
        "OpenRecent" => KeyBinding::new(keys, crate::workbench::OpenRecent, ctx),
        "ProposalAccept" => KeyBinding::new(keys, crate::workbench::ProposalAccept, ctx),
        "RebaseMoveUp" => KeyBinding::new(keys, crate::workbench::RebaseMoveUp, ctx),
        "ConflictOurs" => KeyBinding::new(keys, crate::workbench::ConflictOurs, ctx),
        "ConflictTheirs" => KeyBinding::new(keys, crate::workbench::ConflictTheirs, ctx),
        "ConflictBoth" => KeyBinding::new(keys, crate::workbench::ConflictBoth, ctx),
        "ConflictResolve" => KeyBinding::new(keys, crate::workbench::ConflictResolve, ctx),
        "RebaseFromSelection" => KeyBinding::new(keys, crate::workbench::RebaseFromSelection, ctx),
        "RebaseMoveDown" => KeyBinding::new(keys, crate::workbench::RebaseMoveDown, ctx),
        "ProposalReject" => KeyBinding::new(keys, crate::workbench::ProposalReject, ctx),
        "OpenBlame" => KeyBinding::new(keys, crate::workbench::OpenBlame, ctx),
        "OpenDateFilter" => KeyBinding::new(keys, crate::workbench::OpenDateFilter, ctx),
        "OpenPathFilter" => KeyBinding::new(keys, crate::workbench::OpenPathFilter, ctx),
        "OpenMessageHistory" => KeyBinding::new(keys, crate::workbench::OpenMessageHistory, ctx),
        "OpenWorktrees" => KeyBinding::new(keys, crate::workbench::OpenWorktrees, ctx),
        "ToggleConsoleDock" => KeyBinding::new(keys, crate::workbench::ToggleConsoleDock, ctx),
        _ => return None,
    })
}

pub fn keymap_path() -> PathBuf {
    std::env::var_os("SLUICE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sluice")
        .join("keymap.json")
}

/// `{"ActionName": "keys"}` — unknown actions are ignored, bad keystrokes are skipped.
pub fn load_overrides() -> HashMap<String, String> {
    std::fs::read_to_string(keymap_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Write a commented template listing every action with its current IDEA chord.
pub fn write_template_if_missing() -> std::io::Result<PathBuf> {
    let p = keymap_path();
    if !p.exists() {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut m = serde_json::Map::new();
        m.insert("_comment".into(), serde_json::Value::String("Override any action: \"ActionName\": \"cmd-shift-x\". Remove entries to fall back to the preset. Reload from Settings.".into()));
        for e in IDEA {
            m.insert(
                format!("_{}", e.action),
                serde_json::Value::String(e.keys.join(" | ")),
            );
        }
        std::fs::write(&p, serde_json::to_string_pretty(&serde_json::Value::Object(m))?)?;
    }
    Ok(p)
}

/// Bind the base preset (+ VS Code differences when selected) and user overrides.
pub fn bind(cx: &mut App, preset: &str, overrides: &HashMap<String, String>) {
    let mut list: Vec<KeyBinding> = Vec::new();
    for e in IDEA {
        for k in e.keys {
            list.extend(make(e.action, k, e.nav));
        }
    }
    if preset == "vscode" {
        for e in VSCODE {
            for k in e.keys {
                list.extend(make(e.action, k, e.nav));
            }
        }
    }
    for (action, keys) in overrides {
        if action.starts_with('_') {
            continue;
        }
        let nav = IDEA
            .iter()
            .find(|e| e.action == action)
            .map(|e| e.nav)
            .unwrap_or(false);
        list.extend(make(action, keys, nav));
    }
    cx.bind_keys(list);
}

/// Effective chord for display: override > preset > IDEA.
pub fn effective(action: &str, preset: &str, overrides: &HashMap<String, String>) -> String {
    if let Some(k) = overrides.get(action) {
        return k.clone();
    }
    if preset == "vscode"
        && let Some(e) = VSCODE.iter().find(|e| e.action == action)
    {
        return e.keys.join(" · ");
    }
    IDEA.iter()
        .find(|e| e.action == action)
        .map(|e| e.keys.join(" · "))
        .unwrap_or_default()
}
