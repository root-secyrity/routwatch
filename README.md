<p align="center">
  <h1 align="center">routwatch</h1>
  <p align="center"><strong>Huawei LTE/5G CPE manager in the terminal.</strong></p>
  <p align="center">
    <img src="https://img.shields.io/badge/platform-Linux-blue" alt="Platform">
    <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
    <img src="https://img.shields.io/badge/rust-1.88%2B-orange" alt="Rust">
  </p>
</p>

routwatch is the fourth member of the `syswatch` / `netwatch` / `diskwatch`
family: same config file shape, same themes, same keys. It talks to a Huawei
CPE (tested on the H122-373, WEBUI 11.0.2.21) over its own HTTP API and gives
you signal, traffic, clients, bands, DNS and device state in one screen — plus
a headless mode for Waybar and scripts.

```
 routwatch · Test Net · 00101 · LTE · ENDC on                                        live 0s · poll 0ms
 1 Dashboard │ 2 Bands │ 3 Traffic │ 4 Network │ 5 Clients │ 6 SMS │ 7 Device │ 8 DNS │ 9 Signal    full
┌ SIGNAL · 4G ─────────────────────────────────────────────┐┌ SPEED ───────────────────────────────────┐
│RSRP      -95 dBm                                         ││↓ 7.59 Mbps                               │
│RSRQ      -4 dB                                           ││↑ 87.1 Kbps                               │
│SINR      3 dB                                            ││                                          │
│RSSI      -71 dBm                                         ││session   35.6 MB ↓ / 3.5 MB ↑ (1m)       │
│band      B1  2120/1930 MHz  (20MHz/20MHz)                ││total     49.99 TB ↓ / 1.69 TB ↑ (1011d)  │
│cell      PCI 481 · EARFCN DL:100 UL:18100 · TM[4]        ││                                          │
└──────────────────────────────────────────────────────────┘└──────────────────────────────────────────┘
┌ 5G ──────────────────────────────────────────────┐┌ CONNECTION ──────────────────────────────────────┐
│RSRP      -115 dBm                                ││network   Test Net (00101)                        │
│SINR      4 dB                                    ││status    connected · LTE · ENDC on               │
│band      N41                                     ││wan       203.0.113.43                            │
└──────────────────────────────────────────────────┘└──────────────────────────────────────────────────┘
┌ THROUGHPUT · dots · last 120 samples ────────────────────────────────────────────────────────────────┐
│down 7.59 Mbps                                                                                        │
│⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸      │
│⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸      │
│up   87.1 Kbps                                                                                        │
│⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢸      │
└──────────────────────────────────────────────────────────────────────────────────────────────────────┘
┌ CLIENTS · 3 ───────────────────────────────────────────────┐┌ ERRORS ────────────────────────────────┐
│name                       ip              band             ││no errors this session                  │
└────────────────────────────────────────────────────────────┘└────────────────────────────────────────┘
 live · 192.168.8.1 · refresh 2000ms · v view · g graph · r refresh · p pause · q quit · ? help
```

## Bands, with merge presets

Select several bands (`space`), save the combination as a preset (`s`), and
cycle the ready-made ones with `n`. Nothing is sent until `a` and `y`.

```
│[B1] [B3] [B5]  B7  [B8] [B20] [B28] [B32]  B34   B38   B39   B40   B41   B42   B43                   │
│presets   B1+B3   B3+B20   B1+B3+B8   B1+B3+B20+B28   B1+B3+B5+B8+B20+B28                             │
│selected 7 bands · B1 + B3 + B5 + B8 + B20 + B28 + B32 → mask 0x88080095                              │
```

Presets live in `~/.config/routwatch/config.toml` (`[[app.presets]]`). No
`presets` key means the five built-ins; `presets = []` means you deleted them
and mean it.

## Install

**Arch / CachyOS / Manjaro (VCS package):**

```sh
git clone https://github.com/root-secyrity/routwatch.git
cd routwatch/packaging
makepkg -si
```

