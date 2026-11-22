# Limits (stated, not hidden)

This file answers: what is known not to work, what looks broken but is not, and
why each one is stated instead of hidden.

- **The dashboard window is fixed at 240 seconds**: every column the width
  leaves becomes a cell, so the cell span adapts instead (3.4s at 80 columns,
  3s at 120, 1.5s at 200), the window never does, and the ruler prints `-240s`
  even when the record holds ten seconds. A short record sits at the right edge
  instead of being stretched over the plot, and the legend prints the span the
  columns actually have.
- **A cell is its worst sample, not its average**: RSRP, RSRQ and SINR take the
  lowest reading in the cell, throughput takes the peak. Averaging hides the
  dip the map exists to show.
- **A blank cell means no sample**: it is not coloured good, not filled from a
  neighbour and not interpolated.
- **The store holds 240 seconds**: five series of 480 samples (19.2 KB in
  memory), sized for the 500ms poll floor. A slower poll keeps more seconds than
  the window shows; a faster one cannot, and the cell widens.
- **The map statistics column is the first thing a narrow terminal loses**: the
  three metric rows stay, then RSRQ, then SINR, and the ruler stays last. The
  insight line (`worst ... · SINR < 0 for 41s · 3 dips`) needs ten filled cells
  and is dropped, not guessed, below that.
- **The client table has no RSSI or rate column**: this firmware's client list
  carries name, IP, MAC, frequency, SSID and connection time, and the model
  stores exactly those. A column the router never reports is removed, not
  printed as `-`.
- **An Arabic client name may reorder inside its column**: cells are laid out in
  logical order and the terminal handles direction, the same limit as the SMS
  list. The Clients tab detail shows the full name.
- **`s` sorts on the Dashboard**: on every other tab `s` opens the settings
  panel, and this is the one key that differs by tab by design.
- **SMS**: reading the inbox works on H122-373 when the request carries
  `Ascending` (it did not before). A firmware that still answers `100005` is
  reporting a format error; the tab says the request was rejected, not that SMS
  is blocked.
- **Deleting and marking read are verified on H122-373**: both paths were used on
  the device (2026-11-19; the inbox went from 8 messages to 3, and a delete is
  accepted only when the re-read no longer lists the index). A firmware that
  refuses the path answers `100003` or `100002`; that is reported as
  "unsupported" with its code, never as "deleted", and the keys stay visible so
  the answer is readable.
- **A bulk delete is one request per index and can fail halfway**: each index is
  posted on its own, the first refusal that means "this firmware does not take it"
  stops the loop, and the rows it never reached are listed as "not attempted".
  The verdict comes from re-reading the inbox, not from the POST answer, so the
  summary reads `deleted 5 of 7 · 2 failed (index 12: unsupported (100003) ·
  index 19: still present)` and the command exits non-zero. The lines never reach
  the device in one request, so a dropped link leaves the rest untouched.
- **Opening a message writes nothing**: the reader is client-side, and it does
  not send a read receipt. Marking read is a separate key (`m`) behind the same
  write gate, then verified by re-reading `Smstat`.
- **`BoxType` beyond the inbox is unverified**: `--folder inbox` sends `BoxType
  1`, which the H122-373 accepts; outbox (`2`) and draft (`3`) follow Huawei's
  numbering and were not measured, so the CLI says so instead of implying three
  tested folders.
- **The TUI cannot reorder Arabic**: cells are laid out in logical order, so a
  right-to-left body inside the panel can appear reversed on a terminal without
  bidi. `routwatch sms read <index>` prints the body untouched, and the terminal
  handles the direction.
- **`100003` carries two meanings on the H122-373**: Huawei's table names it
  `ERROR_NO_RIGHT`, and this firmware also answers it for paths it does not serve
  (measured 2026-11-10: 45 candidate NR paths, every one of them 100003, in a
  logged-in session). routwatch keeps the state unsupported and prints "the
  firmware refused this request (100003 ERROR_NO_RIGHT)" instead of naming a
  cause; telling a session refusal from a missing path needs a measurement the two
  do not share.
- **`100002` is not a busy router**: Huawei's table names it `ERROR_NOT_SUPPORT`
  and it is reported as unsupported. `100004` (`ERROR_BUSY`) is the one reported
  as busy, unverified on this firmware: no probed path has answered it.
