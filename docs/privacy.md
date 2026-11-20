# Privacy: watch a setting, not a guess

This page answers two questions: what `routwatch privacy` reads, and when a
drift it reports can be believed. The measured caveats behind both (which
endpoints this firmware refuses, and why two reads are required) are in
[`limits.md`](limits.md); nothing here is repeated there.

The audit is read-only. Every check issues a `GET`; `the_audit_never_posts` holds
that, so running it can never change the answer it is measuring.

## The four checks

| Check | Source | What a state means |
|---|---|---|
| `tr069` | a `Tr069Enable` field when the firmware serves the path, otherwise the declared refusal (what the H122-373 answers; see [`limits.md`](limits.md)) | `on`/`off` from the field; a refused endpoint is `not exposed` with its code |
| `dns` | the server DHCP advertises (`dhcp/settings`) and the one the link reports (`monitoring/status`) | `on` when both names agree, `unknown` when they differ or one is missing |
| `firewall` | `FirewallMainSwitch` on `security/firewall-switch` | `on`/`off` from the field, with the field named beside it |
| `admin` | LAN address (`dhcp/settings`) and `WifiEnable` (`wlan/basic-settings`) | always `unknown`: no field reports whether the web UI needs a password over WLAN, so it prints what it read instead of a pass |

The states are words, not guesses: `on`, `off`, `not exposed` (the firmware
refused the path, with the code), and `unknown` (a value arrived that is neither
`0` nor `1`, or nothing arrived). The text table prints `not exposed`; `--json`
carries the same state as `notexposed`, one word, because the field is consumed by
scripts.

```sh
routwatch privacy          # the audit, as a table
routwatch privacy --json   # {"checks":[{"name","state","detail"}, ...]}
```

## A baseline, and what counts as drift

`--save` records the answers you accept. The file is
`$XDG_DATA_HOME/routwatch/privacy-baseline.json`, written 0600 through the same
writer the config uses, and it holds the check names and their state words, no
device identity.

```sh
routwatch privacy --save         # record the answers you accept
routwatch privacy --diff         # exits 1 and names each settled drift
routwatch privacy --diff --json  # baseline, saved_at, checks, drift[{name,from,to}]
```

`--diff` reads the audit twice, two seconds apart, and reports a check only when
both reads agree on the new value. One read of a document this firmware answers
from a flaky tail is a rumour, not evidence. Two things are never a change: a
refusal (`not exposed`, `unknown`) is not counted as a setting turning off, and a
check missing from the read says nothing at all.

Exit code 1 means a settled drift, so a script can act on it without parsing the
text.

## Watch

```sh
routwatch privacy --watch 60     # a line whenever a drift settles
```

`--watch` reads against the same baseline, so it needs a `--save` first. It
holds one session and reads the checks on that interval, ten seconds at the
smallest. A router reboot or an expired session shows up as
`unknown` rows, which are not drift, so a watch left running reports nothing
rather than inventing a change: restart it after a reboot. It reads only.

## The client inspector

Section 5 carries a pane under the client table for the selected host:

- the six fields `wlan/host-list` sends: name, IP, MAC, band, SSID, time
  associated;
- the age of that read, so the pane cannot look fresher than the tier behind it;
- the three columns this firmware does not report (per-client traffic, signal and
  uptime), named `not reported` rather than printed as a zero.

There is no per-client detail endpoint on the H122-373, so the inspector is the
30 s client read and nothing faster. The numbers behind that sentence are in
[`limits.md`](limits.md).

Back: [README](../README.md) · [`docs/`](README.md)
