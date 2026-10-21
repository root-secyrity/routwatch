#!/usr/bin/env bash
# Build a portable static binary, in a container, for any Linux distribution.
#
#   ./packaging/musl-build.sh            # x86_64, Alpine/musl, static-pie
#   ./packaging/musl-build.sh aarch64    # needs qemu-user-static on the host
#
# Why: a binary linked against a distribution's glibc only runs on that glibc
# or newer. The musl build links nothing at all, so the same file runs on Arch,
# Debian, Ubuntu, Fedora, Alpine, Raspberry Pi OS — everywhere.
#
# Output: dist/routwatch-<arch>-linux-musl
set -euo pipefail

arch="${1:-x86_64}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="alpine:3.20"
platform=""

case "$arch" in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64)
	arch=aarch64
	platform="--platform linux/arm64"
	;;
*)
	echo "usage: $0 [x86_64|aarch64]" >&2
	exit 64
	;;
esac

out="$repo_root/dist"
mkdir -p "$out"

docker run --rm $platform \
	-v "$repo_root:/src:ro" \
	-v "$out:/out" \
	-w /src \
	-e CARGO_TARGET_DIR=/tmp/target \
	"$image" sh -lc '
set -e
apk add --no-cache build-base curl git
curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
rustc -vV | grep host
cargo build --release --locked
cp /tmp/target/release/routwatch "/out/routwatch-'"$arch"'-linux-musl"
'

echo "→ $out/routwatch-$arch-linux-musl"
file "$out/routwatch-$arch-linux-musl" 2>/dev/null || true
"$out/routwatch-$arch-linux-musl" --version
