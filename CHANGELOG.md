# Changelog

Newest first. Each section lists what changed, in the words the release uses.

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
