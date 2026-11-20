# Changelog

Newest first. Each section lists what changed, in the words the release uses.

## 0.9.1 (unreleased)

- **Tiered polling.** `device/signal`, `monitoring/status` and
  `monitoring/traffic-statistics` are read on the refresh interval, `wlan/host-list`
  every 30 s, and the other ten endpoints every 60 s. One collection thread, and the
  first frame is still one full collect. The default `refresh_ms` is 600.
- **Per-tier age in the header.** Each group carries its own stamp, so the header
  says `signal 0.6s · clients 12s · device 41s` when there is room for it.
- **Redraw on change.** The UI repaints when a snapshot arrives or a key is pressed,
  and at least once a second, instead of on a fixed tick.
- **Observation log.** `$XDG_DATA_HOME/routwatch/events.jsonl` (0600, append only)
  records write results, IP renew results, WAN loss and return, tier read failures,
  and signal poor entry and exit. It is an observation log, never a trial journal.
- **Privacy drift.** `privacy --save`, `--diff` and `--watch` compare four checks
  (TR-069, DNS, firewall, admin password) against a 0600 baseline and report a drift
  only after two reads agree. A refusal is never a change, and `unknown` is never
  folded into `off`. `--diff` exits 1 on a drift.
- **Client inspector.** The clients tab shows the selected client's detail in a pane
  under the table, from the 30 s tier. A column the firmware does not report is shown
  as `not reported`, never invented.
- **Incidents.** `incidents list`, `incidents show <id>` and `report --incident <id>
  [--redact]` derive windows from the logs. Confidence is the count of agreeing rows.
- **The smear is derived, not fixed.** Two events are ordered only if the signal log
  brackets both of them and the widest gap between those samples is narrower than the
  distance between the events; the incident prints the gap it used. When the log does
  not bracket both moments the order is `unknown` and no cause is named.
- **Test isolation.** Every test that can reach a persistent log now writes inside its
  own temporary data home, and `scripts/gate-isolation.sh` proves it: the suite runs
  with `XDG_DATA_HOME` pointed at a sentinel directory of marker logs, which must come
  back byte-identical. Before this, a plain `cargo test` appended to the real
  `signal-history.jsonl`.
- **`tests/cli.rs` and its fixture are published.** The CLI tests were tracked before,
  so a clean clone runs the same suite the counter reports; `tests/fixtures/
  snapshot_fixed.json` was rebuilt from synthetic values, because the published file
  carried real device data.

## 0.9.0

The first release with the dashboard, bands, traffic, clients, SMS, device, DNS and
signal tabs, the headless `snapshot` / `status` / `dump` / `doctor` commands, and the
plugin-free theme set. See the release notes on GitHub for the full list.