**Debian / Ubuntu / Fedora / anything with a toolchain** — Rust 1.88+ is
required, and the distributions ship older compilers (Debian 12 has 1.63,
Ubuntu 22.04 has 1.66, Fedora 41 has 1.82), so install rustup first:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
git clone https://github.com/root-secyrity/routwatch.git && cd routwatch
sudo make install              # or: make install PREFIX=$HOME/.local
```

**Any distribution, no build, no glibc worry** — a static musl build links
nothing at all and runs on Arch, Debian, Ubuntu, Fedora, Alpine, Raspberry Pi
OS:

```sh
./packaging/musl-build.sh      # needs docker; writes dist/routwatch-x86_64-linux-musl
```

or grab the binary attached to the [latest release](https://github.com/root-secyrity/routwatch/releases).

## Supported Linux

| Distribution | Verified | Notes |
|---|---|---|
| Arch / CachyOS / Manjaro | built, packaged, installed and run on this machine | `packaging/PKGBUILD` |
| Debian 12 (bookworm) | built in a clean container; binary runs and renders | needs no symbol newer than `GLIBC_2.34`, no loader warning |
| Ubuntu 22.04 | built in a clean container; binary runs and renders | same result as Debian 12 (`GLIBC_2.34`, no warning) |
| Fedora 41 | built in a clean container; binary runs | same source, no patches |
| Alpine 3.20 (musl) | built in a clean container; **static** binary runs on Arch | `packaging/musl-build.sh` |
| Ubuntu 24.04 | covered by the CI matrix (`.github/workflows/ci.yml`) | |

The router side is plain HTTP, so nothing distribution-specific is compiled in.
What *does* differ per distribution is the optional helpers:

| Helper | Package it comes from | Used for |
|---|---|---|
| `ping` | `iputils-ping` (Debian/Ubuntu) · `iputils` (Arch/Fedora) · busybox (Alpine) | Network tab ping, `routwatch ping` |
| `wl-copy` / `xclip` / `xsel` | `wl-clipboard` · `xclip` · `xsel` | copying a client MAC (first one found wins) |
| `secret-tool` | `libsecret-tools` (Debian/Ubuntu) · `libsecret` (Arch) | password keyring (falls back to env/config) |

Missing helpers never break routwatch — `routwatch doctor --json` reports a
`helpers` capability that says which ones were found, and what to install.

**glibc floor.** A binary built against a distribution's glibc only runs on that
glibc or newer. Built on Debian 12 / Ubuntu 22.04 the result needs nothing newer
than `GLIBC_2.34`; built on a newer host (Arch) it picks up a *weak* `GLIBC_2.39`
reference, which older loaders warn about once and then tolerate. For a file
that runs *everywhere*, use the musl build above; for a distribution package,
build on the distribution — that is what the PKGBUILD and the CI matrix do.

## Configure

```sh
routwatch --generate-config    # writes ~/.config/routwatch/config.toml at 0600
```

```toml
[router]
ip = "192.168.8.1"
username = "admin"
# password is optional here — prefer ROUTWATCH_PASSWORD or secret-tool
password = ""

[app]
refresh_ms = 2000
timeout_ms = 3000
theme = "terminal"      # follows your terminal / desktop theme
view = "full"           # full · gauges · lite · dense
graph_style = "dots"    # dots · bars · braille
budget_gb = 100         # local budget only; this firmware reports no monthly quota
```

Password ladder: `--password-stdin` → `ROUTWATCH_PASSWORD` → `secret-tool` →
`config.toml`. The file is refused unless it is `0600`.

## Headless

```sh
routwatch status --waybar
# {"text":"LTE ▄ -96dBm","tooltip":"Test Net (00101)\nLTE · ENDC off · 5 bars\n…","class":"fair","percentage":46}

routwatch status --json
routwatch snapshot --json
routwatch doctor --json          # capabilities with state/reason/detail/next_check
                                 # (config, credentials, theme, reachability, endpoints)
routwatch dump --json            # every raw endpoint → routwatch-dump.json in $PWD
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

Exit codes: `0` ok · `2` network/unreachable (the Waybar document still prints,
`class: "offline"`) · `3` credentials or config.

## Keys