- **Band writes can answer `100006` after they land**: on H122-373 the radio
  accepts the payload and the POST still returns an undefined code. The verdict
  comes from reading `net/net-mode` back, never from the code; routwatch waits
  one grace period and then says what the device reports. A `100006` with no
  change behind it is reported as a failure.
- **5G per-field metrics can be empty**: on some firmware the `nrrsrp`, `nrrsrq`
  and `nrsinr` readings stay blank even with ENDC active, while `SignalIconNr`
  (0–5 bars) and the ENDC flag keep arriving. The signal panel then shows the bars
  and says which `nr` fields did arrive (`nrearfcn`, bandwidths, rank, BLER, CQI,
  TX power) rather than inventing dBm; it switches to the full gauges the moment
  any one of `nrrsrp`, `nrrsrq` or `nrsinr` carries a value.
- **The budget counts from a baseline, and says where its number came from**: the
  bar compares your `budget_gb` against traffic measured since the baseline
  (`b` moves it, and it moves itself at the start of each billing cycle read from
  `monitoring/start_date`). When the firmware answers
  `monitoring/month_statistics`, the number is the router's own month counter;
  otherwise it is the lifetime counters, so it survives a reboot but not a
  counter clear. The per-day ledger behind the usage table only counts while
  routwatch runs, gaps over five minutes are marked rather than spread, and the
  panel prints how many days it covers.
- **Reports are redacted unless you ask**: `e` writes `0600` Markdown in a `0700`
  directory, with IMEI, serial, WAN IP, MACs and SSIDs hidden - these files get
  pasted in public. `report_identity`, `--report-identity` or `i` in the settings
  overlay writes the full version, and a report never overwrites another.
  The month counters are traffic totals, not identities, so they survive
  redaction on purpose.
- **`auto` theme is best-effort**: it reads the running desktop's published colours
  and falls back to `terminal`. The default is still `terminal`, so no machine
  changes appearance unasked.
- **Instrument band**: the dashboard shows RSRP, SINR, clients and a signal map on
  rows 3–9. `UPTIME` and `TEMP` appear only when the router reports them, and the
  band re-flows to four, five or six equal cells instead of leaving a hole. It
  needs 27 body rows; below that the dashboard blocks keep the space.
- **Small terminals**: the frame degrades in a fixed order instead of clipping. The
  tab bar disappears entirely below 12 rows (measured: 100x11 has none), and
  becomes `1/9 Dash · Tab next` below 76 columns; below 60
  columns the status line keeps one fact plus the keys, below 40 only the keys; the
  dashboard drops from four blocks to signal + speed + history (under 25 rows) and
  to one signal panel at the 24x6 floor, where `too small` names the size it needs.
- **A retired graph style loads as braille**: `dots` and `bars` drew braille
  cells with a colour ramp, and the ramp assumed a dark terminal on the six themes
  whose background is `Reset`. Both names now resolve to braille, the series is
  one flat colour, and the file or script that named one is told once
  (`graph dots retired; braille`) instead of failing. A value this build
  never drew is refused with the two it takes, braille or ascii.
- **One renderer draws every plot**: `ui::chart` owns the braille area, the
  mirrored pair and the axis. `ui::widgets` no longer carries `spark` or
  `spark_scaled`; a panel that wants a trend calls `chart::spark_auto` (auto
  ceiling) or `chart::spark_lines_scaled` (a fixed, possibly negative domain such
  as RSRP).
- **A sample is one cell wide**: the mirrored plot is right aligned at one
  braille cell per sample and never stretched, so the bars keep their width as
  the session fills. A history longer than the panel is bucketed per cell with
  the maximum of that bucket, so a spike never widens a bar. A panel too narrow
  for three ticks prints only the window start and `now`.
- **A trend fills the rows it is given**: `chart::spark_lines` draws a braille
  plot of whatever height the caller passes, so a three-row slot in the speed
  panel uses three rows instead of clamping to one and leaving two empty.
