<div align="center">

# Sluice

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-3b5bdb.svg)](LICENSE)
[![CI](https://github.com/kverona-ai/sluice/actions/workflows/ci.yml/badge.svg)](https://github.com/kverona-ai/sluice/actions/workflows/ci.yml)
[![status](https://img.shields.io/badge/status-M1%E2%86%92M2%20in%20progress-e8590c)](#roadmap)
[![Rust](https://img.shields.io/badge/Rust-1.97-dea584?logo=rust&logoColor=white)](rust-toolchain.toml)
[![GPUI](https://img.shields.io/badge/UI-GPUI%200.2%20%2B%20gpui--component-0e7490)](https://www.gpui.rs/)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows-2f9e44?logo=apple&logoColor=white)](#build--run)
[![Docs](https://img.shields.io/badge/docs-sluice--doc%20v0.3-d6006c)](https://github.com/kverona-ai/sluice-doc)
[![AI CLIs](https://img.shields.io/badge/AI%20CLIs-Claude%20Code%20%C2%B7%20Codex%20%C2%B7%20Grok%20Build%20%C2%B7%20dsh-6d28d9)](#ai-integration)

**The IDEA-grade Git workbench for AI coding agents.**
<br>
[English](#english) · [简体中文](#简体中文)

</div>

---

## English

Claude Code, Codex CLI, Grok Build and DeepSeek Harness write code in the terminal at super-human
speed. **Sluice is the sluice gate in front of your repository**: a standalone, GPU-rendered Git
workbench where a human reviews, stages (down to single lines), commits and releases what the
agents produced — commit graph, line-level staging, diff review, agent provenance, and (M4) a
propose-and-confirm queue that lets agents *propose* but never *push*.

> **Status** — M1 (read-only review) is functionally complete and M2 (commit workflow) is
> usable: open any local repository, browse the commit graph with live filters, read diffs,
> stage files or individual lines, commit (with AI-drafted messages from your own logged-in CLI),
> pull / push. Branch operations, merge / rebase, the MCP bridge and the mobile shells are on the
> [roadmap](#roadmap). Expect rough edges.

Requirements, architecture and roadmap live in [kverona-ai/sluice-doc](https://github.com/kverona-ai/sluice-doc)
(Chinese; v0.3 is the development baseline). The visual design is a Claude Design prototype
(Broadsheet design system) archived under `sluice-doc/requirements/assets/prototype/`.

### What works today

| Area | Implemented |
|---|---|
| Log | commit graph with stable lane colors, refs tree (click = filter), text / regex / case search, author & date filters, AI-only toggle, date- or topo-order, commit details, per-file diffs, copy hash, ◀ ▶ selection history |
| Diff | side-by-side / unified, word-level highlights, whitespace toggle, context 0/3/8/20, hunk navigation (F7 / ⇧F7) |
| Local Changes | staged / unstaged / untracked / conflicts tree, stage / unstage per file or group, **hunk- and line-level staging** (`git apply --cached`), commit panel (Amend / Sign-off / Author / skip hooks), Commit & Push, pull / push |
| AI | commit message drafts through your installed CLI — Claude Code, Codex CLI, Grok Build or dsh, auto-detected, zero API keys; agent badges from trailers |
| Live | file watcher refreshes the graph and the change tree within ~200 ms of any agent or terminal activity |
| Console | every git command Sluice ran, with duration, exit code and stderr; verbose mode shows the gix read path |
| CLI | `sluice open <repo>`, `sluice log <repo>`; `mcp serve` / `hook` / `askpass` / `editor` / `seq-editor` / `diagnostics` reserved for later milestones |

### Build & run

Rust is pinned by [`rust-toolchain.toml`](rust-toolchain.toml) (rustup installs it on first use).
macOS builds do **not** need the Xcode Metal toolchain (gpui is built with `runtime_shaders`);
Windows needs the Visual Studio Build Tools (C++ workload) and Git for Windows. git ≥ 2.35 on PATH.

```bash
cargo run -p sluice -- open /path/to/repo      # desktop app
cargo run -p sluice -- log /path/to/repo        # text dump of the read path + lane layout
scripts/bundle-macos.sh debug                   # wrap the binary in target/debug/Sluice.app
fixtures/make-sample.sh                         # deterministic multi-branch sample repository
```

Keyboard (IDEA preset, sluice-doc 05 §11): `↑ ↓ PgUp PgDn Home End` navigate · `Space` toggle staged ·
`⌘9` Log · `⌘0` Local Changes · `⌘6` Console · `⌘F` search · `⌘K` commit panel · `⌘↩` commit ·
`⌥⌘A` / `⌥⌘U` stage / unstage all · `F7` / `⇧F7` next / previous hunk · `Esc` close diff · `⌥⌘Y` refresh · `⌘Q` quit.
(`Ctrl` instead of `⌘` on Windows / Linux.)

### Architecture

| Crate | Layer (sluice-doc 02 §1) |
|---|---|
| `crates/core` | VCS-neutral types, `GitReader` trait, backend capabilities, diff engine, Console |
| `crates/backend-gix` | read path: refs / log / details / changes / blobs via gitoxide (pure Rust, mobile-ready) |
| `crates/backend-cli` | write path: the user's `git` (hooks, signing, credential helpers), porcelain-v2 status |
| `crates/watch` | worktree + `.git` watcher (`notify`) |
| `crates/domain` | `Repo` handle, background snapshots (log / detail / changes / diffs), `LogFilter` |
| `crates/graph` | commit-graph lane layout, stable colors |
| `crates/bridge` | MCP server, AI tool hooks, provenance (M4) |
| `crates/ui` | GPUI views — the only crate allowed to depend on gpui |
| `crates/app` | the `sluice` binary: desktop entry point and CLI subcommands |

Design rules: `core` / `domain` never see a UI type; every read goes through gix, every write
through the user's `git`; `git2` is banned by `cargo-deny`.

### AI integration

Sluice reuses the AI CLI you already have and are already logged into — it never asks for an API
key. Today that means commit-message drafts (the staged diff is sent to the tool on your own
account; a first-use notice is shown). M4 adds the built-in MCP server, one-click hook wiring,
session provenance and the propose-and-confirm queue (see sluice-doc 03).

### Roadmap

M0 spikes ✔ → **M1 read-only review ✔ → M2 commit workflow (in progress)** → M3 branches, merge /
rebase, stash, three-way merge, time machine → M4 AI bridge (MCP, hooks, provenance, queue) and
public beta → M5 mobile (iOS UIKit / Android Compose shells over the same Rust core) → M6 PR review
integration + jujutsu backend. Details: sluice-doc 04.

### Contributing

Code, comments, commits and issues are in English; Conventional Commits; `cargo fmt`,
`clippy -D warnings`, `cargo test`, `cargo deny` run in CI for macOS + Windows.

### License

Apache-2.0 — see [LICENSE](LICENSE). Bundled assets: Source Serif 4 (SIL OFL 1.1), Phosphor Icons (MIT).

---

## 简体中文

Claude Code、Codex CLI、Grok Build、DeepSeek Harness 正以远超人类的速度在终端里改代码。**Sluice（水闸）是仓库前面的那道闸门**：一个独立的、GPU 渲染的 Git 工作台，人在这里审查、暂存（精确到单行）、提交并放行 AI 产出的代码——提交图、行级暂存、Diff 审查、agent 溯源，以及（M4）让 agent 只能「提议」、不能「推送」的提议-确认队列。

> **当前状态** —— M1（只读审查）功能已完整，M2（提交工作流）可用：打开任意本地仓库，看带实时过滤的提交图、读 diff、按文件或按行暂存、提交（可由你已登录的 AI CLI 起草提交信息）、拉取 / 推送。分支操作、merge / rebase、MCP 桥接与移动端见[路线图](#路线图)。仍是早期版本。

需求、架构与路线图在 [kverona-ai/sluice-doc](https://github.com/kverona-ai/sluice-doc)（中文，v0.3 为开发基线）。视觉设计来自 Claude Design 原型（Broadsheet 设计系统），存档于 `sluice-doc/requirements/assets/prototype/`。

### 现在能做什么

| 模块 | 已实现 |
|---|---|
| 日志 | 泳道颜色稳定的提交图、refs 树（点击即过滤）、文本 / 正则 / 大小写搜索、作者与日期过滤、「仅看 AI 提交」、时间序 / 拓扑序、提交详情、逐文件 diff、复制 hash、◀ ▶ 选中历史 |
| Diff | 双栏 / 统一、词级高亮、忽略空白、上下文 0/3/8/20 行、差异导航（F7 / ⇧F7） |
| 本地变更 | 已暂存 / 未暂存 / 未跟踪 / 冲突 分组树，按文件或按组暂存 / 取消暂存，**块级与行级暂存**（`git apply --cached`），提交面板（Amend / Sign-off / Author / 跳过 hooks），Commit & Push，拉取 / 推送 |
| AI | 用你本机已安装、已登录的 CLI 起草提交信息——Claude Code / Codex CLI / Grok Build / dsh 自动探测，零 API key；依据 trailer 显示 agent 徽章 |
| 实时 | 文件监听：agent 或终端的任何改动约 200ms 内反映到提交图与变更树 |
| Console | Sluice 执行过的每条 git 命令（耗时、退出码、stderr）；详细模式显示 gix 读路径的等价命令 |
| CLI | `sluice open <repo>`、`sluice log <repo>`；`mcp serve` / `hook` / `askpass` / `editor` / `seq-editor` / `diagnostics` 为后续里程碑预留 |

### 构建与运行

Rust 版本由 [`rust-toolchain.toml`](rust-toolchain.toml) 锁定（rustup 首次使用时自动安装）。macOS 构建**不需要** Xcode 的 Metal 工具链（gpui 启用 `runtime_shaders`）；Windows 需要 Visual Studio Build Tools（C++ 工作负载）与 Git for Windows。PATH 上需有 git ≥ 2.35。

```bash
cargo run -p sluice -- open /path/to/repo      # 桌面应用
cargo run -p sluice -- log /path/to/repo        # 读路径 + 泳道布局的文本导出
scripts/bundle-macos.sh debug                   # 打成 target/debug/Sluice.app
fixtures/make-sample.sh                         # 生成多分支样例仓库
```

快捷键（IDEA 预设，见 sluice-doc 05 §11）：`↑ ↓ PgUp PgDn Home End` 移动 · `Space` 切换暂存 · `⌘9` 日志 · `⌘0` 本地变更 · `⌘6` Console · `⌘F` 搜索 · `⌘K` 提交面板 · `⌘↩` 提交 · `⌥⌘A` / `⌥⌘U` 全部暂存 / 取消 · `F7` / `⇧F7` 下一处 / 上一处差异 · `Esc` 关闭 diff · `⌥⌘Y` 刷新 · `⌘Q` 退出（Windows / Linux 用 `Ctrl` 代替 `⌘`）。

### 架构

| Crate | 层次（sluice-doc 02 §1） |
|---|---|
| `crates/core` | VCS 中立类型、`GitReader` trait、后端能力声明、diff 引擎、Console |
| `crates/backend-gix` | 读路径：refs / 日志 / 详情 / 变更 / blob（gitoxide，纯 Rust，可交叉编译到移动端） |
| `crates/backend-cli` | 写路径：用户自己的 `git`（hooks、签名、凭据助手），porcelain v2 状态 |
| `crates/watch` | 工作区 + `.git` 监听（`notify`） |
| `crates/domain` | `Repo` 句柄、后台快照（日志 / 详情 / 变更 / diff）、`LogFilter` |
| `crates/graph` | 提交图泳道布局、稳定配色 |
| `crates/bridge` | MCP server、AI 工具 hooks、溯源（M4） |
| `crates/ui` | GPUI 视图——唯一允许依赖 gpui 的 crate |
| `crates/app` | `sluice` 可执行文件：桌面入口与 CLI 子命令 |

设计纪律：`core` / `domain` 不出现任何 UI 类型；所有读走 gix、所有写走用户的 `git`；`git2` 被 `cargo-deny` 禁止。

### AI 集成

Sluice 复用你已安装、已登录的 AI CLI，从不索取 API key。现在提供提交信息起草（staged diff 以你自己的账号发送给该工具，首次使用会提示）；M4 增加内置 MCP server、一键 hooks 接入、会话溯源与提议-确认队列（见 sluice-doc 03）。

### 路线图

M0 技术验证 ✔ → **M1 只读审查 ✔ → M2 提交工作流（进行中）** → M3 分支、merge / rebase、stash、三方合并、时光机 → M4 AI 桥接（MCP、hooks、溯源、队列）与 public beta → M5 移动端（iOS UIKit / Android Compose 壳复用同一 Rust 核心）→ M6 PR 评审集成 + jujutsu 后端。详见 sluice-doc 04。

### 参与

代码、注释、提交信息、issue 使用英文；Conventional Commits；CI 在 macOS + Windows 上运行 `cargo fmt`、`clippy -D warnings`、`cargo test`、`cargo deny`。

### 许可

Apache-2.0，见 [LICENSE](LICENSE)。内嵌资源：Source Serif 4（SIL OFL 1.1）、Phosphor Icons（MIT）。
