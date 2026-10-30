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

# A document cannot fail a test, so it rots: the install row once downloaded
# 0.2.3 and installed 0.2.1, a 404 for anyone who copied it. Every version and
# every release file the README names is compared with what the release actually
# carries. Two accepted values, because a release PR names the next version
# before its tag exists: the newest tag, or the version in Cargo.toml.
# Set README_FILE to point the check at a fixture.
check_readme_versions() {
	local readme="${README_FILE:-README.md}"
	local pkg slug tag newest assets v name bad=0

	pkg="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
	[[ -f "$readme" ]] || fail "$readme not found"

	if ! slug="$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null)" || [[ -z "$slug" ]]; then
		echo "! skipped: README versions vs release assets (no GitHub access)"
		return 0
	fi
	if ! tag="$(gh release view --repo "$slug" --json tagName --jq .tagName 2>/dev/null)" || [[ -z "$tag" ]]; then
		echo "! skipped: README versions vs release assets (no release in $slug yet)"
		return 0
	fi
	newest="${tag#v}"
	assets="$(gh release view "$tag" --repo "$slug" --json assets --jq '.assets[].name' 2>/dev/null || true)"

	while read -r v; do
		[[ -n "$v" ]] || continue
		[[ "$v" == "$newest" || "$v" == "$pkg" ]] && continue
		echo "✗ $readme downloads v$v; the newest release is $tag and Cargo.toml says $pkg" >&2
		bad=1
	done < <(grep -oE '/releases/download/v[0-9]+\.[0-9]+\.[0-9]+' "$readme" | sed 's#.*/v##' | sort -u)

	while read -r name; do
		[[ -n "$name" ]] || continue
		if grep -qxF "$name" <<<"$assets"; then
			continue
		fi
		if [[ "$name" == *"$pkg"* ]]; then
			echo "! $name is not published yet (it ships with $pkg)"
			continue
		fi
		echo "✗ $readme names $name, which $tag does not publish" >&2
		bad=1
	done < <(grep -oE 'routwatch-[0-9][0-9A-Za-z._-]*\.pkg\.tar\.zst(\.sha256)?|routwatch-x86_64-linux-musl(\.sha256)?' "$readme" | sort -u)

	[[ "$bad" -eq 0 ]] || fail "$readme names a version or a file the release does not carry"
	echo "✓ README versions match $tag"
}

command -v gh >/dev/null || fail "gh is not installed (pacman -S github-cli)"
gh auth status >/dev/null 2>&1 || fail "gh is not logged in - run: gh auth login"

[[ -z "$(git status --porcelain)" ]] || fail "working tree is dirty; commit first"

check_readme_versions

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
