# Changelog

Newest first. Each section lists what changed, in the words the release uses.

## Unreleased

**The Align tab names what it cannot compare, and leads with one reading**

- **A window with nothing to compare against says so instead of inventing a direction**:
  the engine returned `steady` whenever the previous 15 s window was empty, so the first
  fifteen seconds of every session, and the whole tab on a fresh start, claimed a hold
  nobody had measured. It now returns `NOT COMPARABLE YET`, and the tab prints the average,
  says no 15 s window exists before it, and counts the seconds of reads it still needs.
  The arrow follows the direction, so it is absent in that state rather than a mark.
- **The words name the action**: `IMPROVING` and `DEGRADING` past the 3.6 dB deadband,
  `HOLD HERE` inside it, `NOT COMPARABLE YET` when there is no comparison. The deadband,
  the 15 s window and the peak rule are unchanged.
- **The tab reads as two columns, and the number carries its unit**: `▲ IMPROVING  4.7 dB`
  on the left and the fact that explains it on the right (`over 15 s · 7 reads`,
  `SINR average · poor`, `hold band ±3.6 dB`, `peak this session 5.0 dB · 12 s ago`),
  each reading printed with its own unit. The four-row direction block and the five rows of
  block digits are gone; the state is one row and one rule. The footer names its three
  groups in the sketch's words: `RSRP -96 dBm · enough` (the advisor's own fair threshold),
  `RSRQ -8 dB · cell load`, `TM[4] · CQI 14`, and it drops below 100 columns or under 20
  rows. The 30 s strip draws eight levels of blocks scaled to its own window, with `30 s`
  at its left edge and `┤ now` at its right, and it stays blocks under either graph style.
- **Two designed captures hold the engine**: `fixtures/align-sweep.csv` (a ramp, a plateau,
  a fall and a cell change) turns the arrow on rows 21 to 26 and 45 to 50 and resets the
  peak; `fixtures/stationary.csv` (200 reads, 1.202 dB spread) never turns it, reading
  `NOT COMPARABLE YET` on its first four rows and `HOLD HERE` on the other 196. Both are
  synthetic; the shape of a real sweep is still unmeasured.

## 0.10.0

**The log remembers the cells, ranks its own smear, and a tenth tab reads the average**

- **Two events are ordered by their own stamps, and the smear ranks only a reading against
  a change**: the incident caveat and `compare` used the signal smear to refuse an order
  between two events, which are two rows the log already stamped. A pair inside the same
  second now says so instead (the log records whole seconds), the smear ranks a reading
  against a change only, and `bands advise` prints "smear not measurable" rather than
  `0 ms` when its window holds fewer than two reads, with `smear_ms` `null` in its JSON.
- **The drift guard counts sampled minutes, not a span**: today must hold six hours of
  distinct sampled minutes on the judged cell, so a day with two reads seven hours apart
  is two minutes of coverage and the guard stays collecting. It names the span beside the
  samples instead of trusting the gap between the first and last row.
- **A tenth tab, Align, reads the average instead of one sample**: `0` opens it. The big
  number is the 15 s moving average of the SINR reads, in its state colour, with an arrow
  that moves only past 3.6 dB (three measured standard deviations of the spread on this
  link), the session peak with the cell it was reached in (cleared when the PCI changes and
  said so on screen), a one row strip of the last 30 s whose right edge is the newest read,
  RSRP as a constraint, RSRQ as cell load, and TM and CQI as context. Nothing on it claims
  to be instant or real-time: the measured cadence is 939 ms at the median and 1748 ms at
  the 95th percentile, so a window holds reads, not seconds. No sweep shape is claimed.
- **The incident smear is ranked inside its window, and a pause cannot widen it**: the
  figure `compare` and `bands advise` quote is now the 95th percentile of the gaps between
  the reads inside their own window, taken from a sorted copy, instead of the widest gap in
  the log read in file order. A log whose rows were not stamped in order no longer reports
  a smear of hours, and a window with fewer than two reads reports no smear rather than a
  number the rows do not support.
- **The observation log records a serving-cell change**: one `cell` row when the key
  changes, held over two reads, with the key before and after, the band and the TAC the
  read reported, and no verdict on the signal. It opens no incident and no cause, and
  `ROUTWATCH_DISABLE_SIGNAL_LOG=1` does not mute it: the switch keeps a file small, it does
  not lose the timeline. `routwatch cells changes [--json] [--window SECS]` prints them.
- **`routwatch compare --around ID` puts one cell beside itself**: two windows of 1800 s
  from the local signal, speed and event logs, with the median and the 10th percentile, a
  class past three measured standard deviations of noise, `insufficient` instead of a delta
  when a side has under 10 rows or under half its window covered, and the download
  difference only when both sides hold two speed reads. A key change inside the window
  splits it, so two cells are never held against each other, and nothing is read from MCS.