- **The DNS write rewrites the whole DHCP document**: `/api/dhcp/settings` is a
  whole-document endpoint, so routwatch reads it immediately before writing and
  echoes every field back, changing only `PrimaryDns` and `SecondaryDns`. A
  setting changed from the web UI in the same second is overwritten. `dns --reset`
  writes the router's own LAN address into both slots, which is how the box
  reports itself at rest; the panel calls that state `router as resolver`, not
  "automatic", because the router answering DNS is what `DnsStatus` means here.

- **A firmware without the DHCP DNS row is refused, never attempted**: the live
  H122-373 answers `dhcpsdns_display=0` on `/api/dhcp/feature-switch` and silently
  drops `DnsStatus`, `PrimaryDns` and `SecondaryDns` from an otherwise accepted
  document, so a write there can only ever end as a mismatch. routwatch reads that
  flag before writing and stops with "nothing was sent"; the snapshot carries
  `editable: false`, so the DNS tab, the dashboard panel and the confirm modal say
  the setting is absent instead of offering it. A router that does not answer the
  endpoint at all is still attempted: a missing flag is not a no.
- **No router-side throughput endpoint**: the H122-373 (HUAWEI 5G CPE Pro 2) WebUI
  `6.0`, whose `/js` bundles are dated 2024-04-02, carries no speed or bandwidth
  test (checked 2026-11-15: the four bundles mention only Wi-Fi channel width and
  VOIP speed dial, and `/api/device/information` is still `100003` from a bridge
  without a session). The speed test is host-side by necessity: it measures the
  path through this machine, never the WAN link on its own.

- **The privacy audit is read-only and says so where it cannot answer**: `privacy`
  issues `GET` calls only (`the_audit_never_posts` holds it) and prints the code a
  refusal carried instead of reading silence as "off". TR-069 is not exposed on
  the H122-373 (WEBUI 11.0.2.21, measured 2026-11-17: `ntwk/tr069`,
  `device/tr069` and `security/tr069` all answered 100003), so the report declares
  that and the DNS check names both sources, because the server DHCP advertises
  and the server the link reports are different facts. Whether the web UI needs a
  password over WLAN is not a field this firmware exposes: the check prints
  `unknown` rather than a pass.

- **A privacy drift needs two reads, and a baseline you saved**: `privacy --diff`
  compares the live audit with `$XDG_DATA_HOME/routwatch/privacy-baseline.json`
  (0600, written by `privacy --save`). A check counts as drifted only when two
  reads agree on the new value: one read of a document this firmware answers from
  a flaky tail is a rumour, not evidence. A refusal is never a change, and
  `unknown` is never folded into `off`; a check that is missing from a read says
  nothing. `--diff` exits 1 when it reports a drift, so a script can act on it.

- **An incident is a window, never a cause**: `incidents` derives windows from
  the observation log, the signal log and the speed history, all read from
  `$XDG_DATA_HOME/routwatch/`. It reads no clock and writes nothing, so the same
  logs always give the same list. Confidence is the count of rows that agree
  (1 low, 2 to 3 medium, 4 or more high), not a probability.

- **The ordering limit is derived from the samples around the incident**: two
  events can be ordered only if the signal log brackets both of them, and only if
  the widest gap between those samples is narrower than the distance between the
  events. The incident prints that gap as the smear it used, so the number moves
  when the cadence moves: at the radio tier's measured cadence it lands near a
  second, and at the one-row-a-minute fallback it runs to a minute. When the log
  does not bracket both moments, the order is `unknown` and no cause is named.

- **An incident reads the logs as they are**: a missing log is an empty log, not
  an error, and a recovered fault is a row but never an incident. `report
  --incident <id>` prints the window and its rows; `--redact` hides every detail
  line, because an error string can carry a host or an address.

- **`privacy --watch` holds one session and does not re-login**: it reads the same
  audit every N seconds (10 at the smallest) and prints a line when a drift
  settles. A router reboot or an expired session shows up as `unknown` rows, which
  are not drift, so a watch left running reports nothing rather than inventing a
  change. Restart it after a reboot. It reads only; no privacy setting is written
  by any of these flags.

- **The watch is a read, and the revert is a question**: `--watch` holds the
  session open for the requested seconds and reads `monitoring/status` every five
  seconds; it writes nothing on its own, and the window ending leaves the mask as
  applied. The revert is offered once after a minute of no state and no address,
  and taken only on a `y`, because the tool cannot tell a band it dropped from an
  ISP that went away. A session the radio restart dropped is signed in again once.
