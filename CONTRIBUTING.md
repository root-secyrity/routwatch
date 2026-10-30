# Contributing to routwatch

Thanks for taking a look. routwatch talks to one family of routers (Huawei
LTE/5G CPE) over their web API, so the bar for a change is simple: does it work
against real hardware, and does it fail politely when it doesn't.

## If you own a cellular router nobody has tested

You do not need to write code, and you do not need to send anything that
identifies you. Two commands, both safe to paste into a public issue:

```sh
routwatch share      # one file: which endpoints answer, the field names they
                     # use, and a snapshot with serial, IMEI, MACs, WAN IP and
                     # client names already replaced
```

If you would rather look at the pieces yourself, the same information comes from
`routwatch doctor --json` (capability map) and `routwatch snapshot --json --redact`
(parsed values). For a vendor we have never seen, `routwatch dump --redact` adds
the raw XML with every tag name kept and the identity values replaced.

Open a **model report** with both. That is enough to write a parser and a mock
profile for your model: the report becomes a `Profile` in `src/mock_cpe.rs` plus
a real test, so your router ends up covered by CI even though no build machine
can reach it. Then check the next release against your hardware and say what is
still wrong - that second report is worth as much as the first.

Please do not attach screenshots of the interface, or `snapshot --raw` output:
those carry IMEI, serial, SSID and WAN IP in the clear, and this repository must
never hold them. `snapshot --raw --redact` is refused by design - the flag cannot
protect a raw dump - so use `dump --redact` when you need the raw shapes.

## Before you open a pull request

- Open an issue first if the change touches a promise in `docs/contracts.md`,
  adds or changes a router endpoint, or moves something in the UI. Those are
  the things other changes are built on.
- Typos, docs and small fixes can go straight to a PR.

## Build and test locally

The minimum supported Rust version is **1.88** (`rust-version` in `Cargo.toml`).
Distro toolchains on Debian 12 and Ubuntu 22.04 are older than that, which is
why CI installs rustup inside containers.

```sh
make build   # cargo build --release --locked
make test    # cargo test --locked
make lint    # cargo clippy --all-targets --locked -- -D warnings
make fmt     # cargo fmt --all --check
```

A change is ready when all four are clean. CI runs exactly these, plus a release
build that must stay under 10 MiB, plus the same source in five containers
(debian 12, ubuntu 22.04, ubuntu 24.04, fedora 41, alpine 3.20) to catch a libc
or toolchain surprise early.

## What CI cannot check

- **No real device data in any tracked file** - no SSID, MAC, IMEI, serial or WAN
  IP, not in code, not in a comment, not in a commit message. The snapshots in
  `fixtures/` are synthetic on purpose; use them instead of your own router.
- **`DOCK/` and `tests/` are local-only** and not tracked. If your change needs a
  capture from your own router, keep it there and describe in the PR what it
  showed.
- **The test count in `README.md` is a promise** - if you add or remove a test,
  update the number in the same PR.
- **`docs/contracts.md` is machine-checked.** Adding a contract means adding the
  guard test that names it; renaming one breaks the build by design.

## Style

- rustfmt defaults, no exceptions - there is no `rustfmt.toml`.
- No new dependency for something the standard library already does. A new crate
  needs its reason in the PR body.
- Errors are for humans: say what failed, on which path, and what still works.
- The UI must degrade, not break: no panic on a missing field, an empty list, or
  a 40-column terminal.
- No em dash (U+2014) in prose, comments or commit messages: use a comma, a colon
  or a single hyphen. CI checks for it.

## Commits and branches

Conventional commits with a lowercase scope: `fix(doctor): ...`, `feat(cli): ...`,
`docs(install): ...`. One logical change per PR, and no unrelated reformatting
mixed into a fix.

## Reporting a bug

Use the issue template. `routwatch doctor --json` is the single most useful thing
to attach: it names every capability as ready/degraded/failed with the reason, and
it never prints a password, serial, IMEI or SSID.

## Security

Please do not open a public issue for a vulnerability - see `SECURITY.md`.
