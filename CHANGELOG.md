# Changelog

All notable changes to Sluice are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow SemVer (0.x while pre-1.0).

## [Unreleased]

### Mobile (binding API, `crates/ffi` — SemVer 0.x)
- First UniFFI export surface (02 §5.5): `SluiceSession` (`open_local`, `pair`, `connect`,
  `disconnect`, `unpair`, `set_event_sink`), `RepoView` (`summary`, `log`, `commit`, `diff`),
  `ReviewQueue` (`items`, `refresh`, `approve`, `reject`), callback `EventSink` with
  `DomainEvent` (Connected / Disconnected / RepoChanged / Proposed / QueueChanged / Decided /
  Error). Async methods run on tokio (Swift `async`, Kotlin `suspend`); bindings via
  `cargo run -p sluice-ffi --features bindgen --bin uniffi-bindgen -- generate --library …`.
  CI smoke: open a repo, read its status, receive one event.

### Added
- **Phone companion channel — desktop side** (02 §5.4 / §5.7, 05 §7.1): new `sluice-sync`
  crate. One-time QR pairing (desktop static X25519 key + 10-minute code), Noise-style
  handshake with forward secrecy, ChaCha20-Poly1305 frames with replay protection, LAN
  listener first and an end-to-end encrypted relay fallback (`sluice relay serve`,
  self-hostable, forwards ciphertext only), trusted-device store with revocation
  (`⌘⇧D` panel: QR, channel status, devices, relay address, recent decisions). Devices see
  a read model (repo card, review queue with patch, commit-graph pages, commit detail,
  diffs); 放行 / 驳回 are Ed25519-signed by the device, carry the item's baseline version
  (stale → `expired`), are executed by the desktop and appended to
  `<common-dir>/sluice/audit.log` with the deciding device (also shown in Console).
  `sluice pair <payload>` / `sluice remote status|queue|approve|reject|log|diff|watch|unpair`
  act as the phone from any machine.
- **Proposal baselines** (05 §7.1): every queued proposal records a fingerprint of HEAD + the
  files it touches; if the repository moved, accepting it answers "expired" instead of
  executing — on the desktop and from devices alike.
- **jujutsu backend** (05 §8 capability mapping): `.jj` detected (colocated or standalone —
  the git store is opened through gix), working-copy changes via `jj diff --summary` /
  `jj file show`, no staging UI, commit = `jj commit` / `jj describe`, push via `jj git push`,
  change id next to the commit id, operation log in the time machine with `jj undo` /
  `jj op restore`, conflicts surfaced from `jj resolve --list`.
- **PR / MR review tab** (`⌘8`): open PRs from the logged-in `gh` / `glab` (GitHub / GitLab
  detected from `origin`), details with checks / decision / discussion, PR head fetched locally
  (`refs/pull/N/head` / `refs/merge-requests/N/head`) and diffed with Sluice's own viewer,
  checkout / approve / request changes / comment / squash-merge, AI pre-review drafted into the
  comment box (never auto-posted).
- Self-screenshot mode (`SLUICE_SCREENSHOT=<png>`, macOS; `_TAB` / `_DARK` / `_OPEN` knobs)
  used for the README images and future visual checks.
- **Syntax highlighting** in diffs via tree-sitter (Rust, JS/TS/TSX, Python, Go, JSON, TOML,
  Bash, C, C++, Java, YAML, HTML, CSS), light + dark palettes.
- **Dock-style layout**: resizable refs / center / details and tree / diff panes (widths
  persisted); Console can be docked at the bottom (⌥⌘6).
- **Keymap presets** (IDEA / VS Code) and `~/.sluice/keymap.json` per-action overrides;
  effective table in Settings; Escape also on ctrl-[.
- **Worktrees panel** (⌘⇧W): list / open / launch the detected AI CLI inside / add / remove.
- **Update check** (`sluice update`, startup toast, Settings) and **opt-in telemetry client**
  with local crash logs; release workflow signs + notarizes when secrets exist.
- **English UI** (`⌘⇧L` or Settings → Appearance): `tr()` / `tf!` translation layer with a
  `assets/i18n/en.json` table covering the whole desktop UI; language persists in settings.
- **Commit message history**: last 50 messages, ⟲ in the commit panel / `⌘⇧M`.
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