- **The speed test measures this host, not the router**: the transfer runs from
  this machine through the CPE, so the number includes the local Wi-Fi or LAN
  segment, and it is a single plain-HTTP stream rather than the sum of every
  flow. A window shorter than three seconds is printed as a ramp-limited
  estimate, because TCP has not reached its rate yet. The endpoints are third
  party (`speed.cloudflare.com` by default), so they see this host's address, and
  the bytes are counted by the router's own month counter: the usage panel moves
  by the test size (60 MB down + 10 MB up with the default bounds). Router polls
  can time out while the link is saturated; `P` pauses them, and a poll that fails
  during a test is backoff, not a gap in the history.

- **An unmeasured field stays empty in a saved run**: the loaded ping competes with
  the transfer it is measuring, so the rate it sits beside is a floor, not a
  ceiling. A run that spent bytes and then failed is saved with `status: partial`
  and marked `*` in the panel: the numbers are real, the window is short. A row
  from the CLI has no ping columns (`-`), and no row is written for a run that
  spent nothing. The panel shows ten rows and the file keeps two hundred; at
  `100x30` all ten fit, and at `80x24` it says `4 of 10` rather than pretending.

- **`scan` ranks this host's path, not the link**: the speed test runs from the
  machine routwatch sits on, through the CPE, so a VPN or a second interface in that
  path measures itself. The run refuses to start when the default route and the
  router's interface differ, and `--ignore-route` is the explicit way past that
  check. One measurement per set is also one sample: a set that wins once has won
  once, and the tables say `measured`, not `best`.

- `[app].verify_secs` (default 20): how long a band or DNS write is re-read before it
  is called a failure. The H122-373 publishes the new mask after its radio restarts,
  and one read at 1.5 s measured a landed switch as a failure.

- **A renew that keeps the same address is not a failure**: `ip renew` reads the
  address, drops the data connection, brings it back and watches. The ISP may hand
  the same lease back, and that is reported as unchanged with exit 0. The write
  path is not measured on a live H122-373 yet: no live renew has been run.

- **The signal log can stop without a word**: every command that collects appends at
  most one row a minute to `$XDG_DATA_HOME/routwatch/signal-history.jsonl`, and a write
  that fails never fails the command the user asked for. The common cause is the
  sidecar lock being held by another routwatch, which is why a gap in the log is read as
  "no sample", not as "the link was down". `ROUTWATCH_DISABLE_SIGNAL_LOG=1` turns the
  log off for scripts that read every second.

- **The observation log records changes, not attempts**: band and DNS writes,
  IP renewals, the WAN going down and coming back, a tier read that failed, and
  the signal crossing into poor or out of it are appended to
  `$XDG_DATA_HOME/routwatch/events.jsonl` (0600, one row per line, 2000 rows kept
  by compaction). Each row is stamped with the moment its tier's state was
  observed, never with the clock of the write, and a link that stays poor for an
  hour writes one row. A refused or busy read is a row; a retry that succeeded is
  not. The address a renew produced is deliberately not a field: the log is not a
  place a WAN IP may live.

- **The screen is polled, never pushed**: measured 2026-11-19 on the H122-373
  (WEBUI 11.0.2.21), seven stream candidates (`websocket`, `ws`, `events`,
  `webserver/events`, `notification`, `sse`, `event-stream`) all answered
  `100003 ERROR_NO_RIGHT`, and `user/state-login` serves a document, not a
  stream. Polling is the ceiling: no screen can be fresher than one read.
