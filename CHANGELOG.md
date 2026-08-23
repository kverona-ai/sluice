# Changelog

All notable changes to Sluice are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow SemVer (0.x while pre-1.0).

## [Unreleased]

### Added
- **Interactive rebase planner** (commit context menu / ⌥⌘R): pick · reword · squash · fixup ·
  drop, reorder (⌥↑/⌥↓), reword editor; runs `git rebase -i` through the plan-driven
  `sluice seq-editor` / `sluice editor`; safety snapshot first; conflicts → in-progress banner.
- **Three-way conflict resolver**: conflicted files open in a marker-aware view (ours / theirs /
  base), per-block take ours / theirs / both (⌥1/⌥2/⌥3), whole-file shortcuts, save, save & mark
  resolved (⌘S → `git add`).
- **Multi-repo**: recent repositories panel (⌘⇧O), native folder picker (⌘O), in-window switch.
- **Release workflow**: macOS arm64 / x86_64 + Windows x86_64 archives with SHA256SUMS on `v*`
  tags (unsigned previews), dry-run via workflow_dispatch.
- **Propose-and-confirm queue**: loopback IPC between the desktop app and `sluice mcp serve`;
  MCP tools `propose_commit` / `propose_branch` / `propose_push` block until a human decides in
  the queue (`⌘⇧P`, rail badge); accepted proposals run through the user's git and the result is
  returned to the agent; rejections and timeouts are reported.
- **Session provenance**: `sluice hook <tool>` normalizes hook payloads (Claude Code / Codex /
  Gemini / Qwen / Kimi / Copilot / Grok) into `~/.sluice/provenance/*.jsonl`; commit details show
  the AI sessions that touched the commit's files in the 12 h before it. Hooks are installed by
  the AI hookup (per-tool merge rules, idempotent).
- `sluice askpass` (GIT_ASKPASS / SSH_ASKPASS through the desktop), `sluice diagnostics`.
- **One-click AI hookup** (`⌘⇧I`, `sluice ai status|connect|disconnect`): detects installed
  AI CLIs and registers the Sluice MCP server — via each tool's own `mcp add` (Claude Code,
  Codex, Gemini, Qwen, Copilot, Grok Build) or a backed-up partial config edit (opencode,
  Kimi Code, Z Code). Files the tools rewrite themselves (`~/.claude.json`, Grok's
  `config.toml`) are never edited directly.
- **File history & blame** views with jump-to-commit (`⌘⇧H` / `⌥⌘B`, context menus).
- **Branches panel** (`⌘B` / `⌃⇧\``): searchable local + remote list, click-to-checkout,
  per-branch merge / rebase actions, delete with confirmation, new-branch dialog (also from any
  commit's context menu).
- **Right-click context menus**: commits (copy hash, new branch from, checkout detached,
  cherry-pick, revert, reset soft/mixed/hard, undo last unpushed commit) and working-tree files
  (stage/unstage, discard with safety snapshot, copy path).
- **Stash panel** (`⌘5`): push (message + include-untracked), apply / pop / drop.
- **Time machine v1** (`⌘7`): automatic safety snapshots (`refs/sluice/snapshots/*`) before
  discard / reset --hard, manual snapshots, restore & delete.
- **Push dialog** (`⌘⇧K`): ahead count, `--force-with-lease`, `-u` upstream.
- **Settings** (`⌘,`): keymap reference, telemetry opt-in (default off), about.
- **In-progress operation banner**: merge / rebase / cherry-pick / revert with
  continue · skip · abort.
- **Read-only MCP server**: `sluice mcp serve --repo <path>` (stdio, JSON-RPC) with
  `repo_status`, `list_changes`, `get_diff`, `log_query` — the first slice of the AI bridge.
- **Expandable tool rail**: labels next to icons; every chrome control has a hover tooltip.
- Windows: self-drawn caption buttons wired through `window_control_area` (gpui 0.2.2 has no
  native-caption path), mnemonic menu row.

### Fixed
- macOS traffic lights were half-clipped: the position must sit inside the native ~28px
  titlebar strip (gpui moves the real NSButtons; they are clipped by that superview).
- AI commit drafts now support ten CLIs with per-tool headless invocation (stdin, prompt-arg +
  stdin, or single-arg modes): Claude Code, Codex CLI, Grok Build, DeepSeek Harness, Gemini CLI,
  Kimi Code, Qwen Code, opencode, GitHub Copilot CLI, Z Code (community wrapper). Aider is
  intentionally excluded (side-effectful by default). Agent badges/provenance recognize the new
  tools.
- Bilingual README (`README.md` English / `README.zh-CN.md` 中文), CONTRIBUTING, SECURITY,
  Code of Conduct, issue/PR templates, star-history chart.

### Fixed
- Windows CI: the read-path integration test now pins `core.autocrlf=false` in its fixture
  (the runner default materialized merged files with CRLF).

- M1 / M2: file watcher with debounced refresh; live search (text / regex / case), author & date
  filters, AI-only, sort toggle; per-file diffs (side-by-side / unified, word-level highlights,
  whitespace, context, hunk navigation); Local Changes tree with file / hunk / line staging,
  commit panel (Amend / Sign-off / Author / no-verify), Commit & Push, pull / push; AI commit
  message drafts via the installed CLI; Console tab; selection history; sidebar toggle; tooltips,
  hover / focus states, bold / fill icons for small sizes.
- `sluice-core::diff` engine (hunks, word diff, side-by-side pairing, partial patch) and
  `Console`; `GitReader::blob`; working-tree status types.
- `sluice-backend-cli`: git runner with login-shell PATH resolution and porcelain-v2 status parser.
- `sluice-watch`: notify-based watcher.
- Cargo workspace laid out per sluice-doc 02 §1: `core`, `backend-gix`, `backend-cli`, `domain`, `graph`,
  `watch`, `bridge`, `ui`, `app` (the `sluice` binary).
- `sluice-core`: VCS-neutral types, `GitReader` trait, backend `Capabilities`, agent detection heuristics.
- `sluice-backend-gix`: refs, `--date-order` / `--topo-order` log, commit details (trailers, signature
  presence), per-commit change lists with line stats (imara-diff), upstream ahead/behind.
- `sluice-graph`: lane layout with stable hashed colors and per-row edge segments.
- `sluice-domain`: `RepoStore` (refs + log + graph, lazy commit details, ref filtering).
- `sluice-ui` + `sluice` app: GPUI Log workspace following the Claude Design prototype — custom macOS
  title bar with Local Changes / Log / Console segmented control, tool rail, refs tree, filter bar,
  virtualized commit list with the lane graph, status bar, commit details panel. Keyboard navigation
  (↑/↓/PgUp/PgDn/Home/End, ⌘9/⌘0/⌘6, ⌥⌘Y refresh).
- `sluice log` text dump, `sluice` CLI skeleton for `open` / `mcp serve` / `hook` / `askpass` /
  `editor` / `seq-editor` / `diagnostics` (later milestones report their target).
- Embedded Source Serif 4 (OFL) and Phosphor duotone icons (MIT).
- `scripts/bundle-macos.sh` dev .app bundle, `fixtures/make-sample.sh` deterministic sample repository.
