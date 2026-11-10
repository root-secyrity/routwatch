# Platforms and routers

This file answers: does routwatch run on my distribution, and has anyone tested
it against my router?

## Tested on

The router side is what varies: routwatch speaks an undocumented HTTP API, so the
firmware decides whether a panel fills, not the distribution.

| Router | WebUI / firmware | What was verified |
|---|---|---|
| Huawei H122-373 | 11.0.2.21 | every section, the bands and DNS write path with its read-back, the headless commands, the report export |
| any other Huawei CPE | - | **untested** |

An untested model is not a refusal. The read path is generic: every endpoint is
probed, and a firmware that does not implement one answers with its own error
code, so the panel that reads it says what is missing instead of guessing. The
write path is the part most likely to differ per firmware - it prints both sides
before it sends anything and reads the router back afterwards.

If you run it on another model, open a
[model report](https://github.com/root-secyrity/routwatch/issues/new?template=model_report.yml)
with the output of `routwatch doctor --json` - it names each capability as
`ready` / `degraded` / `failed` and carries the firmware's own reason for each
failure. [`../CONTRIBUTING.md`](../CONTRIBUTING.md) explains what happens to that
report: it becomes a mock profile plus a test, so your router is covered by CI
even though no build machine can reach it.

## Supported Linux

| Distribution | Verified | Notes |
|---|---|---|
| Arch / CachyOS / Manjaro | built, packaged, installed and run on this machine | `packaging/PKGBUILD` |
| Debian 12 (bookworm) | built in a clean container; binary runs and renders | needs no symbol newer than `GLIBC_2.34`, no loader warning |
| Ubuntu 22.04 | built in a clean container; binary runs and renders | same result as Debian 12 |
| Fedora 41 | built in a clean container; binary runs | same source, no patches |
| Alpine 3.20 (musl) | built in a clean container; **static** binary runs on Arch | `packaging/musl-build.sh` |
| Ubuntu 24.04 | covered by the CI matrix (`.github/workflows/ci.yml`) | |

Optional helpers differ per distribution, and a missing one never breaks
routwatch - `routwatch doctor --json` reports a `helpers` capability instead:

| Helper | Package | Used for |
|---|---|---|
| `ping` | `iputils-ping` (Debian/Ubuntu) · `iputils` (Arch/Fedora) · busybox (Alpine) | Network tab ping, `routwatch ping` |
| `wl-copy` / `xclip` / `xsel` | `wl-clipboard` · `xclip` · `xsel` | copying a client MAC (first one found wins) |
| `secret-tool` | `libsecret-tools` (Debian/Ubuntu) · `libsecret` (Arch) | password keyring (falls back to env/config) |

## Windows

No native `routwatch.exe` is shipped. Inside WSL2 the same static binary runs
unmodified:

```sh
# once, in PowerShell, then reboot
wsl --install -d Ubuntu

# inside Ubuntu, the permanent link: it always points at the newest binary
curl -LO https://github.com/root-secyrity/routwatch/releases/latest/download/routwatch-x86_64-linux-musl
sudo install -m 755 routwatch-x86_64-linux-musl /usr/local/bin/routwatch
routwatch doctor --json
```

Two things break on Windows, both silent until you hit them:

- Keep `~/.config/routwatch/config.toml` inside the WSL filesystem, not under
  `/mnt/c`. DrvFs mounts files `0777`, and routwatch refuses any config the group
  can read (`is mode 777; run chmod 600`), which `chmod` cannot fix there without a
  `metadata` mount option.
- Run it in Windows Terminal. The legacy console host has no truecolor, so the
  theme and the colour-coded panels degrade.

Back: [README](../README.md) · [all docs](README.md)