- **The client inspector is the 30 s client read, not a faster one**: the pane
  under the client table shows the six fields `wlan/host-list` carries (name, IP,
  MAC, band, SSID, time associated), the age of that read, and names the columns
  this firmware does not send (per-client traffic, per-client signal, per-client
  uptime) instead of printing a zero. No per-client detail endpoint exists on the
  H122-373 (`PROBE` carries none and the WebUI's host list is one document), so an
  inspector cannot be fresher than the tier: measured 2026-11-19, `wlan/host-list`
  p50 1191 ms, p95 2297 ms, and the pane says how old its data is.

- **Reads are grouped by how fast this firmware answers them**: signal, status
  and traffic on the refresh period, `wlan/host-list` every 30 s, and the ten
  static documents (device, net-mode, DHCP, SMS count, month counters) every
  60 s. Measured 2026-11-19: `wlan/host-list` p50 1191 ms, p95 2297 ms, the
  worst observed gap 2860 ms - a slower cadence does not shorten that gap, it
  makes it fall half as often, so the client panel can be up to about three
  seconds behind and often is.
- **The signal read is bimodal on this firmware**: 60 reads measured p50 457 ms
  and p95 1575 ms, with nine over 800 ms, clustered near 450 ms or near
  1500 ms. A single screen cannot promise better than that tail, whatever the
  timer says.
- **A slow read delays the next poll and never stacks**: there is one collection
  thread, and a tick that overruns its deadline is not replayed, so a 1.6 s read
  at a 600 ms period means one sample per 1.6 s, not a burst of catching up.
  The default period is 600 ms for that reason: a 30 s soak at 500 ms carried
  132 reads with no busy code and no re-login.
- **The frame is redrawn on a change, not on a timer alone**: a snapshot event or
  a key press asks for a frame, and a one second pulse keeps the age text moving
  when nothing else arrives. Repainting identical cells at the poll rate was pure
  terminal work.
- **A key press draws in the pass that reads it, and a burst costs one frame**:
  the loop drains every event already waiting, marks the app dirty once, and
  draws before it sleeps again. Measured 2026-11-22 over a PTY on 120x34 and
  240x60: key to first byte p50 0.3-0.5 ms, p95 1.5 ms, against p50 400 ms and
  p95 601 ms at 120x34 with a 600 ms refresh before the drain landed (p50
  802 ms, p95 1002 ms at a 2000 ms refresh). The one second pulse is for the
  age text and never releases a key.
- **Keys do not wait for the snapshot**: the collection channel is read with
  `try_recv`, so a silent or slow worker never holds input, and the 30 s client
  read is a data delay, not a keyboard delay.
- **Browsing a list is memory only**: `j`/`k` move a cursor over rows that are
  already in the snapshot. No request and no file read happens in a key handler;
  the two disk touches on the UI thread (incidents every 60 s, the usage ledger
  every 30 s) are timers, not keys.
- **The first frame goes out before the startup reads**: the frame is drawn as
  soon as the terminal is set up, then the usage ledger, the history ring and the
  incident list are read (measured 2026-11-22 on a full ledger: 32 us, 123 us and
  2.65 ms, 2.8 ms in all), and only then does the loop start. A panel with no
  snapshot yet says "collecting the first sample…" and keeps every field as `-`
  until data arrives, so nothing on screen is invented. Measured over a PTY at
  120x34: the first key after launch went from 951 ms to 0.3 ms with a 0.4 s
  settle, because the key no longer queues behind the boot path.
- **There is no clear-all at startup**: `Terminal::clear` asks the terminal for
  its cursor position before it clears, and a bare PTY never answers that query,
  so the call costs crossterm's 2 s read timeout before the first frame.
  Measured 2026-11-22: a raw clear-all escape 0.0 ms, `Terminal::clear` 2000 ms,
  and the whole boot 0.9 ms once it was dropped. The call was redundant anyway,
  because the first draw on a fresh alternate screen paints every cell. A real
  terminal answers the query at once, so the 2 s is an artefact of a terminal
  with no responder; a resize still clears, where the buffers really are stale.
- **The client order is derived once per change, not per frame**: the tab keeps
  a cached order of client indices keyed by a fingerprint of the client list and
  the active sort, so `j`/`k` and a redraw reuse it. The row loop used to call
  `selected_client()` per row, which re-sorted the whole list per row. Measured
  2026-11-22 with 40 clients on a release build: a 120x34 frame fell from p50
  608 us to 228 us and a 240x60 frame from p50 1018 us to 469 us, and the cost
  no longer grows with the number of clients.
- **The periodic UI work is two small timers and nothing else**: the incident
  list is re-read and re-derived every 60 s (measured 2.4 ms, twice, at 60.1 s
  and 120.1 s) and the usage ledger is written every 30 s when it is dirty
  (measured p50 0.5 ms, p95 0.7 ms, max 1.9 ms on 31 day buckets). A 156 s window
  at 120x34 and a 600 ms refresh carried 240 key samples with p99 1.0 ms and max
  1.05 ms after the first key, so no spike lands on a key press. There is no
  disk thread: a second thread would buy 2.4 ms every 60 s and cost a thread.
- **Every timing number here was measured inside a PTY**: the probe owns the
  pty, sets its size, and reads the bytes the app writes. It cannot see a
  compositor or a transparent terminal, so nothing here describes the owner's
  own desktop; whether the screen feels immediate there is his call after he
  installs the build.
- **A wide terminal costs bytes, not milliseconds**: a full repaint measured
  6.5 KB at 120x34 and 12.2 KB at 240x60, and key to first byte was the same at
  both sizes, so the write itself is not what a slow terminal makes you feel.
- **The age is shown per group**: `signal 0.6s · clients 12s · device 41s` in the
  header, because one age for the whole screen could not say which read was late.
  Narrow terminals fall back to the single age of the last full snapshot.
- **The signal log row is stamped at the radio read**: the row written to
  `$XDG_DATA_HOME/routwatch/signal-history.jsonl` carries the moment the signal
  endpoint answered, not the moment the whole snapshot was assembled, so a slow
  client list never shifts the logged sample by seconds.
- **The tests never write your data directory.** Every test that can reach a
  persistent log points `XDG_DATA_HOME` at its own scratch directory and removes it
  on drop, so a plain `cargo test --locked` leaves `$XDG_DATA_HOME/routwatch/`
  byte-identical and creates nothing there. `scripts/gate-isolation.sh` proves it:
  a run against a sentinel directory of marker logs (which must not move), a run
  against an empty directory (which must stay empty), and, when the TUI is not
  running, a run against the real directory. The sentinel check alone can miss a
  leak, because the signal log writes at most one row a minute.

- **The capability matrix records what this firmware answered**: one state per
  endpoint, `answered`, `refused` with the firmware's code, `not served` (a 404) or
  `untested`, keyed by the firmware string and the moment it was measured. A refusal
  is recorded, never folded into "this firmware lacks the feature", and an `untested`
  endpoint never hides a feature: the matrix is a record, not a permission system.
- **A config snapshot keeps field names and salted fingerprints, never a value**:
  `wlan/basic-settings#WifiKey` is recorded by name and its presence, and the digest
  needs both the salt file (`$XDG_DATA_HOME/routwatch/config-salt`, 0600) and the
  value to be useful, which is what makes the fingerprint change detection and not
  storage. A changed secret reads as presence only. There is no restore command.
- **A config change needs two reads that agree, and a firmware difference mutes the
  rest**: `config diff` reads every endpoint twice, and a field the two reads disagree
  on is printed as `unsettled` rather than reported. An endpoint that answered before
  and refuses now is one `visibility-changed` row, not a removal. When the two
  snapshots were taken on different firmware versions the field rows are suppressed
  and the version difference is the finding.
- **Band advice is per served band, except what came from a scan**: the signal log
  records the band that was served, not the set that was configured, so the served
  ranking says so and set advice comes only from `scan --json` results passed with
  `--scans`. The two rankings are never mixed, a reading inside the derived smear is
  not attributed to either band, and a figure beside a band is associated with it,
  never caused by it. The derived smear is the 95th percentile gap between the samples
  inside the window, not the widest gap in the log: a pause of hours is absence of
  observation, and letting it mute the day would hide every real change.
- **Band advice has four confidence levels and no write path**: `insufficient`, `low`,
  `medium`, `high`, from the rows and the measured scans, and the weakest link sets the
  whole advice. `bands advise` prints the command that would add evidence, switches
  nothing, writes no log and needs no router (an unreachable router narrows the advice
  instead of stopping it).

The write path itself is a promise, not a limitation, and it is kept by a test:
reads are free, writes are confirmed and read back. The three rules are in the
README's Safety list, and the guard that enforces them is in
[`contracts.md`](contracts.md).

Back: [README](../README.md) · [all docs](README.md)
