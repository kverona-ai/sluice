<div align="center">

# Sluice

**The IDEA-grade Git workbench for AI coding agents.**

AI agents write the code. You review, stage line by line, and open the gate.

[![CI](https://github.com/kverona-ai/sluice/actions/workflows/ci.yml/badge.svg)](https://github.com/kverona-ai/sluice/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache--2.0-3b5bdb.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.97-dea584?logo=rust&logoColor=white)](rust-toolchain.toml)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows-2f9e44)](#installation)
[![Status](https://img.shields.io/badge/status-M3%20%2F%20M4%20in%20progress-e8590c)](#roadmap)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-d6006c.svg)](#contributing)

**English** · [简体中文](./README.zh-CN.md)

</div>

---

## What is Sluice?

Terminal AI coding agents — Claude Code, Codex CLI, Grok Build, DeepSeek Harness, Gemini CLI,
Kimi Code and friends — now produce most of the commits in a modern repository, at a pace no
human can follow with `git log` and `git add -p`.

**Sluice is the sluice gate between those agents and your history.** It is a standalone,
GPU-rendered Git workbench that gives you the review workflow of JetBrains IDEA's Git tooling,
rebuilt for the agentic era:

- **See everything** — a virtualized commit graph with stable lane colors, live filters, and an
  *agent badge* on every commit telling you whether a human, Claude, Codex, Grok or another agent
  wrote it.
- **Approve precisely** — side-by-side diffs with word-level highlights; stage whole files, single
  hunks, or single lines with checkboxes (`git add -p`, but visual).
- **Stay in control** — every write goes through your own `git` binary, so hooks, commit signing
  and credential helpers behave exactly like your terminal; every command Sluice runs is echoed to
  a Console tab.
- **Bring your own AI** — commit messages drafted by whatever AI CLI you already use, on your own
  account. No API keys, no configuration.
- **Never blocked, never surprised** — a file watcher reflects any agent/terminal activity in
  ~200 ms, and agents will only ever *propose*; a human clicks the gate open (propose-and-confirm
  queue, in development).

> **Status: early preview.** Reading history, filtering, diffing, staging (file / hunk / line),
> committing, pull / push and AI commit drafts work today. Branch operations, merge / rebase UI,
> the MCP bridge and mobile companions are in active development — see the [roadmap](#roadmap).

## Features

| | |
|---|---|
| **Commit graph** | 10k+ commits with smooth scrolling, stable lane colors, `origin & main` combined badges, HEAD highlight, date- or topo-order |
| **Filters** | text / regex / case-sensitive search, author picker, date presets, "AI commits only" toggle — all live |
| **Diff viewer** | side-by-side & unified, word-level highlights, ignore-whitespace, context 0/3/8/20, hunk navigation (`F7`/`⇧F7`) |
| **Line-level staging** | checkboxes on every hunk and line; the exact patch is applied with `git apply --cached` and echoed to the Console |
| **Commit panel** | Amend / Sign-off / Author override / skip-hooks, Commit & Push, undo-friendly toasts |
| **AI commit messages** | drafted by your installed CLI — Claude Code, Codex CLI, Grok Build, DeepSeek Harness, Gemini CLI, Kimi Code, Z Code, Qwen Code, opencode, GitHub Copilot CLI — auto-detected, zero keys |
| **One-click AI hookup** | `⌘⇧I` or `sluice ai connect`: registers Sluice's read-only MCP server with every installed AI CLI (uses each tool's own `mcp add`, or a backed-up config edit where no command exists) |
| **Propose-and-confirm queue** | agents call `propose_commit` / `propose_branch` / `propose_push` over MCP; the call blocks until you accept or reject in Sluice (`⌘⇧P`), and only an accepted proposal runs — via your own git |
| **Session provenance** | `sluice hook <tool>` events (installed with one click) link commits to the AI sessions that touched the same files, even when no trailer was written |
| **File history & blame** | `git log --follow` per file with jump-to-commit; blame with commit-banded gutter, hover to highlight a commit (`⌘⇧H` / `⌥⌘B`, or right-click any file) |
| **Interactive rebase & conflicts** | plan-driven `git rebase -i` (pick/reword/squash/fixup/drop, reorder) and a three-way resolver for conflicted files (take ours/theirs/both per block, save & mark resolved) |
| **Branches / stash / time machine** | branches panel with checkout · merge · rebase · delete · new; stash push/apply/pop/drop; automatic safety snapshots before discard / reset --hard |
| **Agent provenance** | commits are attributed to the agent that made them (trailer analysis today, deterministic session tracking on the roadmap) |
| **Live refresh** | worktree + `.git` watcher; anything an agent does shows up in ~200 ms |
| **Console** | every git command with duration, exit code and stderr; verbose mode reveals the read path too |
| **Keyboard-first** | IDEA-style keymap: `⌘9` Log · `⌘0` Changes · `⌘B` branches · `⌘5` stash · `⌘K` commit panel · `⌘↩` commit · `Space` stage · full list in Settings (`⌘,`) |

## Tech stack

| Layer | Technology | Why |
|---|---|---|
| Language | [Rust](https://www.rust-lang.org/) (pinned by `rust-toolchain.toml`) | one language from git internals to pixels; mobile-ready core |
| UI framework | [GPUI](https://www.gpui.rs/) + [gpui-component](https://github.com/longbridge/gpui-component) | GPU-rendered UI (the framework behind Zed); virtualized lists, docks, inputs |
| Git read path | [gitoxide (`gix`)](https://github.com/GitoxideLabs/gitoxide) | fast, pure-Rust repository reading — refs, log, status, blobs |
| Git write path | your own `git` CLI | 100 % behavioral fidelity: hooks, GPG/SSH signing, credential helpers |
| Diffing | [imara-diff](https://github.com/pascalkuthe/imara-diff) + custom word-level pass | Histogram line diff, token-level highlights, partial-patch generation |
| File watching | [notify](https://github.com/notify-rs/notify) | cross-platform worktree + `.git` events |
| Async | [tokio](https://tokio.rs/) + GPUI executors | git work never blocks a frame |
| Typography / icons | Source Serif 4 (OFL) · Phosphor Icons (MIT) | embedded, no network at runtime |

## Architecture

```
crates/
├── core          VCS-neutral domain types · GitReader trait · backend capability flags
│                 diff engine (hunks, word diff, partial patches) · Console log
├── backend-gix   READ  — refs / log / commit details / status / blobs via gitoxide
├── backend-cli   WRITE — stage, commit, push, pull … through the user's own `git`
│                 login-shell PATH resolution · porcelain-v2 status parser
├── graph         commit-graph lane layout: lane assignment, edge routing, stable colors
├── watch         worktree + .git watcher with debounce-friendly event stream
├── domain        Repo handle · background snapshots (log / detail / changes / diffs)
│                 log filtering — the UI-agnostic brain, no gpui types allowed
├── bridge        MCP server · agent hooks · session provenance        (in development)
├── ui            GPUI views: workbench chrome, log, diff viewer, changes, console
└── app           the `sluice` binary — desktop entry point + CLI subcommands
```

Two rules keep it honest: `core`/`domain` never see a UI type, and `git2` is banned by
`cargo-deny` — reads are pure Rust (`gix`), writes are your own `git`.

## Installation

### Prerequisites

- **git ≥ 2.35** on your `PATH`
- **Rust** — [rustup](https://rustup.rs/) installs the pinned toolchain automatically on first build
- **macOS 12+** (Apple Silicon or Intel; no Xcode / Metal toolchain required), or
  **Windows 10/11** with the Visual Studio Build Tools (C++ workload) and Git for Windows

### Run from source

```bash
git clone https://github.com/kverona-ai/sluice.git
cd sluice
cargo run --release -p sluice -- open /path/to/any/repo
```

### Install / deploy

```bash
# macOS — build a double-clickable app bundle
cargo build --release -p sluice
scripts/bundle-macos.sh release          # → target/release/Sluice.app, drag into /Applications

# Windows — build the executable
cargo build --release -p sluice          # → target\release\sluice.exe

# Or install the `sluice` CLI onto your PATH (both platforms)
cargo install --path crates/app
sluice open .                            # open the current repository
sluice log .                             # text dump of the graph (handy over SSH)
sluice ai status                         # which AI CLIs are installed / hooked up
sluice ai connect                        # register Sluice as MCP server in all of them
sluice mcp serve                         # the stdio MCP server itself (what the tools launch)
```

Tagged versions (`v*`) are built by the [Release workflow](.github/workflows/release.yml) for
macOS arm64 / x86_64 and Windows x86_64 and attached to GitHub Releases with `SHA256SUMS` — unsigned
previews for now (Gatekeeper / SmartScreen will warn); signing and notarization arrive with the
first public beta, together with Homebrew / winget packages. Try it instantly on a generated demo
repository:

```bash
fixtures/make-sample.sh && cargo run --release -p sluice -- open target/fixtures/sample
```

### Keyboard shortcuts

IDEA-style preset (`Ctrl` instead of `⌘` on Windows/Linux): `↑/↓/PgUp/PgDn/Home/End` navigate ·
`Space` stage/unstage · `⌘9` Log · `⌘0` Local Changes · `⌘6` Console · `⌘F` search ·
`⌘K` commit panel · `⌘↩` commit · `⌥⌘A`/`⌥⌘U` stage/unstage all · `F7`/`⇧F7` hunks ·
`Esc` close diff · `⌥⌘Y` refresh.

## Roadmap

- ✅ **M1 — Review**: commit graph, refs, filters, diffs, details, live watcher
- ✅ **M2 — Commit**: staging down to lines, commit panel, AI drafts, push/pull
- ✅ **M3 — Branches**: branches panel, merge / rebase, interactive rebase, stash, snapshots,
  three-way conflict resolver, file history & blame, multi-repo
- 🔨 **M4 — AI bridge** *(mostly done)*: MCP server (read tools + propose_* tools), one-click
  hookup incl. hooks, propose-and-confirm queue, session provenance, askpass; setup wizard polish
  and public beta packaging next
- ⏭ **M5 — Mobile companions**: review & approve from iOS / Android over the same Rust core
- ⏭ **M6 — Extensions**: GitHub / GitLab PR review, jujutsu (jj) backend

## Contributing

PRs are very welcome — the quick path:

```bash
# 1. Fork on GitHub, then
git clone https://github.com/<you>/sluice.git && cd sluice
# 2. Verify your environment (toolchain auto-installs)
cargo test --workspace
# 3. Branch, hack, and keep the checks green
git switch -c feat/my-change
cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace
# 4. Commit (Conventional Commits: feat:/fix:/docs:/refactor: …) and open a PR against main
```

CI runs fmt, clippy `-D warnings`, tests and `cargo-deny` on macOS **and** Windows — a green run is
all a PR needs. See [CONTRIBUTING.md](CONTRIBUTING.md) for the full guide (project layout, fixture
repos, how to run the app against test data), and open an
[issue](https://github.com/kverona-ai/sluice/issues) for anything bigger than a small fix first.

## Star history

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=kverona-ai/sluice&type=Date&theme=dark" />
  <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=kverona-ai/sluice&type=Date" />
  <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=kverona-ai/sluice&type=Date" />
</picture>

## License

[Apache-2.0](LICENSE) © kverona-ai and contributors.
Bundled assets: Source Serif 4 (SIL OFL 1.1) · Phosphor Icons (MIT).
