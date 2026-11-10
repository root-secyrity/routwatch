# Limits (stated, not hidden)

This file answers: what is known not to work, what looks broken but is not, and
why each one is stated instead of hidden.

- **SMS**: reading the inbox works on H122-373 when the request carries
  `Ascending` (it did not before). A firmware that still answers `100005` is
  reporting a format error; the tab says the request was rejected, not that SMS
  is blocked.
- **Band writes can answer `100006` after they land**: on H122-373 the radio
  accepts the payload and the POST still returns an undefined code. The verdict
  comes from reading `net/net-mode` back, never from the code; routwatch waits
  one grace period and then says what the device reports. A `100006` with no
  change behind it is reported as a failure.
- **5G per-field metrics can be empty**: on some firmware the `nrrsrp`, `nrrsrq`
  and `nrsinr` readings stay blank even with ENDC active, while `SignalIconNr`
  (0–5 bars) and the ENDC flag keep arriving. The signal panel then shows the bars
  and says which `nr` fields did arrive (`nrearfcn`, bandwidths, rank, BLER, CQI,
  TX power) rather than inventing dBm; it switches to the full gauges the moment
  any one of `nrrsrp`, `nrrsrq` or `nrsinr` carries a value.
- **The budget is local**: the firmware answers `monitoring/month_statistics`,
  but routwatch does not read the monthly counters; the bar compares your own
  `budget_gb` against the traffic it samples.
- **Reports are redacted unless you ask**: `e` writes `0600` Markdown in a `0700`
  directory, with IMEI, serial, WAN IP, MACs and SSIDs hidden - these files get
  pasted in public. `report_identity`, `--report-identity` or `i` in the settings
  overlay writes the full version, and a report never overwrites another.
- **`auto` theme is best-effort**: it reads the running desktop's published colours
  and falls back to `terminal`. The default is still `terminal`, so no machine
  changes appearance unasked.
- **Instrument band**: the dashboard shows RSRP, SINR, clients and a signal map on
  rows 3–9. `UPTIME` and `TEMP` appear only when the router reports them, and the
  band re-flows to four, five or six equal cells instead of leaving a hole. It
  needs 27 body rows; below that the dashboard blocks keep the space.
- **Small terminals**: the frame degrades in a fixed order instead of clipping. The
  tab bar disappears entirely below 12 rows (measured: 100x11 has none), and
  becomes `1/9 Dash · Tab next` below 76 columns; below 60
  columns the status line keeps one fact plus the keys, below 40 only the keys; the
  dashboard drops from four blocks to signal + speed + history (under 25 rows) and
  to one signal panel at the 24x6 floor, where `too small` names the size it needs.
- **Graph colour assumes a dark terminal when the theme has no background of its
  own**: six of the eight themes set `bg = Reset`, where the background belongs to
  the terminal and cannot be read. The ramp then anchors on a stated dark
  assumption, so the dimmest cells can be dimmer than the measured 4:1 floor on a
  light terminal. `sky` and `paper` paint their own background and are measured
  exactly.
- **One renderer draws every plot**: `ui::chart` owns the braille area, the
  mirrored pair and the axis. `ui::widgets` no longer carries `spark` or
  `spark_scaled`; a panel that wants a trend calls `chart::spark_auto` (auto
  ceiling) or `chart::spark_lines_scaled` (a fixed, possibly negative domain such
  as RSRP).
- **`bars` uses the height it is given**: a block sparkline used to be clamped to
  one row whatever the caller asked for, so a three-row slot in the speed panel
  left two rows empty. It now stacks eighths over the whole slot; `dots` and
  `braille` are still the braille plots, and `braille` stays a flat colour.

The write path itself is a promise, not a limitation, and it is kept by a test:
reads are free, writes are confirmed and read back. The three rules are in the
README's Safety list, and the guard that enforces them is in
[`contracts.md`](contracts.md).

Back: [README](../README.md) · [all docs](README.md)