- **`routwatch budget [--json]`**: what this cycle spent, the limit, the pace the
  ledger measured and how much of the window that pace was divided by, read from
  the local ledger alone: no config file, no router session. With no config the
  100 GB default is labelled a default (`budget_default`), so it cannot pass for a
  configured limit. The Traffic panel prints the same two figures when the screen
  has the room for them and keeps its reset row when it does not.
- **The budget crossing is reported at 80% and 95%**: one `budget` row per
  crossing, once per cycle, with the bytes, the limit and the coverage; the flags
  are persisted so a restart does not repeat a row, and every restart of the
  baseline clears them. The pace is projected from the watched part of the window
  and says so (`measured`, `thin`, `insufficient`) instead of dividing by a window
  that was partly unobserved.
- **A weak CQI is marked where the CQI is already printed**: `CQI 6 weak` and
  below, the number alone from 7, on all three lines that carry a CQI. The label
  is a reading, never a cause.
- **A transmission-mode change is a row in the observation log**: the TUI worker
  writes it once the new value has been read twice in a row, and a read that names
  a different cell resets the baseline instead of reporting one.
- **`ROUTWATCH_DISABLE_SIGNAL_LOG=1` now silences the cells catalog too**: one
  switch for both background records of the same read.
- **`scripts/gate-isolation.sh` holds one run at a time**: concurrent runs shared
  the data homes and the fingerprints, so a second run now exits with one line and
  the lock carries its owner's pid. Part C no longer exits 0 with a skipped line
  while the app is live; a skip must be asked for with
  `ROUTWATCH_ISOLATION_ALLOW_LIVE=1`.
- **`routwatch cells list`**: the serving cells this machine has read, newest first,
  from `$XDG_DATA_HOME/routwatch/cells.json` (`0600`). A cell is keyed by downlink
  ARFCN and PCI; a sample whose ARFCN did not parse becomes its own `earfcn_unknown`
  record instead of a merge, and PCI alone is never a key. The store is capped at 64
  cells, drops the least recently seen, is rewritten once a minute and immediately
  when a cell is new, and carries no ranking, no best cell and no throughput
  comparison: `--json` prints the records, `docs/headless.md` states the shape.
  `docs/limits.md` states the key rule, the cap, the write cadence and why a
  comparison is not offered yet.

## 0.9.3

**The signal log remembers the cell, and the guard reads it back** (#74)

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
- The two CLI readers of the signal log now print the note the TUI already showed
  when rows are skipped: damaged lines, and rows written by a newer routwatch. A
  skipped row is a reported row, not a quietly shorter log.
- A write to the signal log tries the sidecar lock three times, 5 ms apart, before
  it gives up on the sample: a holder that lasts milliseconds is not the same thing
  as another routwatch trimming the log, and the wait stays under 10 ms.
- **`Shift+W` renews the WAN address from the Device section**: the same write the
  CLI already had (`routwatch ip renew`), behind the same confirm, whose rows print
  the address now and whose warning says the connection drops for a few seconds and
  the ISP may keep the lease. With writing off, or with no sample yet, the key says
  why instead of opening a confirm that cannot compare anything.
- **The dashboard answers the four keys its own panels advertise**: its hint and the
  clients panel footer promised `j`/`k`, `enter opens a client`, `s sort` and
  `f filter`, and none of the four had an arm on that section (lowercase `s` opened
  the settings overlay, `f` and `Enter` did nothing). `Enter` now opens the row in
  the Clients section with that client still selected, `s` sorts and names the order
  (this footer does not print the active key), `f` narrows the table to one band, and
  `j`/`k` move the dashboard's own cursor. `Shift+S` and `,` still open settings.
- **The `?` overlay is grouped by section, and it scrolls**: the keys are filed under
  headings (GLOBAL KEYS, SETTINGS, then one per section that answers a key of its own,
  through DNS), and a window shorter than the list names the lines it shows
  (`rows 1-15 of 45`), so the last sections are reached by scrolling rather than by
  resizing the terminal. `j` `k` and `PgUp`/`PgDn` move the
  window while it is open, so they no longer move the selection behind it; `?` and
  `Esc` still close it, and opening starts at the top. A heading whose rows all fell
  outside the window is not drawn.
- **A long help description is wrapped, and a section is a band**: a description that
  did not fit the panel used to be cut and ended in an ellipsis, so the longest rows
  (the SMS keys, the dashboard row, `W renew`, the settings `i`) lost their ending. The
  text now continues on the next line, indented under the description column, and the
  panel is measured in the lines the rows actually draw rather than in their number: at
  120 columns the whole list needs 56 rows and anything shorter scrolls, exactly as
  before. A section heading is now a filled bar (the label drawn on the title colour)
  because two palette slots can resolve to one colour, which made the headings look
  like the key column on a terminal theme. The keys column is still cut, which is why
  `Enter / s / f` shows an ellipsis.

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
