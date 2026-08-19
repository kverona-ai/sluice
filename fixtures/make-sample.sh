#!/usr/bin/env bash
# Deterministic sample repository for graph / log smoke tests (05 §9.1 fixtures).
# Usage: fixtures/make-sample.sh [dir]   (default: target/fixtures/sample)
set -euo pipefail
DIR="${1:-$(cd "$(dirname "$0")/.." && pwd)/target/fixtures/sample}"
rm -rf "$DIR" "$(dirname "$DIR")/sample-origin.git"; mkdir -p "$DIR"; cd "$DIR"
export GIT_AUTHOR_NAME="will" GIT_AUTHOR_EMAIL="will@example.com"
export GIT_COMMITTER_NAME="will" GIT_COMMITTER_EMAIL="will@example.com"
T=1755600000
tick() { T=$((T+3600)); export GIT_AUTHOR_DATE="@$T +0800" GIT_COMMITTER_DATE="@$T +0800"; }
c() { tick; git commit -q --allow-empty -m "$1"; }
git init -q -b main
echo "# sample" > README.md; mkdir -p src; echo "fn main(){}" > src/main.rs; git add -A; c "chore: initial scaffold"
echo "pub fn lanes() {}" > src/lanes.rs; git add -A; c "feat(graph): lane assignment skeleton"
git checkout -q -b feat/graph-lanes
echo "pub fn route() {}" > src/route.rs; git add -A
tick; GIT_AUTHOR_NAME="Claude Code" GIT_AUTHOR_EMAIL="claude@anthropic.com" git commit -q -m "feat(graph): merge edge routing

Co-Authored-By: Claude <noreply@anthropic.com>"
echo "// stable colors" >> src/lanes.rs; git add -A
tick; GIT_AUTHOR_NAME="Codex CLI" GIT_AUTHOR_EMAIL="codex@openai.com" git commit -q -m "feat(graph): stable lane colors

Co-Authored-By: Codex <codex@openai.com>"
git checkout -q main
echo "pub fn watch() {}" > src/watch.rs; git add -A; c "feat(watch): notify-based watcher"
git checkout -q -b feat/mcp
echo "pub fn mcp() {}" > src/mcp.rs; git add -A
tick; GIT_AUTHOR_NAME="Grok Build" GIT_AUTHOR_EMAIL="grok@x.ai" git commit -q -m "feat(bridge): MCP server skeleton"
git checkout -q main
tick; git merge -q --no-ff feat/graph-lanes -m "merge: feat/graph-lanes into main"
echo "v0.1" > VERSION; git add -A; c "chore: bump to v0.1"
tick; git tag -a v0.1 -m "v0.1"
tick; git merge -q --no-ff feat/mcp -m "merge: feat/mcp into main"
git checkout -q -b fix/index-jitter
echo "// debounce" >> src/watch.rs; git add -A
tick; GIT_AUTHOR_NAME="DeepSeek Harness" GIT_AUTHOR_EMAIL="dsh@deepseek.com" git commit -q -m "fix(watch): debounce .git/index jitter"
git checkout -q main
echo "docs" > CHANGELOG.md; git add -A; c "docs: changelog"
# a fake remote so ahead/behind and origin/* badges exist
git clone -q --bare . ../sample-origin.git
git remote add origin ../sample-origin.git
git fetch -q origin
git branch -q --set-upstream-to=origin/main main
echo "// local only" >> README.md; git add -A; c "feat: local-only commit (ahead 1)"
echo "$DIR"
