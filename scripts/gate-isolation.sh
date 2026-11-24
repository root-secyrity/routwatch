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
#   B. empty: the suite runs against an EMPTY data directory and must leave it
#      empty. Part A alone can miss a leak, because the signal log writes at most
#      one row a minute: a leaking test that finds a recent marker row writes
#      nothing, and the marker looks untouched. Against an empty directory any
#      write shows up as a new file.
#   C. real: with no `routwatch` process live, the same run repeats against the
#      user's real directory and must leave it byte-identical. It is skipped, with
#      the reason printed, while the app is live: the TUI appends one signal row
#      a minute, which would move the file under the comparison.
#
# One run at a time. Two concurrent runs share the cargo target directory and
# hand each other the same data homes, so one run's write lands in the other
# run's fingerprint and the gate reports a leak nobody made. The lock sits
# outside the repository on purpose: an untracked file inside it would fail the
# clean-tree gate. It is a directory because mkdir is atomic, and it holds the
# owner's pid so a run killed by a signal does not lock the gate for good.
#
# Part C is skipped only when the caller says so. A skip that still exits 0 is a
# gate that never ran, so the default is to fail and name the reason.
#
# Usage: scripts/gate-isolation.sh [extra cargo test args]
set -uo pipefail

logs=(signal-history.jsonl speed-history.jsonl events.jsonl privacy-baseline.json cells.json)
tmp=""
lock="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/routwatch-gate-isolation.lock"
if ! mkdir "$lock" 2>/dev/null; then
  owner="$(cat "$lock/pid" 2>/dev/null)"
  if [ -n "$owner" ] && kill -0 "$owner" 2>/dev/null; then
    echo "isolation: another run holds $lock (pid $owner); refusing to share the data homes"
    exit 1
  fi
  # The holder is gone: killed, or a machine that rebooted with the lock in
  # place. Take it over rather than leave the gate unrunnable.
  rm -rf "$lock"
  if ! mkdir "$lock" 2>/dev/null; then
    echo "isolation: cannot take over $lock"
    exit 1
  fi
fi
printf '%s\n' "$$" >"$lock/pid"
trap 'rm -rf "$tmp" "$lock"' EXIT
tmp="$(mktemp -d "${TMPDIR:-/tmp}/rw-isolation-XXXXXX")"

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

# ---- B. empty
mkdir -p "$tmp/empty/data" "$tmp/empty/config"
echo "→ cargo test --locked $* (empty data home)"
XDG_DATA_HOME="$tmp/empty/data" XDG_CONFIG_HOME="$tmp/empty/config" run_suite "$tmp/empty.log" "$@"
left="$(find "$tmp/empty/data" "$tmp/empty/config" -mindepth 1 | sort)"
if [ -n "$left" ]; then
  echo "$left"
  echo "isolation: the suite created files in an empty data home"
  exit 1
fi
echo "isolation: an empty data home stayed empty"

# ---- C. real
data="${XDG_DATA_HOME:-$HOME/.local/share}/routwatch"
if pgrep -x routwatch >/dev/null 2>&1; then
  if [ "${ROUTWATCH_ISOLATION_ALLOW_LIVE:-0}" = "1" ]; then
    echo "isolation: part C skipped on request: routwatch is running (pid $(pgrep -x routwatch | tr '\n' ' '))"
    echo "isolation:   ROUTWATCH_ISOLATION_ALLOW_LIVE=1 was set, so this run does not claim the real-directory check."
    exit 0
  fi
  echo "isolation: part C cannot run: routwatch is running (pid $(pgrep -x routwatch | tr '\n' ' '))"
  echo "isolation:   it appends one signal row a minute, so the comparison cannot be trusted."
  echo "isolation:   Close it and run again, or set ROUTWATCH_ISOLATION_ALLOW_LIVE=1 to skip it knowingly."
  exit 1
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
