//! Application state for the TUI. Pure data + transitions: no I/O here, so a
//! test can build an `App` from a fixture and render it.

use crate::config::BandPreset;
use crate::model::{Client, DeviceExtras, Snapshot};
use crate::router::bands;
use crate::theme::{GraphStyle, Theme};
use crate::worker::PingOutcome;
use crate::write::{parse_dns_pair, WriteIntent, WriteState};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const HISTORY: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Bands,
    Traffic,
    Network,
    Clients,
    Sms,
    Device,
    Dns,
    /// The signal-gauge dashboard, reachable as a tab (9) and as the `gauges` view.
    Signal,
}

impl Tab {
    pub const ALL: [Tab; 9] = [
        Tab::Dashboard,
        Tab::Bands,
        Tab::Traffic,
        Tab::Network,
        Tab::Clients,
        Tab::Sms,
        Tab::Device,
        Tab::Dns,
        Tab::Signal,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Bands => "Bands",
            Tab::Traffic => "Traffic",
            Tab::Network => "Network",
            Tab::Clients => "Clients",
            Tab::Sms => "SMS",
            Tab::Device => "Device",
            Tab::Dns => "DNS",
            Tab::Signal => "Signal",
        }
    }

    /// Short label for narrow terminals, where eight full titles do not fit.
    pub fn short(&self) -> &'static str {
        match self {
            Tab::Dashboard => "Dash",
            Tab::Bands => "Bands",
            Tab::Traffic => "Traf",
            Tab::Network => "Net",
            Tab::Clients => "Cli",
            Tab::Sms => "SMS",
            Tab::Device => "Dev",
            Tab::Dns => "DNS",
            Tab::Signal => "Sig",
        }
    }

    /// The keys that belong to this section, drawn in the status bar so every
    /// tab says what it accepts without a help overlay.
    pub fn hints(&self) -> &'static str {
        match self {
            Tab::Dashboard => "v view · g graph · r refresh · p pause",
            Tab::Bands => "space tick · j/k · n preset · s save · x del · m mode · o opt · a apply",
            Tab::Traffic => "d edit DNS · D auto DNS",
            Tab::Network => "t ping host",
            Tab::Clients => "j/k select · s sort · c copy mac",
            Tab::Sms => "R read inbox",
            Tab::Device => "x dump json · b restart · R extras",
            Tab::Dns => "j/k choose · Enter apply · d type · D auto",
            Tab::Signal => "v view · g graph",
        }
    }

    pub fn index(&self) -> usize {
        Tab::ALL.iter().position(|t| t == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Tab {
        Tab::ALL[i.min(Tab::ALL.len() - 1)]
    }

    pub fn next(&self) -> Tab {
        Tab::from_index((self.index() + 1) % Tab::ALL.len())
    }

    pub fn prev(&self) -> Tab {
        Tab::from_index((self.index() + Tab::ALL.len() - 1) % Tab::ALL.len())
    }

    /// Parse the plan's `--tab` spellings.
    pub fn parse(s: &str) -> Option<Tab> {
        Tab::ALL.iter().copied().find(|t| {
            t.title().eq_ignore_ascii_case(s) || t.title().to_lowercase() == s.to_ascii_lowercase()
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Full,
    /// Signal dashboard: radial gauges + speed + network strip + graphs.
    Gauges,
    Lite,
    Dense,
}

impl View {
    pub fn name(&self) -> &'static str {
        match self {
            View::Full => "full",
            View::Gauges => "gauges",
            View::Lite => "lite",
            View::Dense => "dense",
        }
    }

    pub fn next(&self) -> View {
        match self {
            View::Full => View::Gauges,
            View::Gauges => View::Lite,
            View::Lite => View::Dense,
            View::Dense => View::Full,
        }
    }

    pub fn parse(s: &str) -> Option<View> {
        match s.to_ascii_lowercase().as_str() {
            "full" => Some(View::Full),
            "gauges" | "gauge" => Some(View::Gauges),
            "lite" => Some(View::Lite),
            "dense" => Some(View::Dense),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    Live,
    /// After a verified write: the link drop is expected, not an error.
    Restarting,
    Recovering {
        attempt: u8,
        wait_s: u64,
    },
    Disconnected(String),
}

impl SessionState {
    pub fn label(&self) -> String {
        match self {
            SessionState::Connecting => "connecting".into(),
            SessionState::Live => "live".into(),
            SessionState::Restarting => "restarting — waiting for the router to stabilize…".into(),
            SessionState::Recovering { attempt, wait_s } => {
                format!("waiting for router to stabilize… #{attempt} in {wait_s}s")
            }
            SessionState::Disconnected(e) => format!("disconnected: {e}"),
        }
    }
}

/// What a preset row does when the user applies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsPreset {
    /// Hand DNS back to the DHCP server (DnsStatus=1).
    Automatic,
    /// A fixed pair, applied after confirmation.
    Servers(&'static str, &'static str),
    /// Open the text prompt so the user can type any two IPv4 servers.
    Custom,
}

/// Ready-made rows so changing DNS does not require typing addresses — plus a
/// Custom row that opens the prompt, so nothing is unreachable from the tab.
pub const DNS_PRESETS: &[(&str, DnsPreset)] = &[
    ("Automatic (DHCP)", DnsPreset::Automatic),
    ("Cloudflare", DnsPreset::Servers("1.1.1.1", "1.0.0.1")),
    ("Google", DnsPreset::Servers("8.8.8.8", "8.8.4.4")),
    ("Quad9", DnsPreset::Servers("9.9.9.9", "149.112.112.112")),
    (
        "AdGuard",
        DnsPreset::Servers("94.140.14.14", "94.140.15.15"),
    ),
    ("Custom… (type your own)", DnsPreset::Custom),
];

/// Which signal panel the device's own data justifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalPanel {
    /// 4G readings present: four dBm gauges.
    LteGauges,
    /// NR per-field metrics present (not this firmware): full 5G gauges.
    NrGauges,
    /// ENDC/NR bars but no NR metrics: show bars, say dBm is unavailable.
    NrBars,
}

/// Pure rule, unit-tested: never invent 5G values the firmware did not send.
pub fn resolve_signal_panel(snap: Option<&Snapshot>) -> SignalPanel {
    let Some(s) = snap else {
        return SignalPanel::LteGauges;
    };
    if s.signal.nr.as_ref().and_then(|n| n.rsrp_dbm).is_some() {
        return SignalPanel::NrGauges;
    }
    if s.connection.endc || s.connection.nr_bars > 0 {
        return SignalPanel::NrBars;
    }
    SignalPanel::LteGauges
}

/// What happened when the user pressed `s` in the Bands tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetSaved {
    Saved(BandPreset),
    Duplicate(usize),
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Ip,
    Band,
    Time,
}

impl SortKey {
    pub fn next(&self) -> SortKey {
        match self {
            SortKey::Name => SortKey::Ip,
            SortKey::Ip => SortKey::Band,
            SortKey::Band => SortKey::Time,
            SortKey::Time => SortKey::Name,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Ip => "ip",
            SortKey::Band => "band",
            SortKey::Time => "time",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    PingHost,
    Dns,
    /// Type the word that unlocks the restart. Never a keystroke.
    RestartConfirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Seconds since the app started — no clock dependency, still ordered.
    pub at: u64,
    pub level: LogLevel,
    pub msg: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub kind: InputKind,
    pub label: &'static str,
    pub value: String,
}

pub struct App {
    pub tab: Tab,
    pub view: View,
    /// Where `l` returns to: Lite is a detour, not a destination.
    pub view_before_lite: View,
    pub theme: Theme,
    pub graph: GraphStyle,
    pub snap: Option<Snapshot>,
    pub history_dl: VecDeque<f64>,
    pub history_ul: VecDeque<f64>,
    pub history_rsrp: VecDeque<f64>,
    pub history_rsrq: VecDeque<f64>,
    pub history_sinr: VecDeque<f64>,
    pub last_ok: Option<Instant>,
    pub last_poll_ms: u128,
    pub session: SessionState,
    pub paused: bool,
    pub quit: bool,
    pub help: bool,
    pub client_sel: usize,
    pub error_log: VecDeque<String>,
    pub budget_gb: f64,
    pub refresh_ms: u64,
    // --- bands tab (M3 write path) ---
    pub bands_supported: Vec<u32>,
    pub bands_selected: Vec<u8>,
    pub band_cursor: usize,
    pub band_mode: String,
    pub band_option: String,
    /// Merge presets: ready-made combos plus whatever the user saved with `s`.
    pub band_presets: Vec<BandPreset>,
    /// `None` until `n` is pressed, so the first press lands on the first preset.
    pub preset_cursor: Option<usize>,
    pub write: WriteState,
    pub allow_write: bool,
    bands_initialised: bool,
    // --- M4 ---
    pub input: Option<Input>,
    pub ping: Option<PingOutcome>,
    pub client_sort: SortKey,
    pub notice: Option<(String, Instant)>,
    pub dns_preset: usize,
    // --- M5 ---
    pub extras: Option<DeviceExtras>,
    pub extras_requested: bool,
    pub event_log: VecDeque<LogEntry>,
    pub settings_open: bool,
    /// Where `w` writes the settings (set by the CLI, never guessed here).
    pub config_path: Option<std::path::PathBuf>,
    started: Instant,
}

impl App {
    /// Choosing a section leaves a stripped view: Lite/Dense are single-screen
    /// summaries that ignore `tab`, so a digit there used to look dead.
    pub fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
        // Lite, Dense and Gauges are whole-screen summaries that ignore `tab`;
        // picking a section must leave them or the digit appears to do nothing.
        if matches!(self.view, View::Lite | View::Dense | View::Gauges) {
            self.view = View::Full;
        }
    }

    /// `l` is a toggle: Lite is a detour, so the second press returns to the
    /// view the user was actually in (Full, Dense or Gauges).
    pub fn toggle_lite(&mut self) {
        if self.view == View::Lite {
            self.view = self.view_before_lite;
        } else {
            self.view_before_lite = self.view;
            self.view = View::Lite;
        }
    }

    /// Cycle views, remembering where `l` should come back to.
    pub fn cycle_view(&mut self) {
        self.view = self.view.next();
        if self.view != View::Lite {
            self.view_before_lite = self.view;
        }
    }

    pub fn new(
        theme: Theme,
        graph: GraphStyle,
        view: View,
        budget_gb: u64,
        refresh_ms: u64,
    ) -> App {
        App {
            tab: Tab::Dashboard,
            view,
            view_before_lite: if view == View::Lite { View::Full } else { view },
            theme,
            graph,
            snap: None,
            history_dl: VecDeque::with_capacity(HISTORY),
            history_ul: VecDeque::with_capacity(HISTORY),
            history_rsrp: VecDeque::with_capacity(HISTORY),
            history_rsrq: VecDeque::with_capacity(HISTORY),
            history_sinr: VecDeque::with_capacity(HISTORY),
            last_ok: None,
            last_poll_ms: 0,
            session: SessionState::Connecting,
            paused: false,
            quit: false,
            help: false,
            client_sel: 0,
            error_log: VecDeque::new(),
            budget_gb: budget_gb as f64,
            refresh_ms,
            bands_supported: Vec::new(),
            bands_selected: Vec::new(),
            band_cursor: 0,
            band_mode: "00".into(),
            band_option: "0".into(),
            band_presets: crate::config::default_presets(),
            preset_cursor: None,
            write: WriteState::Idle,
            allow_write: false,
            bands_initialised: false,
            input: None,
            ping: None,
            client_sort: SortKey::Name,
            notice: None,
            dns_preset: 0,
            extras: None,
            extras_requested: false,
            event_log: VecDeque::new(),
            settings_open: false,
            config_path: None,
            started: Instant::now(),
        }
    }

    pub fn on_snapshot(&mut self, snap: Snapshot, poll_ms: u128) {
        self.history_dl.push_back(snap.traffic.dl_rate_bytes as f64);
        self.history_ul.push_back(snap.traffic.ul_rate_bytes as f64);
        while self.history_dl.len() > HISTORY {
            self.history_dl.pop_front();
        }
        while self.history_ul.len() > HISTORY {
            self.history_ul.pop_front();
        }
        let push = |q: &mut VecDeque<f64>, v: Option<f32>| {
            if let Some(x) = v {
                q.push_back(x as f64);
                while q.len() > HISTORY {
                    q.pop_front();
                }
            }
        };
        push(&mut self.history_rsrp, snap.signal.rsrp_dbm);
        push(&mut self.history_rsrq, snap.signal.rsrq_db);
        push(&mut self.history_sinr, snap.signal.sinr_db);

        if let Some(c) = snap.clients.len().checked_sub(1) {
            if self.client_sel > c {
                self.client_sel = c;
            }
        } else {
            self.client_sel = 0;
        }
        let supported = snap.supported_bands.clone();
        let current_mask = snap
            .net_mode
            .lte_band
            .as_deref()
            .and_then(bands::from_hex)
            .map(bands::bands_from_mask)
            .unwrap_or_default();
        let current_mode = snap
            .net_mode
            .network_mode
            .clone()
            .unwrap_or_else(|| "00".into());
        if !self.bands_initialised && !supported.is_empty() {
            self.bands_supported = supported;
            self.bands_selected = current_mask;
            self.band_mode = current_mode;
            self.bands_initialised = true;
        }
        let first_or_recovered = self.session != SessionState::Live;
        if first_or_recovered {
            let plmn = snap.plmn.name.clone();
            let net = snap.connection.net_type.clone();
            self.log(LogLevel::Info, format!("session live · {plmn} · {net}"));
        }
        self.snap = Some(snap);
        self.last_ok = Some(Instant::now());
        self.last_poll_ms = poll_ms;
        self.session = SessionState::Live;
    }

    pub fn on_error(&mut self, msg: String) {
        self.log(LogLevel::Error, msg.clone());
        self.error_log.push_front(msg);
        while self.error_log.len() > 200 {
            self.error_log.pop_back();
        }
    }

    /// Append to the session event log (shown on the Device tab).
    pub fn log(&mut self, level: LogLevel, msg: impl Into<String>) {
        self.event_log.push_front(LogEntry {
            at: self.started.elapsed().as_secs(),
            level,
            msg: msg.into(),
        });
        while self.event_log.len() > 200 {
            self.event_log.pop_back();
        }
    }

    pub fn on_extras(&mut self, extras: DeviceExtras) {
        match &extras.sms_error {
            Some(err) => self.log(LogLevel::Warn, format!("inbox blocked: {err}")),
            None => self.log(
                LogLevel::Info,
                format!("inbox: {} messages", extras.sms_inbox.len()),
            ),
        }
        if let Some(pin) = &extras.pin {
            if pin.locked() {
                self.log(LogLevel::Warn, "SIM is waiting for a PIN");
            }
        }
        self.extras_requested = false;
        self.extras = Some(extras);
    }

    /// The SMS/Device tabs ask for extras once; `R` forces a refetch.
    pub fn needs_extras(&self) -> bool {
        self.extras.is_none() && !self.extras_requested
    }

    pub fn mark_extras_requested(&mut self) {
        self.extras_requested = true;
    }

    pub fn adjust_refresh(&mut self, delta: i64) {
        let next = (self.refresh_ms as i64 + delta).clamp(500, 10_000) as u64;
        self.refresh_ms = (next / 100) * 100;
    }

    pub fn adjust_budget(&mut self, delta: f64) {
        self.budget_gb = (self.budget_gb + delta).clamp(1.0, 10_000.0);
    }

    /// The literal the user must type to unlock the restart.
    pub fn restart_word() -> &'static str {
        "REBOOT"
    }

    pub fn band_mask_hex(&self) -> String {
        bands::to_hex(bands::mask_from_bands(&self.bands_selected))
    }

    /// Both sides of the pending write, for the confirm modal. `None` when no
    /// write is awaiting confirmation.
    pub fn preflight(&self) -> Option<crate::preflight::Preflight> {
        let intent = self.write.intent()?;
        Some(crate::preflight::for_intent(intent, self.snap.as_ref()))
    }

    pub fn band_toggle(&mut self) {
        if self.write.is_busy() || matches!(self.write, WriteState::Confirming(_)) {
            return;
        }
        let Some(band) = self.bands_supported.get(self.band_cursor).copied() else {
            return;
        };
        if let Some(pos) = self.bands_selected.iter().position(|b| *b as u32 == band) {
            self.bands_selected.remove(pos);
        } else {
            self.bands_selected.push(band as u8);
            self.bands_selected.sort_unstable();
            self.bands_selected.dedup();
        }
    }

    pub fn band_cursor_move(&mut self, delta: isize) {
        if self.bands_supported.is_empty() {
            return;
        }
        let max = self.bands_supported.len() as isize - 1;
        let next = (self.band_cursor as isize + delta).clamp(0, max);
        self.band_cursor = next as usize;
    }

    pub fn cycle_mode(&mut self) {
        self.band_mode = match self.band_mode.as_str() {
            "00" => "03".into(),
            "03" => "08".into(),
            _ => "00".into(),
        };
    }

    pub fn cycle_option(&mut self) {
        self.band_option = match self.band_option.as_str() {
            "0" => "1".into(),
            "1" => "2".into(),
            _ => "0".into(),
        };
    }

    /// The merge the user is building right now: `B1 + B3 + B5`.
    pub fn band_merge_label(&self) -> String {
        if self.bands_selected.is_empty() {
            return "—".into();
        }
        self.bands_selected
            .iter()
            .map(|b| format!("B{b}"))
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// Bands a preset asks for that this device's own list does not contain.
    pub fn preset_missing(&self, idx: usize) -> Vec<u8> {
        let Some(p) = self.band_presets.get(idx) else {
            return Vec::new();
        };
        if self.bands_supported.is_empty() {
            return Vec::new();
        }
        p.bands
            .iter()
            .copied()
            .filter(|b| !self.bands_supported.contains(&(*b as u32)))
            .collect()
    }

    /// Cycle to the next preset and load it into the tick list. Loading is not
    /// applying: `a` still goes through the confirmation.
    pub fn band_preset_next(&mut self) -> Option<String> {
        if self.band_presets.is_empty() {
            return None;
        }
        let next = match self.preset_cursor {
            None => 0,
            Some(i) => (i + 1) % self.band_presets.len(),
        };
        self.preset_cursor = Some(next);
        let bands: Vec<u8> = self.band_presets[next]
            .bands
            .iter()
            .copied()
            .filter(|b| {
                self.bands_supported.is_empty() || self.bands_supported.contains(&(*b as u32))
            })
            .collect();
        if bands.is_empty() {
            return None;
        }
        self.bands_selected = bands;
        Some(self.band_presets[next].name.clone())
    }

    /// Save the ticked bands as a merge preset. An identical merge is not
    /// duplicated: the cursor just moves to it.
    pub fn band_preset_save(&mut self) -> PresetSaved {
        if self.bands_selected.is_empty() {
            return PresetSaved::Empty;
        }
        let preset = BandPreset::new(&self.bands_selected);
        if let Some(i) = self
            .band_presets
            .iter()
            .position(|p| p.bands == preset.bands)
        {
            self.preset_cursor = Some(i);
            return PresetSaved::Duplicate(i);
        }
        self.band_presets.push(preset.clone());
        self.preset_cursor = Some(self.band_presets.len() - 1);
        PresetSaved::Saved(preset)
    }

    pub fn band_preset_delete(&mut self) -> Option<BandPreset> {
        let i = self.preset_cursor?;
        if i >= self.band_presets.len() {
            self.preset_cursor = None;
            return None;
        }
        let removed = self.band_presets.remove(i);
        self.preset_cursor = if self.band_presets.is_empty() {
            None
        } else {
            Some(i.min(self.band_presets.len() - 1))
        };
        Some(removed)
    }

    pub fn band_intent(&self) -> WriteIntent {
        WriteIntent::ApplyBands {
            mode: self.band_mode.clone(),
            bands: self.bands_selected.clone(),
            mask: self.band_mask_hex(),
            option: if self.band_mode == "08" {
                Some(self.band_option.clone())
            } else {
                None
            },
        }
    }

    /// `a` in the Bands tab: needs a non-empty selection and nothing in flight.
    pub fn begin_confirm(&mut self) -> bool {
        if self.bands_selected.is_empty() || self.write.is_busy() {
            return false;
        }
        self.write = WriteState::Confirming(self.band_intent());
        true
    }

    pub fn cancel_confirm(&mut self) {
        if matches!(self.write, WriteState::Confirming(_)) {
            self.write = WriteState::Idle;
        }
    }

    pub fn settle_write(&mut self, state: WriteState) {
        match &state {
            WriteState::Success { intent, note } => self.log(
                LogLevel::Info,
                format!("write ok · {} · {note}", intent.label()),
            ),
            WriteState::Failed { intent, err } => self.log(
                LogLevel::Error,
                format!("write failed · {} · {err}", intent.label()),
            ),
            _ => {}
        }
        self.write = state;
    }

    /// How old the displayed data is — the header turns this into `live`/`stale`.
    pub fn age(&self) -> Option<Duration> {
        self.last_ok.map(|t| t.elapsed())
    }

    pub fn stale(&self) -> bool {
        self.age()
            .map(|a| a.as_millis() as u64 > self.refresh_ms.saturating_mul(3))
            .unwrap_or(false)
    }

    pub fn signal_quality(&self) -> &'static str {
        match &self.snap {
            Some(s) => match s.signal.rsrp_dbm {
                Some(v) => self.theme.rsrp_quality(v),
                None => "n/a",
            },
            None => "n/a",
        }
    }

    /// Clients in display order. Sorting never touches the snapshot itself.
    pub fn sorted_clients(&self) -> Vec<&Client> {
        let Some(s) = &self.snap else {
            return Vec::new();
        };
        let mut list: Vec<&Client> = s.clients.iter().collect();
        match self.client_sort {
            SortKey::Name => list.sort_by_key(|c| {
                let name = c.name.clone().unwrap_or_default().to_lowercase();
                // unnamed hosts sort last: they are the least useful rows
                (name.is_empty(), name)
            }),
            SortKey::Ip => list.sort_by_key(|c| ip_key(c.ip.as_deref().unwrap_or(""))),
            SortKey::Band => list.sort_by_key(|c| c.freq.clone().unwrap_or_default()),
            SortKey::Time => list.sort_by_key(|c| std::cmp::Reverse(c.associated_secs)),
        }
        list
    }

    pub fn selected_client(&self) -> Option<usize> {
        let len = self.sorted_clients().len();
        if len == 0 {
            None
        } else {
            Some(self.client_sel.min(len - 1))
        }
    }

    pub fn client_cursor(&mut self, delta: isize) {
        let len = self.sorted_clients().len() as isize;
        if len == 0 {
            return;
        }
        self.client_sel = (self.client_sel as isize + delta).clamp(0, len - 1) as usize;
    }

    pub fn sort_next(&mut self) {
        self.client_sort = self.client_sort.next();
        self.client_sel = 0;
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), Instant::now()));
    }

    pub fn set_ping(&mut self, outcome: PingOutcome) {
        let level = if outcome.ok {
            LogLevel::Info
        } else {
            LogLevel::Warn
        };
        self.log(level, format!("ping {} · {}", outcome.host, outcome.detail));
        self.note(format!("ping {} · {}", outcome.host, outcome.detail));
        self.ping = Some(outcome);
    }

    /// Transient message shown in the status bar for a few seconds.
    pub fn live_notice(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, at)| at.elapsed().as_secs() < 8)
            .map(|(m, _)| m.as_str())
    }

    pub fn open_input(&mut self, kind: InputKind) {
        let (label, value) = match kind {
            InputKind::PingHost => (
                "ping host",
                self.ping
                    .as_ref()
                    .map(|p| p.host.clone())
                    .unwrap_or_else(|| "1.1.1.1".to_string()),
            ),
            InputKind::Dns => {
                let (p, s) = self
                    .snap
                    .as_ref()
                    .and_then(|s| s.dns.as_ref())
                    .map(|d| {
                        (
                            d.primary.clone().unwrap_or_default(),
                            d.secondary.clone().unwrap_or_default(),
                        )
                    })
                    .unwrap_or_default();
                ("dns primary,secondary", format!("{p},{s}"))
            }
            InputKind::RestartConfirm => ("type REBOOT to restart the router", String::new()),
        };
        self.input = Some(Input { kind, label, value });
    }

    pub fn input_push(&mut self, c: char) {
        if let Some(input) = &mut self.input {
            if input.value.chars().count() < 64 {
                input.value.push(c);
            }
        }
    }

    pub fn input_backspace(&mut self) {
        if let Some(input) = &mut self.input {
            input.value.pop();
        }
    }

    /// Close the input and return what was typed, so the caller can dispatch.
    pub fn input_take(&mut self) -> Option<(InputKind, String)> {
        let input = self.input.take()?;
        Some((input.kind, input.value.trim().to_string()))
    }

    /// Turn a typed "primary,secondary" into a confirmable intent.
    pub fn begin_dns_confirm(&mut self, raw: &str) -> Result<(), String> {
        let (primary, secondary) = parse_dns_pair(raw)?;
        self.write = WriteState::Confirming(WriteIntent::SetDns { primary, secondary });
        Ok(())
    }

    pub fn dns_preset_move(&mut self, delta: isize) {
        let len = DNS_PRESETS.len() as isize;
        if len == 0 {
            return;
        }
        self.dns_preset = (self.dns_preset as isize + delta).clamp(0, len - 1) as usize;
    }

    pub fn dns_preset_is_custom(&self) -> bool {
        matches!(
            DNS_PRESETS.get(self.dns_preset).map(|(_, p)| p),
            Some(DnsPreset::Custom)
        )
    }

    /// Turn the highlighted preset into a confirmable intent (writes need `y`).
    /// Returns false for the Custom row: the caller opens the text prompt.
    pub fn begin_dns_preset(&mut self) -> bool {
        if self.write.is_busy() {
            return false;
        }
        match DNS_PRESETS.get(self.dns_preset).map(|(_, p)| p) {
            Some(DnsPreset::Automatic) => {
                self.write = WriteState::Confirming(WriteIntent::ResetDns);
                true
            }
            Some(DnsPreset::Servers(primary, secondary)) => {
                self.write = WriteState::Confirming(WriteIntent::SetDns {
                    primary: (*primary).to_string(),
                    secondary: (*secondary).to_string(),
                });
                true
            }
            _ => false,
        }
    }

    pub fn begin_dns_reset(&mut self) {
        if self.write.is_busy() {
            return;
        }
        self.write = WriteState::Confirming(WriteIntent::ResetDns);
    }
}

