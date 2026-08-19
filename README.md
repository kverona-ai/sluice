# Sluice

**The IDEA-grade Git workbench for AI coding agents.** Claude Code, Codex CLI, Grok Build and
DeepSeek Harness write code in the terminal; Sluice is the sluice gate where a human reviews, stages,
commits and releases what they produced — commit graph, line-level staging, diff review, and a
propose-and-confirm queue for agents.

> Status: **M0 → M1 in progress.** The desktop app opens any local repository and renders the
> read-only Log workspace (commit graph, refs, details). Nothing writes to a repository yet.

Requirements, architecture and roadmap live in the companion repository
[kverona-ai/sluice-doc](https://github.com/kverona-ai/sluice-doc) (Chinese; v0.3 is the development
baseline). The visual design is a Claude Design prototype (`SluiceDesktop.dc.html`, Broadsheet
design system) kept under `sluice-doc/requirements/assets/prototype/`.

## Build & run

Rust is pinned by `rust-toolchain.toml` (rustup installs it on first use).

```bash
cargo run -p sluice -- open /path/to/repo      # desktop app (macOS / Windows; Linux follows)
cargo run -p sluice -- log /path/to/repo        # text dump of the gix read path + lane layout
scripts/bundle-macos.sh debug                   # wrap the binary in target/debug/Sluice.app
fixtures/make-sample.sh                         # deterministic multi-branch sample repo
```

macOS builds do **not** need the Xcode Metal toolchain: gpui is built with `runtime_shaders`.
Windows needs the Visual Studio Build Tools (C++ workload) and Git for Windows.

Keyboard (IDEA preset, see sluice-doc 05 §11): `↑ ↓ PgUp PgDn Home End` move the selection,
`⌘9` Log, `⌘0` Local Changes, `⌘6` Console, `⌥⌘Y` refresh, `⌘Q` quit.

## Layout

| Crate | Layer (sluice-doc 02 §1) |
|---|---|
| `crates/core` | VCS-neutral types, `GitReader` trait, backend capabilities |
| `crates/backend-gix` | read path: refs / log / details / changes via gitoxide |
| `crates/backend-cli` | write path: git CLI runner with console echo (M2) |
| `crates/domain` | `RepoStore` — UI-agnostic state, commands and events |
| `crates/graph` | commit-graph lane layout, stable colors |
| `crates/watch` | worktree + `.git` watcher (M1) |
| `crates/bridge` | MCP server, AI tool hooks, provenance (M4) |
| `crates/ui` | GPUI views — the only crate allowed to depend on gpui |
| `crates/app` | the `sluice` binary: desktop entry point and CLI subcommands |

Conventions: code, comments, commits, issues in English; Conventional Commits; `cargo fmt`,
`clippy -D warnings`, `cargo-deny` in CI for macOS + Windows.

## License

Apache-2.0 — see [LICENSE](LICENSE). Bundled assets: Source Serif 4 (SIL OFL 1.1),
Phosphor Icons (MIT); their license files sit next to the assets.
