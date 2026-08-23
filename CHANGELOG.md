# Changelog

All notable changes to Sluice are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow SemVer (0.x while pre-1.0).

## [Unreleased]

### Added
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
