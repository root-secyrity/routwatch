# Keys

This file answers: what does each key do, and where does it work?

| Key | Where | Action |
|---|---|---|
| `1`–`9` | everywhere | jump to a section (works from any view) |
| `Tab` / `Shift+Tab` | everywhere | next / previous section |
| `v` `V` | everywhere | cycle view: full → gauges → lite → dense |
| `l` `L` | everywhere | toggle lite, returning to the view you came from |
| `g` `r` | everywhere | graph style · refresh now |
| `P` | everywhere | pause / resume polling |
| `,` or `s` | everywhere | settings overlay (theme, view, graph, refresh, budget); `w` saves |
| `i` | everywhere | event feed: `j`/`k` select, `space` pause/resume, `Esc` or `i` closes |
| `?` | everywhere | key help |
| `space` `j` `k` | Bands | tick a band · move (inside the event feed they belong to the feed) |
| `n` `s` `x` | Bands | next merge preset · save the ticked merge · delete preset |
| `m` `o` `a` | Bands | network mode · NR option · apply (confirm with `y`) |
| `d` `z` | Traffic/DNS | type DNS servers · back to DHCP |
| `j` `k` `Enter` | DNS | choose a preset · apply (confirm with `y`) |
| `p` | everywhere | ping a host from this machine (the result shows on Network) |
| `t` | everywhere | next colour theme (nine of them); `w` in settings saves it |
| `s` `c` | Clients | sort (name/ip/band/time) · copy MAC |
| `e` / `R` | SMS/Device | read extras (inbox, SIM, data switch) |
| `x` `b` | Device | dump all endpoints to JSON · restart router (type `REBOOT`) |
| `e` | every section except SMS/Device | write a Markdown report to `~/.local/share/routwatch/reports/` |
| `i` | settings overlay (`,`) | switch the report between `redacted` (default) and `full` |

Every section lists its own keys in the status bar, and `e` stays "read extras" on
SMS and Device, where the inbox is what you want. The event feed is a window over
the session log (up to 200 lines, newest at the top) and its selection is counted
from the newest, so a line arriving while you read cannot slide the text out from
under you; moving pauses the feed, because following would undo your scroll on the
next poll. The header names the session state (`● live`, `● recovering #3 8s`,
`● disconnected`), `paused`, the age of the last sample (`stale 45s`) and, while a
scan runs, the endpoint being read (`✓ plmn 3/10`); on a narrow terminal the state
is the last thing dropped.

The write keys (`m` `o` `a` on Bands, `d` on Traffic/DNS, `b` on Device) are the
only ones that change the router. The three rules that guard them are in the
README's Safety list; what can go wrong during a write is in
[limits.md](limits.md).

Back: [README](../README.md) · [all docs](README.md)
