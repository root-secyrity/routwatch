# Changelog

Newest first. Each section lists what changed, in the words the release uses.

## Unreleased

**The signal log remembers the cell, and the guard reads it back**

- A signal row is v2: it carries the downlink and uplink earfcn as numbers and the
  transmission mode. `DL:100 UL:18100` is parsed once, on the way in; a malformed
  half writes `null` rather than a guess. A v1 row still reads with those fields
  empty, and a row from a newer version is refused with one warning.
- The log keeps three days instead of one (4320 rows, one a minute), because the
  guard needs two complete previous days to have a baseline. Measured 2026-11-22:
  trimming 4380 rows took 131 ms and turned 813 KB into 802 KB. A trim keeps each
  row's own version, so a v1 row stays v1 instead of being relabelled v2.
- **The drift guard**: the signal tab draws one line when today's best SINR on the
  serving cell is 4 dB or more below the median of the previous days' best, and one
  line while it is still collecting. It draws nothing otherwise, and writes one
  observation event per alert, never one per check. A pci change inside one earfcn
  is reported as another cell and kept out of the drift number.
- **`routwatch antenna worth`**: one class per cell from the saved log,
  signal-limited, interference-limited, not-signal-limited or insufficient, using
  the same cuts as the gauges. RSRQ is shown and kept out of the class, and a saved
  speed run is shown beside it, not inside it: three runs at one position and
  constant RSRP moved x1.69 (measured 2026-11-22), so throughput cannot place the
  lever. No purchase advice and no causal claim.

## 0.9.2

