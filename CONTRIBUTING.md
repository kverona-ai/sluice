# Contributing to Sluice

Thanks for considering a contribution! This guide covers everything the README's quick path skips.

## Development setup

```bash
git clone https://github.com/<you>/sluice.git
cd sluice
cargo test --workspace        # first build takes a while (gpui); the toolchain auto-installs
```

- **macOS 12+**: no Xcode required — gpui builds with `runtime_shaders`, the CommandLineTools are enough.
- **Windows 10/11**: Visual Studio Build Tools (C++ workload) + Git for Windows.
- **git ≥ 2.35** must be on `PATH` (the write path shells out to it).

Run the app against a throwaway repository:

```bash
fixtures/make-sample.sh                       # deterministic multi-branch repo with agent commits
cargo run -p sluice -- open target/fixtures/sample
cargo run -p sluice -- log  target/fixtures/sample   # read-path text dump, useful in tests
scripts/bundle-macos.sh debug                 # optional: a double-clickable dev Sluice.app
```

## Project layout

See the [Architecture](README.md#architecture) table. The rules that reviews enforce:

1. `crates/core` and `crates/domain` must never depend on gpui or any UI type.
2. Repository **reads** go through `gix` (`backend-gix`); **writes** go through the user's `git`
   (`backend-cli`). `git2`/`libgit2` are rejected by `cargo-deny`.
3. Everything the app writes to a repo must be reproducible from the Console output.
4. UI code follows the existing theme tokens (`crates/ui/src/theme.rs`) — no hard-coded colors.

## Before you push

```bash
cargo fmt --all
cargo clippy --workspace --all-targets      # warnings are errors in CI
cargo test --workspace
cargo deny check licenses bans sources      # cargo install cargo-deny (optional locally, CI runs it)
```

CI must be green on **macOS and Windows** — platform-specific behavior (CRLF, paths, PATH
resolution) is the most common cause of red Windows runs; prefer explicit configuration in tests
(e.g. `git config core.autocrlf false` in fixtures).

## Commits & PRs

- **Conventional Commits**: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:` (+ optional
  scope, e.g. `feat(graph): …`). English, imperative mood.
- One logical change per PR; include tests for behavior changes.
- Fill in the PR template; link the issue it addresses.
- For anything larger than a small fix, open an issue first so the approach can be agreed on —
  the milestone plan is strict about scope.

## Reporting bugs / requesting features

Use the issue templates. For bugs, `sluice log <repo>` output and the OS/git versions help a lot.
Security issues: see [SECURITY.md](SECURITY.md) — please do not open public issues for those.
