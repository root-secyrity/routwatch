# Headless

This file answers: how do I put routwatch in a status bar, and how do I read its
output from a script?

```sh
routwatch status --waybar
# {"text":"LTE ▄ -96dBm","tooltip":"Test Net (00101)\nLTE · ENDC off · 5 bars\n…","class":"fair","percentage":46}

routwatch snapshot --json
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

If you are publishing the output of `dump` somewhere else, prefer
`dump --redact` over editing a raw file by hand; see
[`../CONTRIBUTING.md`](../CONTRIBUTING.md) for what a public issue may contain.

Back: [README](../README.md) · [all docs](README.md)
