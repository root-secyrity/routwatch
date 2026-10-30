<!--
Thanks for the PR. Keep it to one logical change; the checklist below is what a
reviewer would ask about anyway.
-->

## What this changes

<!-- one paragraph. Link the issue if there is one. -->

## Why

<!-- the symptom, or the promise in docs/contracts.md this restores -->

## Checklist

- [ ] `make fmt` clean
- [ ] `make lint` clean (clippy, `-D warnings`)
- [ ] `make test` passes, and the test count in `README.md` still matches
- [ ] release build still under 10 MiB (`make build`)
- [ ] no real device data in any tracked file or commit message (no SSID, MAC, IMEI, serial, WAN IP)
- [ ] if a promise in `docs/contracts.md` changed, the guard test that names it changed too
- [ ] a user-visible change is described in `README.md` or `docs/` in this same PR

## Tested against

<!-- router model + firmware, or "the synthetic snapshot in fixtures/" if no hardware was involved -->

## What still does not work

<!-- a known gap written here is better than one found later -->
