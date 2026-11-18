# Keys

This file answers: what does each key do, and where does it work?

Letters are matched case-free (`v` and `V` are the same key) except where the
table names `Shift`. Every section prints the keys that work there in the status
bar, and `?` opens the same list inside the tool; this file is the long form.
The code and this list are held together by a test: every key the handler answers
to has a row in the help table.

## Getting around

| Key | Where | Action |
|---|---|---|
| `1`–`9` / `Tab` / `Shift+Tab` | everywhere | jump to a section · next · previous (from any view) |
| `j` `k` / `↑` `↓` | everywhere | move the selection: messages on SMS, bands on Bands, presets on DNS, clients elsewhere |
| `v` / `V` | everywhere | cycle view: full → gauges → lite → dense |
| `l` / `L` | everywhere | toggle lite, returning to the view you came from |
| `g` | everywhere | graph style: dots → bars → braille |
| `r` | everywhere | refresh now (`Shift+r` reads the extras on SMS/Device) |
| `[` / `]` | everywhere | refresh period ∓0.5 s, applied at once |
| `p` | everywhere except SMS | ping a host from this machine, not through the router |
| `P` | everywhere | pause / resume polling (capital P, from any section) |
| `q` / `Ctrl+C` | everywhere | quit; during a write both are refused with "a write is in progress; wait for it to settle" |

## Overlays

| Key | Where | Action |
|---|---|---|
| `S` or `,` | everywhere except Clients / Bands | settings: theme, view, graph, refresh, budget; `w` saves, `Esc` closes |
| `-` / `+` | settings | traffic budget in 10 GB steps |
| `?` | everywhere | this help |
| `i` | everywhere | event feed: `j`/`k` read back, `space` pauses, `Esc` or `i` closes |
| `Esc` | everywhere | close whatever is open: help, settings, event feed, band confirm, open message |

While the event feed is open it owns `j` `k` `space` `Esc`, so those keys stop
moving the list underneath until you close it. On Clients `s` sorts and on Bands
`s` saves the ticked merge, so `Shift+S` (or `,`) is the way into settings from
those two sections.

## Section keys

| Key | Where | Action |
|---|---|---|
| `space` | Bands | tick a band (in the event feed, the same key pauses) |
| `a` then `y` | Bands | apply the ticked merge · `y` confirms, `n` or `Esc` cancels |
| `u` | Bands | put the mask the last apply replaced back in the tick list (says so when there is nothing to undo) |
| `n` `s` `x` | Bands | next merge preset · save the ticked merge · delete it |
| `m` `o` | Bands | network mode · NR option |
| `j` `k` | SMS | move the cursor over the inbox rows; it wraps at both ends |
| `Enter` / `o` | SMS | open the message under the cursor in the reader below; `Esc` closes it, and opening never marks it read |
| `PageDown` / `PageUp` | SMS | scroll the open message by eight lines |
| `space` | SMS | mark or unmark the row under the cursor |
| `a` | SMS | mark every row on the page; pressing it again clears them |
| `n` / `p` | SMS | next / previous page of twenty; the title shows `21-40 of 45` |
| `d` then `y` | SMS | delete the marked rows (or the one under the cursor); `y` confirms, `n` or `Esc` cancels |
| `m` then `y` | SMS | mark the open message read on the device; this key only, never the open itself |
| `d` `z` | Traffic / DNS | type DNS servers · back to DHCP (`Shift+d` also resets) |
| `b` | Traffic | reset the budget baseline (asks first; the day table and the router counters stay) |
| `j` `k` `Enter` | DNS | choose a preset · apply it (then `y`) |
| `t` | everywhere | next colour theme (nine: eight palettes plus `auto`, which follows your desktop) |
| `T` | Network | speed test through this host (asks first; spends data, and the run is saved locally) |
| `s` `c` | Clients | sort by name/ip/band/time · copy the MAC |
| `e` | SMS / Device | read the extras: inbox, SIM, data switch |
| `e` | every other section | write a Markdown report to `~/.local/share/routwatch/reports/` |
| `x` `b` | Device | dump every endpoint to JSON · restart the router (type `REBOOT`) |
| `i` | settings overlay | switch the report between `redacted` (default) and `full` |

The event feed is a window over the session log (up to 200 lines, newest at the
top) and its selection is counted from the newest, so a line arriving while you
read cannot slide the text out from under you; moving pauses the feed, because
following would undo your scroll on the next poll. The header names the session
state (`● live`, `● recovering #3 8s`, `● disconnected`), `paused`, the age of
the last sample (`stale 45s`) and, while a scan runs, the endpoint being read
(`✓ plmn 3/10`); on a narrow terminal the state is the last thing dropped.

The write keys (`a` on Bands, `d` on Traffic/DNS, `b` on Device, `d` and `m` on
SMS) are the only ones that change the router; `b` on Traffic only moves the local
budget baseline, which is why it asks for a `y` but dials nothing. All of them stay
off unless the TUI is started with `--allow-write`; without it the key answers
`writes are off: start with --allow-write` and sends nothing. The three rules that
guard them are in the README's Safety list; what can go wrong during a write is in
[limits.md](limits.md).

Back: [README](../README.md) · [all docs](README.md)
