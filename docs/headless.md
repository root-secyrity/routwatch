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

Identity never leaves by accident: `snapshot --json --redact` replaces serial,
IMEI, MACs, WAN IP, client names and addresses, and the operator name with
`(hidden)`, `dump --redact` does the same to the raw XML (keeping every tag name,
and covering wifi keys and every `*ssid`/`*ipaddress` spelling), and
`snapshot --raw --redact` is refused outright rather than pretending to be safe.
`--redact` also covers `status`, `record`, `replay` and `preview`, so the flag
means the same thing on every command.
`routwatch share` writes the whole ask-for-a-model document - capabilities,
endpoint field names, redacted snapshot - as `0600` in the current directory.
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
file, so it works offline and needs no `--allow-data`.

If you are publishing the output of `dump` somewhere else, prefer
`dump --redact` over editing a raw file by hand; see
[`../CONTRIBUTING.md`](../CONTRIBUTING.md) for what a public issue may contain.

Back: [README](../README.md) · [all docs](README.md)