| Key | Where | Action |
|---|---|---|
| `1`–`9` | everywhere | jump to a section (works from any view) |
| `Tab` / `Shift+Tab` | everywhere | next / previous section |
| `v` `V` | everywhere | cycle view: full → gauges → lite → dense |
| `l` `L` | everywhere | toggle lite, returning to the view you came from |
| `g` `r` `p` | everywhere | graph style · refresh now · pause polling |
| `,` or `s` | everywhere | settings overlay (theme, view, graph, refresh, budget); `w` saves |
| `?` | everywhere | key help |
| `space` `j` `k` | Bands | tick a band · move |
| `n` `s` `x` | Bands | next merge preset · save the ticked merge · delete preset |
| `m` `o` `a` | Bands | network mode · NR option · apply (confirm with `y`) |
| `d` `z` | Traffic/DNS | type DNS servers · back to DHCP |
| `j` `k` `Enter` | DNS | choose a preset · apply (confirm with `y`) |
| `t` | Network | ping a host from this machine |
| `s` `c` | Clients | sort (name/ip/band/time) · copy MAC |
| `e` / `R` | SMS/Device | read extras (inbox, SIM, data switch) |
| `x` `b` | Device | dump all endpoints to JSON · restart router (type `REBOOT`) |

Every section also lists its own keys in the status bar.

## Themes

Eight, identical to the rest of the family and byte-for-byte the same palettes:
`dark`, `terminal` (default — follows your terminal palette), `ocean`,
`solarized`, `dracula`, `nord`, `sky`, `paper` (the last two paint their own
panel background). Aliases: `system`/`ansi` → `terminal`, `light`/`minimal` →
`paper`. Pick one with `--theme <name>` or cycle with `t` inside the settings
overlay.

`preview` renders a frame without a TTY; `--ansi` keeps the colours:

```sh
routwatch preview --tab bands --size 120x30 --ansi
routwatch preview --tab dashboard --fixture fixtures/sample-snapshot-5g.json
```

## Safety

- Reads are free; **writes are confirmed** and verified by reading the router
  back (`mask verified`), so a silent revert is visible.
- Band/DNS/restart changes need the in-TUI `a` → `y` confirmation, or
  `--allow-write` on the CLI. `--dry-run` prints the exact payload and sends
  nothing.
- Band changes drop the link for 10–30 seconds. The UI says so before you do it.

## Limits (stated, not hidden)

- **SMS** is blocked by the firmware (`100005`); the tab shows counters and
  says so instead of pretending.
- **5G per-field metrics** (`nrrsrp`, `nrsinr`, `nrdlmcs`) are empty on some
  firmware even with ENDC active. Only `SignalIconNr` (0–5 bars) and the ENDC
  flag are reliable there; the UI switches to a bars panel instead of faking
  dBm.
- **No monthly quota**: this firmware answers `100003`. The budget bar is local.

## Development

```sh
cargo test                 # 106 unit tests, no router and no fixtures on disk
cargo clippy --all-targets # zero warnings
cargo fmt --check
```

Every measurement in this README (payload shapes, band masks, what the firmware
refuses) was taken from a real H122-373; nothing here is guessed. The sample
frames come from `fixtures/sample-snapshot*.json`, which is synthetic — the
device names, MACs, WAN address and identifiers are placeholders.

## Credits and prior art

routwatch is the fourth member of the family that already had **netwatch**,
**syswatch** and **diskwatch** — all three by [Matt Hartley
(matthart1983)](https://github.com/matthart1983), all three MIT. They are the
explicit inspiration for this tool: the `~/.config/<tool>/config.toml` shape,
`theme = "terminal"`, the `V`/`L` view cycle, `--tab`/`--graph`, the
`doctor --json` capability schema and the habit of stating a limitation instead
of showing a plausible zero all come from them.

- [netwatch](https://github.com/matthart1983/netwatch) — real-time network diagnostics
- [syswatch](https://github.com/matthart1983/syswatch) — system diagnostics, twelve tabs
- [diskwatch](https://github.com/matthart1983/diskwatch) — disk diagnostics, eight tabs

The eight palettes in `src/theme.rs` are **derived byte-for-byte from
netwatch's** theme module (slot names are ours, the colour values are not), so
a theme looks identical whichever of the four tools you open. That derivation
is covered by netwatch's MIT licence and attributed in full.

The feature set and the router knowledge behind it come from **HManagerPy 2.1.0**,
the Electron application this replaces, written by RoOt-SeC.

See [NOTICE](NOTICE) for the complete attribution and the reproduced MIT text.

## License

MIT — see [LICENSE](LICENSE) and [NOTICE](NOTICE).
