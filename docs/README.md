# routwatch documentation

`README.md` in the repository root is the short version: what routwatch is, one
install command, and the limits that matter before you start. Everything longer
lives here, one file per question, so you can read only the part you need.

## Start here

| If you want to | Read |
|---|---|
| install it, and write the config file | [`install.md`](install.md) |
| know whether your router or distribution is covered | [`platforms.md`](platforms.md) |
| look up a key while the TUI is open | [`keys.md`](keys.md) |
| change bands, or save a band combination | [`bands.md`](bands.md) |
| put it in Waybar, or script against `--json` | [`headless.md`](headless.md) |
| change the colours | [`themes.md`](themes.md) |
| know what does not work, and why | [`limits.md`](limits.md) |
| know who wrote what | [`credits.md`](credits.md) |
| contribute, or read the promises a change must keep | [`../CONTRIBUTING.md`](../CONTRIBUTING.md), [`contracts.md`](contracts.md) |

## Other material

| Path | What it is |
|---|---|
| [`contracts.md`](contracts.md) | Behaviours the tool promises, each with the test that fails when one breaks. Machine-checked by `src/contracts.rs`. |
| [`diagrams/`](diagrams) | Architecture and sequence maps in English and Arabic, as interactive HTML plus PNG stills, generated from the source at a named commit. |
| [`screenshots/`](screenshots) | Nine frames rendered from `fixtures/sample-snapshot*.json` with `preview --ansi`, so every name, MAC, IMEI and WAN address in them is a placeholder. |
| [`../NOTICE`](../NOTICE) | Third-party attribution and the reproduced MIT text, including HManagerPy, the Electron application routwatch replaces. |

## Conventions in this folder

Each file starts by naming the question it answers, states what was measured
versus what is assumed, and links back to the README. No real device data
appears anywhere here: the fixtures are synthetic on purpose, and the
screenshots come from those fixtures.

Back: [README](../README.md)
