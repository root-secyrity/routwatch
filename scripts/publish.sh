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
	local pkg recipe slug tag newest assets v name installed bad=0

	pkg="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
	[[ -f "$readme" ]] || fail "$readme not found"

	# One number, four witnesses. The recipe, the README, the tag and the
	# installed package all have to name what Cargo.toml carries: a tree at
	# 0.4.1 that ships a README installing 0.2.5 is a false public claim, and
	# accepting "the coming release" as equal is how that passed for a day.
	recipe="$(grep -m1 '^pkgver=' packaging/PKGBUILD.release 2>/dev/null | cut -d= -f2)"
	if [[ "$recipe" != "$pkg" ]]; then
		echo "✗ packaging/PKGBUILD.release says pkgver=$recipe, Cargo.toml says $pkg" >&2
		bad=1
	fi

	while read -r v; do
		[[ -n "$v" ]] || continue
		if [[ "$v" != "$pkg" ]]; then
			echo "✗ $readme installs v$v; the tree carries $pkg" >&2
			bad=1
		fi
	done < <(grep -oE '/releases/download/v[0-9]+\.[0-9]+\.[0-9]+' "$readme" | sed 's#.*/v##' | sort -u)

	# A version nobody tagged cannot be reproduced, whatever the files say.
	if ! git rev-parse -q --verify "refs/tags/v$pkg" >/dev/null; then
		echo "✗ v$pkg is not tagged; cut the tag before publishing" >&2
		bad=1
	fi
	installed="$(pacman -Q routwatch 2>/dev/null | awk '{print $2}')"
	if [[ -n "$installed" && "${installed%%-*}" != "$pkg" ]]; then
		echo "✗ pacman has routwatch $installed; the tree carries $pkg" >&2
		bad=1
	fi
	[[ "$bad" -eq 0 ]] || fail "Cargo.toml, PKGBUILD.release, README, the tag and the installed package disagree"

	if ! slug="$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null)" || [[ -z "$slug" ]]; then
		echo "! skipped: release asset names (no GitHub access)"
		return 0
	fi
	if ! tag="$(gh release view --repo "$slug" --json tagName --jq .tagName 2>/dev/null)" || [[ -z "$tag" ]]; then
		echo "! skipped: release asset names (no release in $slug yet)"
		return 0
	fi
	newest="${tag#v}"
	if [[ "$newest" != "$pkg" ]]; then
		echo "✗ the newest release is $tag; the tree carries $pkg" >&2
		bad=1
	fi
	assets="$(gh release view "$tag" --repo "$slug" --json assets --jq '.assets[].name' 2>/dev/null || true)"

	while read -r name; do
		[[ -n "$name" ]] || continue
		if grep -qxF "$name" <<<"$assets"; then
			continue
		fi
		echo "✗ $readme names $name, which $tag does not publish" >&2
		bad=1
	done < <(grep -oE 'routwatch-[0-9][0-9A-Za-z._-]*\.pkg\.tar\.zst(\.sha256)?|routwatch-x86_64-linux-musl(\.sha256)?' "$readme" | sort -u)

	[[ "$bad" -eq 0 ]] || fail "$readme, the release assets and the tree disagree"
	echo "✓ version $pkg is one number in Cargo.toml, the recipe, the README, $tag and pacman"
}
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