/// Typed-confirmation guard: only the exact word unlocks a restart.
pub fn restart_allowed(typed: &str) -> bool {
    typed == App::restart_word()
}

/// Sort addresses numerically, not lexically (192.168.8.104 < 192.168.8.20 is wrong).
fn ip_key(ip: &str) -> (u32, u32, u32, u32) {
    let mut parts = ip.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_cycle_both_ways() {
        assert_eq!(Tab::Dashboard.next(), Tab::Bands);
        assert_eq!(Tab::Dashboard.prev(), Tab::Signal);
        assert_eq!(Tab::parse("traffic"), Some(Tab::Traffic));
        assert_eq!(Tab::parse("Bands"), Some(Tab::Bands));
        assert_eq!(Tab::parse("nope"), None);
    }

    #[test]
    fn dns_presets_produce_intents() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.dns_preset = 0;
        assert!(app.begin_dns_preset());
        assert!(matches!(
            app.write,
            WriteState::Confirming(WriteIntent::ResetDns)
        ));

        app.settle_write(WriteState::Idle);
        app.dns_preset = 5; // Custom… -> no intent, the prompt opens instead
        assert!(app.dns_preset_is_custom());
        assert!(!app.begin_dns_preset());
        assert!(matches!(app.write, WriteState::Idle));

        app.dns_preset = 1; // Cloudflare
        assert!(app.begin_dns_preset());
        match &app.write {
            WriteState::Confirming(WriteIntent::SetDns { primary, secondary }) => {
                assert_eq!(
                    (primary.as_str(), secondary.as_str()),
                    ("1.1.1.1", "1.0.0.1")
                );
            }
            other => panic!("expected SetDns, got {other:?}"),
        }

        // no preset can be applied twice while one is in flight
        app.settle_write(WriteState::Pending {
            intent: app.write.intent().cloned().unwrap(),
            since: std::time::Instant::now(),
        });
        assert!(!app.begin_dns_preset());
    }

    #[test]
    fn presets_save_dedupe_cycle_and_delete() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.bands_supported = vec![1, 3, 5, 8, 20, 28, 32];
        let builtins = app.band_presets.len();
        assert!(builtins >= 5, "the ready-made merges must ship");

        app.bands_selected = vec![1, 3, 20];
        match app.band_preset_save() {
            PresetSaved::Saved(p) => assert_eq!(p.name, "B1+B3+B20"),
            other => panic!("expected Saved, got {other:?}"),
        }
        assert_eq!(app.band_presets.len(), builtins + 1);
        assert_eq!(
            app.preset_cursor,
            Some(builtins),
            "cursor follows the new preset"
        );

        // same set in a different order is the same merge: no duplicate
        app.bands_selected = vec![20, 3, 1];
        assert!(matches!(app.band_preset_save(), PresetSaved::Duplicate(i) if i == builtins));
        assert_eq!(app.band_presets.len(), builtins + 1);

        app.bands_selected.clear();
        assert!(matches!(app.band_preset_save(), PresetSaved::Empty));

        // the first `n` lands on the first preset, not the second
        app.preset_cursor = None;
        assert_eq!(app.band_preset_next().as_deref(), Some("B1+B3"));
        assert_eq!(app.bands_selected, vec![1, 3]);
        assert_eq!(app.band_merge_label(), "B1 + B3");
        assert_eq!(app.band_mask_hex(), "5");
        // loading a preset must never arm a write by itself
        assert!(matches!(app.write, WriteState::Idle));

        assert_eq!(
            app.band_preset_delete().map(|p| p.name),
            Some("B1+B3".into())
        );
        assert_eq!(app.preset_cursor, Some(0));
    }

    #[test]
    fn preset_load_filters_to_bands_this_device_lists() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.bands_supported = vec![1, 3];
        app.band_presets = vec![BandPreset::new(&[1, 3, 20])];
        app.preset_cursor = None;

        assert_eq!(app.band_preset_next().as_deref(), Some("B1+B3+B20"));
        assert_eq!(
            app.bands_selected,
            vec![1, 3],
            "B20 is not offered by this device"
        );
        assert_eq!(app.preset_missing(0), vec![20]);

        // a preset with nothing supported leaves the tick untouched
        app.band_presets = vec![BandPreset::new(&[20])];
        app.preset_cursor = None;
        app.bands_selected = vec![1];
        assert_eq!(app.band_preset_next(), None);
        assert_eq!(app.bands_selected, vec![1]);

        // deleting with no cursor is a no-op, not a panic
        app.preset_cursor = None;
        assert!(app.band_preset_delete().is_none());
    }

    #[test]
    fn history_is_bounded() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        for i in 0..(HISTORY + 40) {
            let mut s = Snapshot::default();
            s.traffic.dl_rate_bytes = i as u64;
            app.on_snapshot(s, 400);
        }
        assert_eq!(app.history_dl.len(), HISTORY);
        assert_eq!(*app.history_dl.back().unwrap(), (HISTORY + 39) as f64);
    }

    #[test]
    fn stale_after_three_intervals() {
        let mut app = App::new(Theme::get("terminal"), GraphStyle::Dots, View::Full, 100, 0);
        assert!(!app.stale());
        app.last_ok = Some(Instant::now() - Duration::from_millis(5));
        assert!(app.stale(), "refresh_ms=0 makes any age stale");
    }

    #[test]
    fn clients_sort_numerically_and_by_time() {
        use crate::model::Client;
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        let snap = Snapshot {
            clients: vec![
                Client {
                    name: Some("b".into()),
                    ip: Some("192.168.8.20".into()),
                    associated_secs: 10,
                    ..Default::default()
                },
                Client {
                    name: Some("a".into()),
                    ip: Some("192.168.8.104".into()),
                    associated_secs: 99,
                    ..Default::default()
                },
                Client {
                    name: Some("c".into()),
                    ip: Some("192.168.8.9".into()),
                    associated_secs: 50,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        app.on_snapshot(snap, 1);

        app.client_sort = SortKey::Ip;
        let ips: Vec<&str> = app
            .sorted_clients()
            .iter()
            .map(|c| c.ip.as_deref().unwrap())
            .collect();
        assert_eq!(
            ips,
            vec!["192.168.8.9", "192.168.8.20", "192.168.8.104"],
            "numeric, not lexical"
        );

        app.client_sort = SortKey::Time;
        let names: Vec<&str> = app
            .sorted_clients()
            .iter()
            .map(|c| c.name.as_deref().unwrap())
            .collect();
        assert_eq!(names, vec!["a", "c", "b"], "newest association first");

        app.client_sort = SortKey::Name;
        let names: Vec<&str> = app
            .sorted_clients()
            .iter()
            .map(|c| c.name.as_deref().unwrap())
            .collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn client_cursor_is_bounded_in_display_order() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        let snap = Snapshot {
            clients: (0..3)
                .map(|i| crate::model::Client {
                    name: Some(format!("c{i}")),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        app.on_snapshot(snap, 1);
        app.client_cursor(5);
        assert_eq!(app.client_sel, 2);
        app.client_cursor(-9);
        assert_eq!(app.client_sel, 0);
    }

    #[test]
    fn dns_input_becomes_a_confirmable_intent() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        assert!(app.begin_dns_confirm("1.1.1.1, 1.0.0.1").is_ok());
        match &app.write {
            WriteState::Confirming(WriteIntent::SetDns { primary, secondary }) => {
                assert_eq!(primary, "1.1.1.1");
                assert_eq!(secondary, "1.0.0.1");
            }
            other => panic!("expected a confirming DNS intent, got {other:?}"),
        }
        // invalid input must not open a modal
        app.settle_write(WriteState::Idle);
        assert!(app.begin_dns_confirm("1.1.1.1").is_err());
        assert!(matches!(app.write, WriteState::Idle));
    }

    #[test]
    fn input_editing_and_take() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.open_input(InputKind::PingHost);
        // the prompt is prefilled with the last host; start typing from empty
        if let Some(i) = app.input.as_mut() {
            i.value.clear();
        }
        app.input_push('8');
        app.input_push('8');
        app.input_backspace();
        app.input_push('1');
        let (kind, value) = app.input_take().unwrap();
        assert_eq!(kind, InputKind::PingHost);
        assert_eq!(value, "81");
        assert!(app.input.is_none());
    }

    #[test]
    fn lite_is_a_toggle_and_sections_leave_it() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.toggle_lite();
        assert_eq!(app.view, View::Lite);
        app.toggle_lite();
        assert_eq!(
            app.view,
            View::Full,
            "second `l` returns to where we came from"
        );

        // coming from Dense must return to Dense, not Full
        app.view = View::Dense;
        app.toggle_lite();
        app.toggle_lite();
        assert_eq!(app.view, View::Dense);

        // a digit always wins over a stripped view
        app.view = View::Lite;
        app.select_tab(Tab::Bands);
        assert_eq!(app.view, View::Full);
        assert_eq!(app.tab, Tab::Bands);

        app.view = View::Dense;
        app.select_tab(Tab::Signal);
        assert_eq!(app.view, View::Full);
    }

    #[test]
    fn cycling_views_remembers_the_return_point() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.cycle_view();
        assert_eq!(app.view, View::Gauges);
        app.toggle_lite();
        assert_eq!(app.view, View::Lite);
        app.toggle_lite();
        assert_eq!(app.view, View::Gauges, "returns to the view before Lite");
    }

    #[test]
    fn signal_panel_resolver_never_invents_5g() {
        let mut snap = Snapshot::default();
        assert_eq!(resolve_signal_panel(Some(&snap)), SignalPanel::LteGauges);

        snap.connection.endc = true;
        assert_eq!(resolve_signal_panel(Some(&snap)), SignalPanel::NrBars);

        snap.connection.endc = false;
        snap.connection.nr_bars = 3;
        assert_eq!(resolve_signal_panel(Some(&snap)), SignalPanel::NrBars);

        // Only real NR metrics promote the panel to full 5G gauges.
        snap.signal.nr = Some(crate::model::Nr {
            rsrp_dbm: Some(-88.0),
            ..Default::default()
        });
        assert_eq!(resolve_signal_panel(Some(&snap)), SignalPanel::NrGauges);

        assert_eq!(resolve_signal_panel(None), SignalPanel::LteGauges);
    }

    #[test]
    fn signal_history_caps_at_120() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        for i in 0..(HISTORY + 25) {
            let mut s = Snapshot::default();
            s.signal.rsrp_dbm = Some(-100.0 + i as f32 * 0.1);
            app.on_snapshot(s, 1);
        }
        assert_eq!(app.history_rsrp.len(), HISTORY);
        assert!(app.history_rsrq.is_empty(), "missing fields are not pushed");
    }

    #[test]
    fn restart_needs_the_exact_word() {
        assert!(restart_allowed("REBOOT"));
        assert!(!restart_allowed("reboot"));
        assert!(!restart_allowed("REBOOT "));
        assert!(!restart_allowed("yes"));
        assert!(!restart_allowed(""));
    }

    #[test]
    fn event_log_is_bounded_and_ordered() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        for i in 0..250 {
            app.log(LogLevel::Info, format!("event {i}"));
        }
        assert_eq!(app.event_log.len(), 200);
        assert_eq!(app.event_log.front().unwrap().msg, "event 249");
    }

    #[test]
    fn extras_mark_blocked_inbox_as_warning() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.mark_extras_requested();
        assert!(!app.needs_extras());
        let extras = crate::model::DeviceExtras {
            sms_error: Some("permission denied (100005)".into()),
            ..Default::default()
        };
        app.on_extras(extras);
        assert!(app.extras.is_some());
        assert!(!app.extras_requested, "a reply clears the in-flight flag");
        assert!(matches!(
            app.event_log.front().unwrap().level,
            LogLevel::Warn
        ));
        assert!(app.event_log.front().unwrap().msg.contains("100005"));
    }

    #[test]
    fn settings_adjustments_are_clamped() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.adjust_refresh(-5000);
        assert_eq!(app.refresh_ms, 500);
        app.adjust_refresh(50_000);
        assert_eq!(app.refresh_ms, 10_000);
        app.adjust_budget(-500.0);
        assert_eq!(app.budget_gb, 1.0);
    }

    #[test]
    fn session_labels_are_human() {
        assert_eq!(SessionState::Live.label(), "live");
        assert!(SessionState::Recovering {
            attempt: 2,
            wait_s: 5
        }
        .label()
        .contains("stabilize"));
    }

    #[test]
    fn a_confirmed_band_write_carries_its_preflight() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.snap = Some(crate::model::Snapshot::default());
        app.bands_selected = vec![1, 3];
        assert!(app.begin_confirm());
        let p = app.preflight().expect("preflight for a confirming write");
        assert!(p
            .rows
            .iter()
            .any(|(k, v)| k == "bands after" && v == "B1,B3"));
        assert!(p.warning.unwrap_or_default().contains("offline"));
    }
}
