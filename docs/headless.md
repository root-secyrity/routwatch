# Headless

This file answers: how do I put routwatch in a status bar, and how do I read its
output from a script?

```sh
routwatch status --waybar
# {"text":"LTE ▄ -96dBm","tooltip":"Test Net (00101)\nLTE · ENDC off · 5 bars\n…","class":"fair","percentage":46}

routwatch snapshot --json
routwatch cells list --json       # the serving cells this machine read, newest first
routwatch doctor --json          # capabilities with state/reason/detail/next_check
routwatch dump --json            # every raw endpoint → routwatch-dump.json in $PWD
routwatch share                  # one file: capabilities, endpoint field names,
                                 # redacted snapshot - the ask-for-a-model document
```

Waybar module:

```jsonc
"custom/routwatch": {
  "exec": "routwatch status --waybar",
  "interval": 5,
  "return-type": "json",
  "on-click": "kitty -e routwatch"
}
```

Exit codes: `0` ok · `1` router or write failure (the real reason is printed) ·
`2` network/unreachable (the Waybar document still prints, `class: "offline"`) ·
`3` usage, credentials or config.

The Waybar `class` is one of `good`, `fair`, `poor`, `unknown` (a live link the
firmware gives no RSRP for) or `offline` (no link). `status --json` reports
`dl_bps` and `ul_bps` in bits per second, and `clients_known` is false when the
firmware refused the host list instead of printing a fake zero.

`cells list --json` (no network, no config) prints one array of the records the
catalog holds, newest `last_seen` first. The keys are fixed:

```json
[
  {
    "band": "B1",
    "pci": 481,
    "earfcn_dl": 100,
    "earfcn_ul": 18100,
    "tac": 516,
    "first_seen": 1763634127,
    "last_seen": 1763981580,
    "rows": 1438,
    "best_sinr": 9,
    "best_sinr_at": 1763979000,
    "earfcn_unknown": false
  }
]
```

`earfcn_dl`, `earfcn_ul`, `tac` and `best_sinr` are `null` when the firmware never
answered them, and `best_sinr_at` is `null` while `best_sinr` is. An entry with
`earfcn_unknown: true` is a cell whose ARFCN did not parse: `earfcn_dl` is `null`
there by construction, and the record is a separate cell, never a merge. A missing
file reads as `[]` with exit 0; a damaged one warns on stderr and prints the records
that still read. The text table prints the same set with `best_sinr` as `9 dB` and
both stamps in local ISO 8601.

`cells changes [--json] [--window SECS]` (no network, no config) prints the `cell` rows
of the observation log, newest first: the event id, the stamp, the tier, the outcome and
the detail the worker wrote. `--window` keeps only the rows whose `at_unix` falls inside
the last SECS counted back from the newest cell row (not from the clock, so an old log
still lists something). The keys are fixed, and `detail` is absent when the row had none:

```json
[
  {
    "id": "cell-1763981580",
    "at_unix": 1763981580,
    "at": "2026-11-24T08:13:00",
    "tier": "signal",
    "outcome": "changed",
    "detail": "earfcn_dl 100 pci 481 -> earfcn_dl 300 pci 395 band B3 tac 516 held 2"
  }
]
```

`compare --around ID [--window SECS] [--json]` (no network, no config, exit `3` when the
id is not in the log) compares one cell with itself on both sides of that event, 1800 s
each side by default. `segments` holds one entry per stretch of one cell, so a key change
inside the window splits it instead of comparing two cells; a side with fewer than 10 rows
or under half the window covered is `insufficient` and carries no delta. `class` is one of
`improved`, `degraded`, `within_noise`, `mixed`, `insufficient`, and `confidence` reuses the
advisor's words (`insufficient`, `low`, `medium`, `high`). `smear_ms` is the 95th
percentile of the gaps between the reads inside the window, and `null` when the window holds
fewer than two of them: a window cannot rank an order it never saw. `notes` and `missing`
are plain sentences, never a verdict:

