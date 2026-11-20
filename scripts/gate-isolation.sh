#!/usr/bin/env bash
# Gate: the test suite must not write a data directory it does not own.
#
# Every persistent log resolves its path through `dirs::data_dir()`, which
# honours `XDG_DATA_HOME`. Two fingerprints, in this order:
#
#   A. sentinel: the suite runs with `XDG_DATA_HOME` pointed at a directory
#      holding four marker logs. If any test reaches a write path without its own
#      scratch directory, the marker moves and the gate fails. This part is safe
#      while the app is running, because the app never writes the sentinel.
#   B. real: with no `routwatch` process live, the same run repeats against the
#      user's real directory and must leave it byte-identical. It is skipped, with
#      the reason printed, while the app is live: the TUI appends one signal row
#      a minute, which would move the file under the comparison.
#
# Usage: scripts/gate-isolation.sh [extra cargo test args]
set -uo pipefail

logs=(signal-history.jsonl speed-history.jsonl events.jsonl privacy-baseline.json)
tmp="$(mktemp -d "${TMPDIR:-/tmp}/rw-isolation-XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

fingerprint() {
  local dir="$1" out="$2"
  : >"$out"
  if [ -d "$dir" ]; then
    find "$dir" -maxdepth 1 -type f -printf '%f %s %T@\n' | sort >>"$out"
    local f
    for f in "${logs[@]}"; do
      [ -f "$dir/$f" ] && sha256sum "$dir/$f" >>"$out"
    done
  fi
}

run_suite() {
  local log="$1"
  shift
  if ! cargo test --locked "$@" >"$log" 2>&1; then
    tail -n 40 "$log"
    echo "isolation: the suite failed, so this gate says nothing"
    exit 1
  fi
}

# ---- A. sentinel
sentinel="$tmp/data/routwatch"
mkdir -p "$sentinel" "$tmp/config"
for f in "${logs[@]}"; do printf '{"marker":"isolation-gate"}\n' >"$sentinel/$f"; done
fingerprint "$sentinel" "$tmp/sentinel-before"
echo "→ cargo test --locked $* (sentinel data home)"
XDG_DATA_HOME="$tmp/data" XDG_CONFIG_HOME="$tmp/config" run_suite "$tmp/sentinel.log" "$@"
fingerprint "$sentinel" "$tmp/sentinel-after"
if ! cmp -s "$tmp/sentinel-before" "$tmp/sentinel-after"; then
  diff -u "$tmp/sentinel-before" "$tmp/sentinel-after" || true
  echo "isolation: a test wrote the shared data home instead of its own"
  exit 1
fi
echo "isolation: sentinel unchanged, no test uses the shared data home"

# ---- B. real
data="${XDG_DATA_HOME:-$HOME/.local/share}/routwatch"
if pgrep -x routwatch >/dev/null 2>&1; then
  echo "isolation: part B skipped: routwatch is running (pid $(pgrep -x routwatch | tr '\n' ' '))"
  echo "isolation:   it appends one signal row a minute, so the comparison cannot be trusted."
  echo "isolation:   Close it and run this gate again for the real-directory check."
  exit 0
fi
fingerprint "$data" "$tmp/real-before"
echo "→ cargo test --locked $* (real data home)"
run_suite "$tmp/real.log" "$@"
fingerprint "$data" "$tmp/real-after"
if ! cmp -s "$tmp/real-before" "$tmp/real-after"; then
  diff -u "$tmp/real-before" "$tmp/real-after" || true
  echo "isolation: the real data directory changed during the test run"
  exit 1
fi
echo "isolation: ${#logs[@]} logs unchanged in $data"
