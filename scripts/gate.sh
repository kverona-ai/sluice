#!/usr/bin/env bash
# Local CI gate — run before every commit. Mirrors .github/workflows/ci.yml.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
if command -v cargo-deny >/dev/null 2>&1; then cargo deny check licenses bans sources; fi
echo "GATE OK"