```json
{
  "event_id": "cell-1763981580",
  "event_kind": "cell",
  "event_at_unix": 1763981580,
  "event_detail": "earfcn_dl 100 pci 481 -> earfcn_dl 300 pci 395 band B3 tac 516 held 2",
  "window_secs": 600,
  "smear_ms": 120000,
  "segments": [
    {
      "cell": "earfcn_dl 100 pci 481",
      "before": {
        "rows": 10,
        "watched_secs": 600,
        "coverage": 1.0,
        "sinr_median": 10.0,
        "sinr_p10": 10.0,
        "rsrp_median": -100.0,
        "download_median_mbps": null,
        "speed_rows": 0
      },
      "after": {
        "rows": 10,
        "watched_secs": 600,
        "coverage": 1.0,
        "sinr_median": 15.0,
        "sinr_p10": 15.0,
        "rsrp_median": -100.0,
        "download_median_mbps": null,
        "speed_rows": 0
      },
      "sinr_delta_db": 5.0,
      "rsrp_delta_db": 0.0,
      "download_delta_mbps": null,
      "class": "improved",
      "confidence": "low"
    }
  ],
  "confidence": "low",
  "notes": [],
  "missing": ["speed: a side has fewer than two reads, so there is no difference"]
}
```

`routwatch budget [--json]` answers what this cycle has spent and how fast, from
the local ledger alone: no config file, no router session, exit `0` even when the
router is unreachable. `--json` prints the fixed keys:

```json
{
  "used_bytes": 2000000000,
  "budget_gb": 100,
  "budget_bytes": 107374182400,
  "percent": 1.862645149230957,
  "window_secs": 7200,
  "watched_secs": 7200,
  "coverage": "measured",
  "per_day_bytes": 24000000000,
  "days_left": 5,
  "over": false,
  "as_of": "2026-09-21 19:13",
  "budget_default": true
}
```

`per_day_bytes` and `days_left` are `null` when the ledger watched too little of
the window for a pace, `coverage` is one of `measured`, `thin` or `insufficient`,
and `budget_default` is true only when no config file was found, so a limit
nobody set never reads as a configured one. The text form prints the same figures
as four lines, and `docs/limits.md` states where the cuts and the baseline come
from.

Identity never leaves by accident: `snapshot --json --redact` replaces serial,
IMEI, MACs, WAN IP, client addresses, the DNS resolvers and the operator name
with `(hidden)` and client names with `(client N)`, `dump --redact` does the same to
the raw XML
(keeping every tag name and the document's shape, and hiding the value of every
element or attribute whose name is an identity word: wifi keys and passwords,
`*ssid` and `*ipaddress` spellings, every name carrying `macaddr`, the SIM and
operator fields, the SMS sender, body and service centre, and the DNS
resolvers), and `snapshot --raw --redact` is refused outright
rather than pretending to be safe.
`--redact` also covers `status`, `record`, `replay` and `preview`, so the same flag
covers every command that prints a snapshot or a frame.
`routwatch share` writes the whole ask-for-a-model document - capabilities,
endpoint field names, redacted snapshot - as `0600` in the current directory, and the
directory it has to create is `0700` (one that already exists keeps its own mode,
and a parent created above it follows the umask).
`dump` follows the same rule for its output (json or text): `0600` inside a
`0700` directory, and an existing file is replaced only with `--force`.

`routwatch speedtest` measures the path from this machine through the CPE, in
plain HTTP: it is not the router measuring its own WAN link, and it spends data
(60 MB down and 10 MB up with the default bounds) that also counts in the
router's month total. It needs no router session, so it works on a machine that
never logged in. `--dry-run` prints the plan and sends nothing, `--json` prints
that same plan there and the result otherwise, `--download-only` skips the
upload, `--max-mb`, `--upload-mb` and `--seconds` change the bounds (clamped the
same way the TUI clamps them), and a real run needs `--allow-data`, which exits
`3` without it.

A run that succeeds is appended to `$XDG_DATA_HOME/routwatch/speed-history.jsonl`
(`0600`, the newest two hundred kept) with `source = "cli"`; the row carries no
ping figures, because the CLI does not ping and a saved row must describe what
was measured. `--no-save` leaves that run out of the file, and
`speedtest --history` lists the saved rows without touching the network or the
file, so it works offline and needs no `--allow-data`; with `--json` it prints
the rows in the shape the file holds instead of the text table.

If you are publishing the output of `dump` somewhere else, prefer
`dump --redact` over editing a raw file by hand; see
[`../CONTRIBUTING.md`](../CONTRIBUTING.md) for what a public issue may contain.

Back: [README](../README.md) · [all docs](README.md)