**Sessions are closed on every exit path** (#69)

- Every command now closes its router session on the way out, including the error
  paths. The client logs in on a flag and `Drop` posts the logout while the flag is
  still set, so a read that fails with `?` no longer leaves a session behind. The
  firmware keeps ten sessions at once and refuses the eleventh with `108003`
  (measured 2026-11-22: the eleventh consecutive `status --json` failed, the
  sessions freed themselves 259 s later); twenty consecutive runs after the fix all
  succeeded, where the eleventh used to fail for minutes. Automation such as cron and
  monitoring is the part that felt this.
- Seven permanent tests hold it: a failed read still logs out, an early return logs
  out, a manual logout is not doubled by `Drop`, a client that never logged in never
  logs out, twenty sequential binary runs never exhaust the sessions, a dropped
  client logs out on the error path, and no `process::exit` or `panic = "abort"` can
  bypass the guard.

**The tab bar names the view only** (#69)

- The `full/braille` indicator is gone. The view (`full`, `gauges`, `lite`,
  `dense`) is a real axis toggled by `v` and stays; the renderer name beside it was
  half of a choice that no longer exists, left over from retiring the second
  renderer. A test fails if the bar names a renderer again.

**Measured values have a colour of their own** (#70)

- Neutral measurements (durations, byte totals, message counters) are drawn in one
  `value` colour per palette, as a whole value and never digit by digit. State
  colours outrank it, identifiers are never split, and the words stay the same, so a
  terminal without colour loses nothing.
- Five permanent tests hold it: a measurement uses the value colour, a semantic cell
  keeps its state colour, an identifier is one colour, a tab number keeps the tab bar
  colour, and the frame reads the same without the value colour.

**Input responsiveness** (#67)

- A key press is drawn in the pass that reads it instead of waiting for the one
  second pulse that keeps the age text moving. Measured over a PTY, key to first
  byte fell from p50 400 ms at 120x34 and a 600 ms refresh (p95 601 ms; p50
  802 ms and p95 1002 ms at a 2000 ms refresh) to p50 0.3-0.5 ms and p95 1.5 ms
  at both 120x34 and 240x60.
- The loop drains every key already waiting before it draws, so a burst of ten
  keys costs one frame, and an idle loop still blocks on the poll instead of
  spinning.
- Five permanent tests hold it: a batch is drained before one draw, a state
  changing key owes a frame without a pulse, a silent worker does not block
  input, a tab switch does not clear the terminal, and browsing a list sends
  nothing to the worker.

**Instant boot and a lighter frame** (#68)

- The first frame is drawn before the process reads the usage ledger, the
  history ring or the incident list, so the screen answers at once; those reads
  are 2.8 ms in total and now happen behind a painted frame. A panel with no
  data yet says "collecting the first sample…" and every field stays `-` until a
  value arrives, so nothing is guessed.
- The redundant clear-all at startup is gone. `Terminal::clear` asks for the
  cursor position first, and a terminal that never answers that query costs
  crossterm's 2 s timeout before the first frame (measured over a PTY, 2026-11-22);
  on the alternate screen the first draw paints every cell anyway.
- The client tab caches the display order and rebuilds it only when the client
  list or the sort changes. The row loop used to re-sort the list once per row.
  With 40 clients a 120x34 frame dropped from p50 608 us to 228 us and a 240x60
  frame from p50 1018 us to 469 us, flat in the number of clients.
- Five permanent tests hold this: the cached order is reused between draws and
  rebuilt when the client list or the sort changes, the boot frame is drawn
  before the first disk read, a panel with no data says it is collecting, and an
  empty field is a dash and never a number.

## 0.9.1

**Polling in tiers**

- `device/signal`, `monitoring/status` and `monitoring/traffic-statistics` are read
  on the refresh interval, `wlan/host-list` every 30 s, and the other ten endpoints
  every 60 s, each group on its own deadline. One collection thread, and the first
  frame is still one full collect. The default `refresh_ms` is 600.
- The header shows one age per group (`signal 0.6s · clients 12s · device 41s`) when
  there is room for it, and the UI repaints on a snapshot or a key press, at least
  once a second, instead of on a fixed tick.
- The history ring takes one sample per radio read, not one per arriving snapshot,
  so a clients or device tick no longer duplicates the signal point.
- `stale` keeps a floor of two seconds whatever the refresh interval, because the
  firmware itself stalls near 1.6 s on `device/signal`.

**Observation**

- `$XDG_DATA_HOME/routwatch/events.jsonl` (0600, append only) records write results,
  IP renew results, WAN loss and return, tier read failures, and signal poor entry
  and exit. It is an observation log, never a trial journal.
- `privacy --save`, `--diff` and `--watch` compare four checks (TR-069, DNS,
  firewall, admin password) against a 0600 baseline and report a drift only after two
  reads agree. A refusal is never a change, and `unknown` is never folded into `off`.
  `--diff` exits 1 on a drift.
- The clients tab shows the selected client's detail in a pane under the table, read
  from the 30 s tier. A column the firmware does not report is shown as
  `not reported`, never invented.
- `incidents list`, `incidents show <id>` and `report --incident <id> [--redact]`
  derive windows from the logs; confidence is the count of agreeing rows.
- Two events are ordered only if the signal log brackets both of them and the widest
  gap between those samples is narrower than the distance between the events. The
  incident prints the gap it used, and when the log does not bracket both moments the
  order is `unknown` and no cause is named.

**Closure**

- Every test that can reach a persistent log writes inside its own temporary data
  home, and `scripts/gate-isolation.sh` proves it three ways: the suite runs against a
  sentinel directory of marker logs, which must come back byte-identical; against an
  empty directory, which must stay empty; and against the user's real directory, which
  must not move. The sentinel check alone can miss a leak, because the signal log
  writes at most one row a minute and a leaking test that finds a recent marker writes
  nothing.
- The smear an incident measures now comes from the samples around its own window,
  not from the widest gap in the whole log: a night of silence no longer mutes the
  next day.
- `tests/cli.rs` and its fixture are published, so a clean clone runs the same suite
  the counter reports. `tests/fixtures/snapshot_fixed.json` was rebuilt from synthetic
  values, because the earlier file carried real device data.

**Capabilities, config and advice**

- `capabilities [--json] [--probe]` records, per endpoint, whether this firmware
  answered, refused it with a code, did not serve it (404) or was never tested, keyed
  by the firmware string and the measurement moment, at
  `$XDG_DATA_HOME/routwatch/capabilities.json` (0600). A refusal is recorded, not read
  as absence, and an untested endpoint never hides a feature.
- `config snapshot`, `config diff [--against latest|<file>]` and `config list` keep
  field names and salted fingerprints only, never a value, in
  `$XDG_DATA_HOME/routwatch/config-snapshots/` (files 0600, directory 0700, the newest
  20 kept). A change needs two agreeing reads, an endpoint that stops answering reads
  as `visibility-changed`, a firmware difference between the two snapshots suppresses
  the field alarms, and there is no restore.
- `bands advise [--json] [--window <secs>] [--min-rows N] [--scans <file>]` ranks what
  the logs support. The log records the served band, not the configured set, so set
  advice comes only from `scan --json` results and the two are never mixed. Four
  confidence levels, a reading inside the derived smear stays unattributed, and a
  correlation is printed as an association. It writes nothing, needs no router, and
  switches nothing.

## 0.9.0

The first release with the dashboard, bands, traffic, clients, SMS, device, DNS and
signal tabs, the headless `snapshot` / `status` / `dump` / `doctor` commands, and the
plugin-free theme set. See the release notes on GitHub for the full list.
