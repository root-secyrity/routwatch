# Credits and prior art

This file answers: whose work is this built on, and what exactly was reused?

routwatch is the fourth member of the family that already had
[netwatch](https://github.com/matthart1983/netwatch) (real-time network
diagnostics), [syswatch](https://github.com/matthart1983/syswatch) (system, twelve
tabs) and [diskwatch](https://github.com/matthart1983/diskwatch) (disk, eight
tabs) - all three by [Matt Hartley (matthart1983)](https://github.com/matthart1983),
all three MIT, and the explicit inspiration here: the
`~/.config/<tool>/config.toml` shape, `theme = "terminal"`, the `V`/`L` view
cycle, `--tab`/`--graph`, the `doctor --json` capability schema and the habit of
stating a limitation instead of showing a plausible zero. The eight palettes in
`src/theme.rs` are derived byte-for-byte from netwatch's theme module (slot names
are ours, the colour values are not), so a theme looks identical whichever of the
four tools you open.

The feature set and the router knowledge come from **HManagerPy 2.1.0**, the
Electron application this replaces, written by RoOt-SeC. See [`../NOTICE`](../NOTICE) for
the complete attribution and the reproduced MIT text.

Back: [README](../README.md) · [all docs](README.md)
