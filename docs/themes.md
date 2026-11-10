# Themes

This file answers: what palettes ship, and how does `auto` decide?

Eight, identical to the rest of the family and byte-for-byte the same palettes:
`dark`, `terminal` (default - follows your terminal palette), `ocean`,
`solarized`, `dracula`, `nord`, `sky`, `paper` (the last two paint their own panel
background). Aliases: `system`/`ansi` → `terminal`, `light`/`minimal` → `paper`.
Pick one with `--theme <name>` or cycle with `t` in the settings overlay.

`auto` is not a ninth palette: it follows whatever the running desktop publishes
and falls back to `terminal` when there is nothing to read. The ladder is
`$ROUTWATCH_THEME_FILE` → the running Ryoku desktop's `theme.name` +
`scheme.json` → an Omarchy `colors.toml` → `terminal`. The desktop that is
actually running wins, because a Ryoku box also carries an Omarchy-compatible copy
that can be stale. Every derived palette is checked for 4.5:1 body contrast.

```sh
ROUTWATCH_THEME_FILE=~/mine.toml routwatch --theme auto
routwatch preview --tab bands --size 120x30 --ansi
```

A theme that sets no background of its own leaves the graph ramp on a stated dark
assumption; the graph-colour bullet of [limits.md](limits.md) explains the floor
before you file a contrast bug on a light terminal.

Back: [README](../README.md) · [all docs](README.md)
