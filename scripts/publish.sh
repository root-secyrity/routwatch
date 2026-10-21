#!/usr/bin/env bash
# Publish routwatch to GitHub.
#
#   ./scripts/publish.sh public     # create the repo as public and push
#   ./scripts/publish.sh private    # same, private
#   ./scripts/publish.sh check      # only report what is missing
#
# Refuses to push a dirty tree or a failing build. `gh auth login` must have
# been run once; the script never asks for credentials itself.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

mode="${1:-check}"
case "$mode" in
public | private | check) ;;
*)
	echo "usage: $0 [public|private|check]" >&2
	exit 64
	;;
esac

fail() {
	echo "✗ $*" >&2
	exit 1
}

command -v gh >/dev/null || fail "gh is not installed (pacman -S github-cli)"
gh auth status >/dev/null 2>&1 || fail "gh is not logged in — run: gh auth login"

[[ -z "$(git status --porcelain)" ]] || fail "working tree is dirty; commit first"

if [[ "$mode" == check ]]; then
	echo "✓ gh authenticated as: $(gh api user --jq .login)"
	echo "✓ tree clean at $(git rev-parse --short HEAD) ($(git describe --tags --always))"
	remote="$(git remote get-url origin 2>/dev/null || echo "none yet")"
	echo "✓ origin: $remote"
	echo "→ ./scripts/publish.sh public|private"
	exit 0
fi

echo "→ cargo test --locked"
cargo test --locked >/dev/null
echo "→ cargo clippy --all-targets --locked"
cargo clippy --all-targets --locked -- -D warnings >/dev/null

if ! git remote get-url origin >/dev/null 2>&1; then
	gh repo create "$(basename "$repo_root")" "--$mode" --source=. --remote=origin
fi

git push -u origin HEAD
git push --tags
echo "✓ published: $(gh repo view --json url --jq .url)"
