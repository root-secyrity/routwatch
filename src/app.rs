//! Application state for the TUI. Pure data + transitions: no I/O here, so a
//! test can build an `App` from a fixture and render it.

use crate::config::BandPreset;
use crate::headless::{Phase, PhaseState};
use crate::model::{Client, DeviceExtras, SmsMessage, Snapshot};
use crate::router::bands;
use crate::theme::{GraphStyle, Theme};
use crate::worker::{PingOutcome, SpeedRun};
use crate::write::{parse_dns_pair, WriteIntent, WriteState};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Samples kept per series. The dashboard and the signal graphs draw a fixed
/// 240-second window, and the poll interval can be as short as 500 ms, so the
/// store has to hold 480 samples to cover that window at the finest interval
/// instead of declaring a window it cannot fill: 5 series x 480 x 8 B = 19.2 KB.
pub const HISTORY: usize = 480;

/// The time window every plot on the dashboard draws, in seconds. One window
/// for the whole screen: the cell granularity adapts, the window never does.
pub const WINDOW_SECS: u64 = 240;

/// The Bands write log keeps this many settled operations in memory. It is a
/// session record, not a file: nothing is written to disk and a restart forgets
/// it, which is why the panel says "session only".
pub const BAND_LOG_CAP: usize = 32;

/// How long the status bar keeps offering the revert after a band write lands.
/// A radio that restarts takes a while to show the damage, so the offer outlives
/// the banner: the countdown writes nothing when it ends.
pub const WATCH_SECS: u64 = 180;

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

    /// Short label for narrow terminals, where nine full titles do not fit.
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
            Tab::Dashboard => {
                "j/k · enter open · s sort · f filter · v view · r refresh · P pause · e report"
            }
            Tab::Bands => {
                "space tick · s save · a apply · u undo · j/k · n preset · x del · m mode · o nr · e report"
            }
            Tab::Traffic => "d edit DNS · D auto DNS · b reset budget · e report",
            Tab::Network => "p ping host · T speed test · e report",
            Tab::Clients => "j/k select · s sort · c copy mac · e report",
            Tab::Sms => "j/k · Enter open · space mark · a all · d delete · m read · n page · R inbox",
            Tab::Device => "x dump json · b restart · R extras",
            Tab::Dns => "j/k choose · Enter apply · d type · D auto · e report",
            Tab::Signal => "v view · e report",
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
            SessionState::Restarting => "restarting - waiting for the router to stabilize…".into(),
            SessionState::Recovering { attempt, wait_s } => {
                format!("waiting for router to stabilize… #{attempt} in {wait_s}s")
            }
            SessionState::Disconnected(e) => format!("disconnected: {e}"),
        }
    }

    /// The same state in a header slot: the full label is a sentence
    /// (`restarting - waiting for the router to stabilize…`, 49 columns) and
    /// would push the PLMN out of a normal terminal.
    pub fn short(&self) -> String {
        match self {
            SessionState::Connecting => "connecting".into(),
            SessionState::Live => "live".into(),
            SessionState::Restarting => "restarting".into(),
            SessionState::Recovering { attempt, wait_s } => {
                format!("recovering #{attempt} {wait_s}s")
            }
            SessionState::Disconnected(_) => "disconnected".into(),
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

/// Ready-made rows so changing DNS does not require typing addresses - plus a
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
///
/// One real NR reading is enough for the gauge panel. A firmware that fills
/// only `nrearfcn` or the bandwidths keeps the bars panel, which now names the
/// fields it did get instead of dropping them.
pub fn resolve_signal_panel(snap: Option<&Snapshot>) -> SignalPanel {
    let Some(s) = snap else {
        return SignalPanel::LteGauges;
    };
    let has_nr_reading = s
        .signal
        .nr
        .as_ref()
        .map(|n| n.rsrp_dbm.is_some() || n.rsrq_db.is_some() || n.sinr_db.is_some())
        .unwrap_or(false);
    if has_nr_reading {
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

/// The client rows in display order, kept between frames. A frame reuses the
/// order unless the snapshot or the sort key moved, so the same rows cost one
/// cheap pass instead of a sort per row drawn.
#[derive(Debug, Default)]
pub struct DerivedCache {
    key: Option<(u64, SortKey)>,
    order: Vec<usize>,
    /// Frames that found the order already computed. Read by the tests.
    pub hits: usize,
    /// Times the order had to be computed since boot. Read by the tests.
    pub rebuilds: usize,
}

impl DerivedCache {
    fn ensure(&mut self, clients: &[Client], sort: SortKey) -> &[usize] {
        let fp = client_fp(clients);
        if matches!(self.key, Some((k, s)) if k == fp && s == sort) {
            self.hits += 1;
            return &self.order;
        }
        let mut order: Vec<usize> = (0..clients.len()).collect();
        match sort {
            SortKey::Name => order.sort_by_key(|&i| {
                let name = clients[i].name.clone().unwrap_or_default().to_lowercase();
                // unnamed hosts sort last: they are the least useful rows
                (name.is_empty(), name)
            }),
            SortKey::Ip => order.sort_by_key(|&i| ip_key(clients[i].ip.as_deref().unwrap_or(""))),
            SortKey::Band => order.sort_by_key(|&i| clients[i].freq.clone().unwrap_or_default()),
            SortKey::Time => order.sort_by_key(|&i| std::cmp::Reverse(clients[i].associated_secs)),
        }
        self.order = order;
        self.key = Some((fp, sort));
        self.rebuilds += 1;
        &self.order
    }
}

/// What the order depends on: the rows themselves, not just how many there are.
/// Every field the sort reads is in here, so a changed row cannot hide behind a
/// matching length.
fn client_fp(clients: &[Client]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    clients.len().hash(&mut h);
    for c in clients {
        c.name.hash(&mut h);
        c.ip.hash(&mut h);
        c.freq.hash(&mut h);
        c.associated_secs.hash(&mut h);
    }
    h.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Ip,
    Band,
    Time,
}

/// Which clients the dashboard table shows. The radio section draws one band at
/// a time, so the same filter narrows the client rows under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashFilter {
    All,
    Only5,
    Only24,
}

impl DashFilter {
    pub fn next(&self) -> DashFilter {
        match self {
            DashFilter::All => DashFilter::Only5,
            DashFilter::Only5 => DashFilter::Only24,
            DashFilter::Only24 => DashFilter::All,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            DashFilter::All => "all",
            DashFilter::Only5 => "5G",
            DashFilter::Only24 => "2.4G",
        }
    }

    pub fn matches(&self, client: &Client) -> bool {
        let band = client.freq.as_deref().unwrap_or("");
        match self {
            DashFilter::All => true,
            DashFilter::Only5 => band.contains('5'),
            DashFilter::Only24 => band.contains("2.4"),
        }
    }
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
    /// Seconds since the app started - no clock dependency, still ordered.
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

/// What the last settled apply did, for the `last` row. Session only: a restart
/// loses it, which is honest because the router keeps the change, not the story.
#[derive(Debug, Clone)]
pub struct LastApply {
    pub bands: Vec<u8>,
    pub mode: String,
    /// Display text: `verified` or `verified · router 100006`. Derived once, at
    /// settle time, so the render does not parse English.
    pub verdict: String,
    pub at: Instant,
}

/// One settled band operation, kept for the Bands log panel. `verdict` is the
/// text the write layer produced, so the render shows the same words the header
/// banner used instead of parsing English.
#[derive(Debug, Clone)]
pub struct BandLog {
    pub at: Instant,
    pub what: String,
    pub verdict: String,
    pub ok: bool,
}

/// Rows a page holds. Re-exported so the reader and the request body cannot
/// drift: the number is the firmware's `ReadCount`, not a second opinion.
pub use crate::router::client::SMS_PAGE;

/// What the SMS tab holds between frames. Everything here is the reader's own
/// state; the messages themselves live in `extras.sms_inbox`.
#[derive(Debug, Clone, Default)]
pub struct SmsState {
    /// Row the reader is on, within the current page.
    pub cursor: usize,
    /// Message indices marked for a bulk operation.
    pub selected: std::collections::BTreeSet<String>,
    /// Index of the message whose body is open in the lower pane. Opening is a
    /// read: nothing is written and nothing is marked read.
    pub open: Option<String>,
    /// Rows scrolled inside the open body.
    pub scroll: u16,
    /// 1-based page requested from the firmware.
    pub page: u32,
    /// Inbox total as the device answered it (sms-count), so paging and the
    /// range label do not depend on the snapshot's counters being current.
    pub total: Option<u32>,
    /// Set when the firmware refused delete or set-read: the keys stay off and
    /// the bar says why, because a refusal is not a retry.
    pub blocked: Option<String>,
    /// Last outcome line for this tab, memory only.
    pub notice: Option<String>,
    /// The question on screen, waiting for `y`.
    pub confirm: Option<SmsPlan>,
    /// A delete or mark-read is in flight; the keys wait for its report.
    pub busy: bool,
}

impl SmsState {
    pub fn page_or_one(&self) -> u32 {
        if self.page == 0 {
            1
        } else {
            self.page
        }
    }
}

/// A committed operation, held only until the user confirms it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsPlan {
    pub op: crate::write::SmsOp,
    pub indices: Vec<String>,
}

impl SmsPlan {
    pub fn label(&self) -> String {
        let verb = match self.op {
            crate::write::SmsOp::Delete => "delete",
            crate::write::SmsOp::MarkRead => "mark read",
        };
        if self.indices.len() == 1 {
            format!("{verb} 1 message (index {})", self.indices[0])
        } else {
            format!("{verb} {} messages", self.indices.len())
        }
    }
}

/// What the device carried before a pending apply, whole. Captured at the
/// confirmation, because the snapshot that supplied it is replaced by the next
/// poll long before the verdict arrives.
#[derive(Debug, Clone, Default)]
pub struct UndoPoint {
    pub net_mode: String,
    pub bands: Vec<u8>,
    pub mask: String,
    pub option: Option<String>,
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
    /// When each tier last answered, from the worker's event. Empty on a
    /// replayed frame, which has no live read behind it.
    pub stamps: crate::worker::TierStamp,
    /// Set by anything that changes what a frame would draw, cleared when the
    /// frame is drawn. The loop repaints on this instead of on every iteration.
    pub needs_draw: bool,
    /// The radio stamp of the sample already in the history rings, so a clients
    /// or device tick does not push a second copy of the same signal.
    pub last_sample_at: Option<std::time::Instant>,
    /// The client rows in display order, kept between frames. Drawing reads it
    /// instead of sorting, so a frame costs the same for ten clients as for a
    /// hundred.
    pub derived: std::cell::RefCell<DerivedCache>,
    pub session: SessionState,
    pub paused: bool,
    pub quit: bool,
    pub help: bool,
    pub client_sel: usize,
    pub error_log: VecDeque<String>,
    /// Incidents derived from the logs on disk. Read at start and on a slow
    /// timer, never per snapshot: they are a reading of history, not live state.
    pub incidents: Vec<crate::incidents::Incident>,
    pub budget_gb: f64,
    /// Local ledger plus the budget baseline. Filled by `on_snapshot` and
    /// flushed by the TUI loop; `preview` and `status` never write it.
    pub usage: crate::usage::Usage,
    /// Where the TUI flushes it. `None` outside the TUI.
    pub usage_path: Option<std::path::PathBuf>,
    pub usage_dirty: bool,
    pub usage_saved: Option<Instant>,
    /// The exclusive claim on the ledger; held for the whole TUI session.
    pub usage_lock: Option<std::fs::File>,
    /// `b` on the Traffic tab asks here first; a reset is not undoable.
    pub reset_confirm: bool,
    pub refresh_ms: u64,
    /// `true` writes serial/IMEI/WAN/client identities into `e` reports. Off by
    /// default: a report is meant to be pasted in public.
    pub report_identity: bool,
    /// What `headless::collect_traced` has read so far in the current scan.
    pub phases: [PhaseState; 10],
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
    /// The device mask the pending write will replace, captured at confirm time.
    pending_undo: Option<Vec<u8>>,
    /// The same point with mode and option, taken at the confirmation.
    pending_undo_full: Option<UndoPoint>,
    /// True once the worker reported `Pending`: a no-op success never sets it,
    /// and that is what separates "sent and verified" from "already active"
    /// without reading the note's English.
    write_sent: bool,
    /// How long the link is watched after an apply lands. The user pressed a
    /// key that restarted the radio, so the tool keeps saying so, with the key
    /// that undoes it, for as long as a revert is still useful.
    pub watch_until: Option<Instant>,
    /// The mask `u` puts back in the tick list. None until an apply lands.
    pub undo_mask: Option<Vec<u8>>,
    /// The same point with its mode and option, which is what `u` writes back.
    pub undo_full: Option<UndoPoint>,
    pub last_apply: Option<LastApply>,
    /// Newest first, capped at `BAND_LOG_CAP`, memory only.
    pub band_log: VecDeque<BandLog>,
    /// SMS reader: cursor, marks, the open message and the pending operation.
    pub sms: SmsState,
    bands_initialised: bool,
    // --- M4 ---
    pub input: Option<Input>,
    pub ping: Option<PingOutcome>,
    pub client_sort: SortKey,
    /// Row under the cursor in the dashboard table, which is filtered, so it
    /// cannot share `client_sel` with the Clients tab's unfiltered list.
    pub dash_sel: usize,
    pub dash_filter: DashFilter,
    pub notice: Option<(String, Instant)>,
    pub dns_preset: usize,
    // --- M5 ---
    pub extras: Option<DeviceExtras>,
    pub extras_requested: bool,
    pub event_log: VecDeque<LogEntry>,
    /// The events feed: a window over `event_log` that can hold its place.
    pub events_open: bool,
    pub events_paused: bool,
    /// Index counted from the newest line, so appends never shift it.
    pub events_back: usize,
    pub settings_open: bool,
    /// Where `w` writes the settings (set by the CLI, never guessed here).
    pub config_path: Option<std::path::PathBuf>,
    // --- speed test ---
    pub speed_cfg: crate::config::Speedtest,
    pub speed_running: bool,
    pub speed_confirm: bool,
    /// Incremented per run; every event carries the id it belongs to.
    pub speed_run: u64,
    /// Replaced on every run, so one run's cancellation cannot reach another.
    pub speed_cancel: std::sync::Arc<AtomicBool>,
    /// When the current test started, for the watchdog only.
    pub speed_started: Option<Instant>,
    pub speed: Option<crate::speedtest::SpeedResult>,
    pub speed_progress: Option<crate::speedtest::SpeedProgress>,
    pub speed_error: Option<String>,
    pub speed_context: Option<SpeedContext>,
    speed_band_at_start: Option<String>,
    speed_peak_dl: u64,
    speed_peak_ul: u64,
    // --- speed history ---
    /// Newest first, capped at `history::SHOWN`.
    pub history: Vec<crate::history::Entry>,
    pub history_total: usize,
    pub history_warn: Option<String>,
    pub history_writable: bool,
    pub history_path: Option<std::path::PathBuf>,
    started: Instant,
}

/// The radio values around a test, so a number is never shown without the
/// signal that carried it, and a band change mid-test cannot hide.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpeedContext {
    pub net_type: String,
    /// The band when the test started, and the band when it finished. They
    /// differ when the CPE moved during the window, and both are shown.
    pub band_at_start: Option<String>,
    pub band: String,
    pub rsrp_dbm: Option<f32>,
    pub rsrq_db: Option<f32>,
    pub sinr_db: Option<f32>,
    /// Peak rates the device itself reported while the test ran, in bytes per
    /// second (`src/model.rs:74`). A consistency check, not proof.
    pub router_peak_dl: u64,
    pub router_peak_ul: u64,
}

impl App {
    /// One place decides how every graph is drawn. A series is one flat
    /// colour: the plot never synthesises a shade, so nothing has to be
    /// measured against an assumed background.
    pub fn chart_opts(&self) -> crate::ui::chart::ChartOpts {
        crate::ui::chart::ChartOpts {
            ascii: self.graph.is_ascii(),
            baseline: true,
        }
    }

    /// Choosing a section leaves a stripped view: Lite, Dense and Gauges are
    /// single-screen summaries that ignore `tab`, so a digit there must still
    /// select one.
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
            stamps: crate::worker::TierStamp::default(),
            needs_draw: true,
            last_sample_at: None,
            derived: std::cell::RefCell::new(DerivedCache::default()),
            session: SessionState::Connecting,
            paused: false,
            quit: false,
            help: false,
            client_sel: 0,
            error_log: VecDeque::new(),
            incidents: Vec::new(),
            budget_gb: budget_gb as f64,
            usage: crate::usage::Usage::default(),
            usage_path: None,
            usage_dirty: false,
            usage_saved: None,
            usage_lock: None,
            reset_confirm: false,
            refresh_ms,
            report_identity: false,
            phases: [PhaseState::Waiting; 10],
            bands_supported: Vec::new(),
            bands_selected: Vec::new(),
            band_cursor: 0,
            band_mode: "00".into(),
            band_option: "0".into(),
            band_presets: crate::config::default_presets(),
            preset_cursor: None,
            write: WriteState::Idle,
            allow_write: false,
            pending_undo: None,
            pending_undo_full: None,
            write_sent: false,
            undo_mask: None,
            last_apply: None,
            band_log: VecDeque::new(),
            sms: SmsState::default(),
            undo_full: None,
            watch_until: None,
            bands_initialised: false,
            input: None,
            ping: None,
            client_sort: SortKey::Name,
            dash_sel: 0,
            dash_filter: DashFilter::All,
            notice: None,
            dns_preset: 0,
            extras: None,
            extras_requested: false,
            event_log: VecDeque::new(),
            events_open: false,
            events_paused: false,
            events_back: 0,
            settings_open: false,
            config_path: None,
            speed_cfg: crate::config::Speedtest::default(),
            speed_running: false,
            speed_confirm: false,
            speed_run: 0,
            speed_cancel: std::sync::Arc::new(AtomicBool::new(false)),
            speed_started: None,
            speed: None,
            speed_progress: None,
            speed_error: None,
            speed_context: None,
            speed_band_at_start: None,
            speed_peak_dl: 0,
            speed_peak_ul: 0,
            history: Vec::new(),
            history_total: 0,
            history_warn: None,
            history_writable: false,
            history_path: crate::history::path(),
            started: Instant::now(),
        }
    }

    pub fn on_snapshot(&mut self, snap: Snapshot, poll_ms: u128) {
        self.needs_draw = true;
        // A different router means different counters, not a different user:
        // keep the daily table, restart the baseline, and record the change as
        // a gap. Resetting the whole ledger here lost a month of history.
        if !self.usage.router.is_empty() && self.usage.router != snap.router {
            crate::usage::switch_router(
                &mut self.usage,
                snap.generated_at,
                &snap.traffic,
                snap.quota.as_ref(),
            );
        }
        self.usage.router = snap.router.clone();
        let start_day = snap.quota.as_ref().map_or(1, |q| q.start_day);
        self.usage_dirty |= crate::usage::observe(
            &mut self.usage,
            snap.generated_at,
            &snap.traffic,
            snap.quota.as_ref(),
            start_day,
        );
        // One sample per radio read, not one per snapshot: with tiers the client
        // and device ticks carry the same signal value again, and pushing them
        // would both duplicate the sample and break the spacing the plots assume.
        let radio_at = self.stamps.get(crate::worker::Tier::Radio);
        let new_sample = radio_at.is_none() || self.last_sample_at != radio_at;
        if new_sample {
            self.last_sample_at = radio_at;
            self.history_dl.push_back(snap.traffic.dl_rate_bytes as f64);
            self.history_ul.push_back(snap.traffic.ul_rate_bytes as f64);
            while self.history_dl.len() > HISTORY {
                self.history_dl.pop_front();
            }
            while self.history_ul.len() > HISTORY {
                self.history_ul.pop_front();
            }
        }
        // What the device itself reported while a test ran: a consistency check
        // on the number the test measured, not proof of it.
        if self.speed_running {
            self.speed_peak_dl = self.speed_peak_dl.max(snap.traffic.dl_rate_bytes);
            self.speed_peak_ul = self.speed_peak_ul.max(snap.traffic.ul_rate_bytes);
        }
        let push = |q: &mut VecDeque<f64>, v: Option<f32>| {
            if let Some(x) = v {
                q.push_back(x as f64);
                while q.len() > HISTORY {
                    q.pop_front();
                }
            }
        };
        if new_sample {
            push(&mut self.history_rsrp, snap.signal.rsrp_dbm);
            push(&mut self.history_rsrq, snap.signal.rsrq_db);
            push(&mut self.history_sinr, snap.signal.sinr_db);
        }

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
        // An unpaused feed follows the newest line; a paused one holds still,
        // otherwise `space` would be a button with no effect.
        if !self.events_paused {
            self.events_back = 0;
        }
    }

    /// Where the reader is in the log, counted from the newest line.
    pub fn event_cursor(&self) -> Option<usize> {
        if self.event_log.is_empty() {
            None
        } else {
            Some(self.events_back.min(self.event_log.len() - 1))
        }
    }

    pub fn event_select(&mut self, back: usize) {
        self.events_back = back.min(self.event_log.len().saturating_sub(1));
    }

    /// `j`/`k` in the feed. Moving is a decision to stop following: otherwise
    /// the next event would yank the reader back to the top.
    pub fn event_move(&mut self, delta: isize) {
        let max = self.event_log.len().saturating_sub(1) as isize;
        let next = (self.events_back as isize + delta).clamp(0, max.max(0));
        self.events_back = next as usize;
        self.events_paused = true;
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

    /// One inbox page arrived: the rows replace what the tab shows, and the
    /// cursor is clamped into the new list so it never points past the end.
    pub fn on_sms_page(
        &mut self,
        page: u32,
        messages: Vec<SmsMessage>,
        total: Option<u32>,
        error: Option<String>,
    ) {
        self.sms.page = page.max(1);
        if total.is_some() {
            self.sms.total = total;
        }
        let extras = self.extras.get_or_insert_with(DeviceExtras::default);
        extras.sms_inbox = messages;
        extras.sms_error = error;
        self.sms.selected.clear();
        self.sms.open = None;
        self.sms.scroll = 0;
        let len = self.sms_rows().len();
        if len == 0 {
            self.sms.cursor = 0;
        } else {
            self.sms.cursor = self.sms.cursor.min(len - 1);
        }
    }

    /// The SMS/Device tabs ask for extras once; `R` forces a refetch.
    pub fn needs_extras(&self) -> bool {
        self.extras.is_none() && !self.extras_requested
    }

    pub fn mark_extras_requested(&mut self) {
        self.extras_requested = true;
    }

    // --- SMS reader ---

    /// The rows on the current page, from the last extras read.
    pub fn sms_rows(&self) -> &[crate::model::SmsMessage] {
        match &self.extras {
            Some(e) => &e.sms_inbox,
            None => &[],
        }
    }

    /// `j`/`k`: a reader at the last row wraps to the first, so one key held
    /// down never parks on a dead end.
    pub fn sms_move(&mut self, delta: isize) {
        let len = self.sms_rows().len();
        if len == 0 {
            self.sms.cursor = 0;
            return;
        }
        let next = (self.sms.cursor as isize + delta).rem_euclid(len as isize);
        self.sms.cursor = next as usize;
    }

    fn sms_current_index(&self) -> Option<String> {
        self.sms_rows()
            .get(self.sms.cursor)
            .and_then(|m| m.index.clone())
    }

    /// `space`: mark or unmark the row under the cursor.
    pub fn sms_toggle(&mut self) -> bool {
        let Some(index) = self.sms_current_index() else {
            return false;
        };
        if !self.sms.selected.remove(&index) {
            self.sms.selected.insert(index);
        }
        true
    }

    /// `V`: mark every row on the page, or clear when they are all marked.
    pub fn sms_select_all(&mut self) -> bool {
        let indexes: Vec<String> = self
            .sms_rows()
            .iter()
            .filter_map(|m| m.index.clone())
            .collect();
        if indexes.is_empty() {
            return false;
        }
        let all_marked = indexes.iter().all(|i| self.sms.selected.contains(i));
        if all_marked {
            self.sms.selected.clear();
        } else {
            for index in indexes {
                self.sms.selected.insert(index);
            }
        }
        true
    }

    /// `Enter`/`o`: show the body. This is a pure read of state already in
    /// memory: no request, and nothing is marked read.
    pub fn sms_open(&mut self) -> bool {
        match self.sms_current_index() {
            Some(index) => {
                self.sms.open = Some(index);
                self.sms.scroll = 0;
                true
            }
            None => false,
        }
    }

    pub fn sms_close(&mut self) -> bool {
        let had = self.sms.open.is_some();
        self.sms.open = None;
        self.sms.scroll = 0;
        had
    }

    pub fn sms_scroll(&mut self, delta: i16) {
        if self.sms.open.is_none() {
            return;
        }
        let next = self.sms.scroll as i32 + delta as i32;
        self.sms.scroll = next.max(0).min(u16::MAX as i32) as u16;
    }

    /// `n`/`p`: ask for another page. The total comes from the counters, which
    /// are the only place this firmware states how many messages exist.
    pub fn sms_total(&self) -> Option<u32> {
        self.sms.total.or_else(|| {
            self.snap
                .as_ref()
                .and_then(|s| s.sms.as_ref())
                .map(|c| c.inbox)
        })
    }

    pub fn sms_page_next(&mut self) -> bool {
        let page = self.sms.page_or_one();
        let len = self.sms_rows().len();
        let more = len as u32 >= crate::router::client::SMS_PAGE;
        match self.sms_total() {
            Some(total) => {
                if page * crate::router::client::SMS_PAGE >= total {
                    return false;
                }
            }
            None if !more => return false,
            None => {}
        }
        self.sms.page = page + 1;
        self.sms.selected.clear();
        self.sms.cursor = 0;
        self.sms.open = None;
        true
    }

    pub fn sms_page_prev(&mut self) -> bool {
        let page = self.sms.page_or_one();
        if page <= 1 {
            return false;
        }
        self.sms.page = page - 1;
        self.sms.selected.clear();
        self.sms.cursor = 0;
        self.sms.open = None;
        true
    }

    /// `1-40 of 45` for the footer, or `-` when nothing is known yet.
    pub fn sms_range(&self) -> String {
        let len = self.sms_rows().len() as u32;
        if len == 0 {
            return String::new();
        }
        let start = (self.sms.page_or_one() - 1) * crate::router::client::SMS_PAGE + 1;
        let end = start + len - 1;
        match self.sms_total() {
            Some(total) => format!("{start}-{end} of {total}"),
            None => format!("{start}-{end}"),
        }
    }

    /// The indices a bulk operation would touch: the marks, or the cursor row
    /// when nothing is marked.
    fn sms_targets(&self) -> Vec<String> {
        if !self.sms.selected.is_empty() {
            return self.sms.selected.iter().cloned().collect();
        }
        self.sms_current_index().into_iter().collect()
    }

    /// Why a write is refused right now, or `None` when it may proceed. The key
    /// handler prints this, so a refused `d` says why instead of doing nothing.
    pub fn sms_plan_refusal(&self) -> Option<String> {
        if let Some(why) = &self.sms.blocked {
            return Some(why.clone());
        }
        if !self.allow_write {
            return Some("writes are off: start with --allow-write".to_string());
        }
        if self.sms.busy {
            return Some("an operation is still running".to_string());
        }
        None
    }

    /// `d`: ask before deleting. Builds the plan, never sends it.
    pub fn sms_plan_delete(&mut self, all: bool) -> Option<SmsPlan> {
        if self.sms_plan_refusal().is_some() {
            return None;
        }
        let indexes = if all {
            self.sms_rows()
                .iter()
                .filter_map(|m| m.index.clone())
                .collect::<Vec<_>>()
        } else {
            self.sms_targets()
        };
        if indexes.is_empty() {
            return None;
        }
        let plan = SmsPlan {
            op: crate::write::SmsOp::Delete,
            indices: indexes,
        };
        self.sms.confirm = Some(plan.clone());
        Some(plan)
    }

    /// `m`: mark the open (or cursor) message read. Its own key, its own gate.
    pub fn sms_plan_mark_read(&mut self) -> Option<SmsPlan> {
        if self.sms_plan_refusal().is_some() {
            return None;
        }
        let index = self.sms.open.clone().or_else(|| self.sms_current_index());
        let index = index?;
        let plan = SmsPlan {
            op: crate::write::SmsOp::MarkRead,
            indices: vec![index],
        };
        self.sms.confirm = Some(plan.clone());
        Some(plan)
    }

    /// `n` or `Esc` in the dialogue: drop the plan, send nothing.
    pub fn sms_cancel(&mut self) -> bool {
        self.sms.confirm.take().is_some()
    }

    /// `y` in the dialogue: hand the plan out, once.
    pub fn sms_take_plan(&mut self) -> Option<SmsPlan> {
        self.sms.confirm.take()
    }

    /// Take one outcome into the tab: counts in the bar, failed rows kept, and a
    /// refusal disables the keys instead of inviting a retry.
    pub fn on_sms_report(&mut self, report: &crate::write::SmsReport) {
        self.sms.busy = false;
        self.sms.notice = Some(report.summary());
        if let Some((_, code)) = &report.unsupported {
            self.sms.blocked = Some(format!(
                "this firmware refused {} ({code}); the keys stay off",
                report.op.verb()
            ));
        }
        let failed: std::collections::BTreeSet<String> =
            report.failed.iter().map(|(i, _)| i.clone()).collect();
        self.sms.selected.retain(|i| failed.contains(i));
        if report.op == crate::write::SmsOp::Delete {
            if let Some(open) = &self.sms.open {
                if !failed.contains(open) && report.deleted > 0 {
                    self.sms.open = None;
                }
            }
            self.sms.scroll = 0;
        }
        let len = self.sms_rows().len();
        if len > 0 && self.sms.cursor >= len {
            self.sms.cursor = len - 1;
        }
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

    /// The order the grid draws: low, mid, high, other, ascending inside each
    /// class. `band_cursor` indexes this, so `j`/`k` walk the picture the user
    /// is looking at, not the firmware's list order.
    /// Every band the panel can show: what the device advertises plus what the
    /// tick list holds. A ticked band the device never listed must still be
    /// drawn, because dropping it would hide the one row that says so.
    pub fn bands_in_draw_order(&self) -> Vec<u32> {
        let mut v = self.bands_supported.clone();
        for b in &self.bands_selected {
            if !v.contains(&(*b as u32)) {
                v.push(*b as u32);
            }
        }
        v.sort_by_key(|b| (bands::class_of(*b as u8), *b));
        v
    }

    /// What the device runs right now, read from its own mask. Never from the
    /// tick list: after a preset loads the two differ, and that is the point.
    pub fn bands_on_device(&self) -> Vec<u8> {
        crate::preflight::current_bands(self.snap.as_ref())
    }

    /// (added, dropped) between the device's mask and the tick list.
    pub fn bands_diff(&self) -> (Vec<u8>, Vec<u8>) {
        let now = self.bands_on_device();
        let added = self
            .bands_selected
            .iter()
            .copied()
            .filter(|b| !now.contains(b))
            .collect();
        let dropped = now
            .iter()
            .copied()
            .filter(|b| !self.bands_selected.contains(b))
            .collect();
        (added, dropped)
    }

    /// True when the tick list is exactly what the device already has.
    pub fn bands_unchanged(&self) -> bool {
        let (added, dropped) = self.bands_diff();
        added.is_empty() && dropped.is_empty()
    }

    /// The device's own bands grouped for the grid, empty classes omitted.
    pub fn bands_by_class(&self) -> Vec<(bands::BandClass, Vec<u32>)> {
        let order = self.bands_in_draw_order();
        bands::BandClass::ALL
            .iter()
            .map(|cls| {
                let members = order
                    .iter()
                    .copied()
                    .filter(|b| bands::class_of(*b as u8) == *cls)
                    .collect::<Vec<u32>>();
                (*cls, members)
            })
            .filter(|(_, members)| !members.is_empty())
            .collect()
    }

    pub fn band_toggle(&mut self) {
        if self.write.is_busy() || matches!(self.write, WriteState::Confirming(_)) {
            return;
        }
        let order = self.bands_in_draw_order();
        let Some(band) = order.get(self.band_cursor).copied() else {
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
        let len = self.bands_in_draw_order().len();
        if len == 0 {
            return;
        }
        let max = len as isize - 1;
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
            return "-".into();
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

    /// Fill the five series the charts read with `samples` synthetic points
    /// around the current reading, so `preview --demo-history` can show a full
    /// window without a router. Deterministic: the same arguments render the
    /// same frame, and the cap is the one `on_snapshot` applies.
    pub fn demo_history(&mut self, samples: usize) {
        if samples == 0 {
            return;
        }
        let mut seed = 0x2026_1118u64;
        // xorshift64: a wobble that depends on the counter, not on the clock.
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 1000) as f64 / 1000.0
        };
        let dl = self.history_dl.back().copied().unwrap_or(1.0e6).max(1.0);
        let ul = self.history_ul.back().copied().unwrap_or(1.0e5).max(1.0);
        let rsrp = self.history_rsrp.back().copied().unwrap_or(-90.0);
        let rsrq = self.history_rsrq.back().copied().unwrap_or(-8.0);
        let sinr = self.history_sinr.back().copied().unwrap_or(12.0);
        for i in 0..samples {
            let phase = i as f64 * 0.17;
            let swing = 0.25 + 0.6 * phase.sin().abs() + 0.15 * noise();
            self.history_dl.push_back(dl * swing);
            self.history_ul
                .push_back(ul * (0.5 + 0.5 * (phase * 1.7).sin().abs()));
            // Clamped to the fixed domains the signal charts draw with, so a
            // sample never falls outside the scale and leaves a hole in the row.
            self.history_rsrp.push_back(
                (rsrp + 14.0 * (phase * 0.6).sin() + 3.0 * noise()).clamp(-139.0, -45.0),
            );
            self.history_rsrq
                .push_back((rsrq + 6.0 * (phase * 0.45).cos() + 2.0 * noise()).clamp(-19.5, -3.5));
            self.history_sinr
                .push_back((sinr + 10.0 * (phase * 0.8).sin() + 3.0 * noise()).clamp(-9.0, 29.0));
        }
        for q in [
            &mut self.history_dl,
            &mut self.history_ul,
            &mut self.history_rsrp,
            &mut self.history_rsrq,
            &mut self.history_sinr,
        ] {
            while q.len() > HISTORY {
                q.pop_front();
            }
        }
    }

    /// `a` in the Bands tab: needs a non-empty selection and nothing in flight.
    pub fn begin_confirm(&mut self) -> bool {
        if self.bands_selected.is_empty() || self.write.is_busy() {
            return false;
        }
        self.pending_undo = Some(self.bands_on_device());
        // Captured from the same snapshot the tick list came from: the mode and
        // option the device carries now are half of what a revert needs.
        let nm = self
            .snap
            .as_ref()
            .map(|s| s.net_mode.clone())
            .unwrap_or_default();
        let bands = self.bands_on_device();
        self.pending_undo_full = Some(UndoPoint {
            net_mode: nm.network_mode.clone().unwrap_or_default(),
            mask: nm
                .lte_band
                .clone()
                .unwrap_or_else(|| bands::to_hex(bands::mask_from_bands(&bands))),
            option: nm.network_option.clone(),
            bands,
        });
        self.write_sent = false;
        self.write = WriteState::Confirming(self.band_intent());
        true
    }

    pub fn cancel_confirm(&mut self) {
        if matches!(self.write, WriteState::Confirming(_)) {
            self.write = WriteState::Idle;
        }
        // A cancelled confirmation never wrote anything, so there is nothing to
        // remember and nothing to put back.
        self.pending_undo = None;
        self.pending_undo_full = None;
        self.write_sent = false;
    }

    /// Record one settled band write for the session log. Ring, not file: the
    /// oldest entry goes when the cap is reached.
    pub fn note_band_op(&mut self, what: String, verdict: String, ok: bool) {
        self.band_log.push_front(BandLog {
            at: Instant::now(),
            what,
            verdict,
            ok,
        });
        while self.band_log.len() > BAND_LOG_CAP {
            self.band_log.pop_back();
        }
    }

    pub fn settle_write(&mut self, state: WriteState) {
        match &state {
            WriteState::Pending { .. } | WriteState::Verifying { .. } => {
                // The write is on the wire. Nothing is decided, so nothing is
                // cleared: clearing here would erase the undo point before the
                // verdict that needs it.
                self.write_sent = true;
            }
            WriteState::Success { intent, note } => {
                self.log(
                    LogLevel::Info,
                    format!("write ok · {} · {note}", intent.label()),
                );
                if let WriteIntent::ApplyBands { mode, bands, .. } = intent {
                    if self.write_sent {
                        self.watch_until = Some(Instant::now() + Duration::from_secs(WATCH_SECS));
                        self.undo_mask = self.pending_undo.take();
                        // The point `u` writes back is the one saved before the
                        // apply; `last_apply` describes what was applied.
                        self.undo_full = self.pending_undo_full.take();
                        self.last_apply = Some(LastApply {
                            bands: bands.clone(),
                            mode: mode.clone(),
                            verdict: if note.contains("100006") {
                                "verified · router 100006".into()
                            } else {
                                "verified".into()
                            },
                            at: Instant::now(),
                        });
                        self.note_band_op(
                            format!("apply {} bands", bands.len()),
                            note.clone(),
                            true,
                        );
                    }
                }
                self.pending_undo = None;
                self.pending_undo_full = None;
                self.write_sent = false;
            }
            WriteState::Failed { intent, err } => {
                self.log(
                    LogLevel::Error,
                    format!("write failed · {} · {err}", intent.label()),
                );
                if matches!(intent, WriteIntent::ApplyBands { .. }) {
                    self.note_band_op(intent.label().to_string(), err.clone(), false);
                }
                self.pending_undo = None;
                self.pending_undo_full = None;
                self.write_sent = false;
            }
            _ => {}
        }
        self.write = state;
    }

    /// Put the mask the device carried before the last apply back into the tick
    /// list. Never writes: `a` still goes through the confirmation.
    pub fn band_undo(&mut self) -> Option<String> {
        if self.write.is_busy() || matches!(self.write, WriteState::Confirming(_)) {
            return None;
        }
        let mask = self.undo_mask.clone()?;
        let keep: Vec<u8> = mask
            .into_iter()
            .filter(|b| self.bands_supported.contains(&(*b as u32)))
            .collect();
        if keep.is_empty() {
            return None;
        }
        self.bands_selected = keep;
        // The highlighted preset no longer describes the tick list.
        self.preset_cursor = None;
        Some(format!(
            "undo → 0x{} ({} bands)",
            self.band_mask_hex(),
            self.bands_selected.len()
        ))
    }

    /// The payload that puts the device back where it was before the last
    /// apply, built from the four values saved at the confirmation. The mode
    /// decides whether `NetworkBand` goes out as the band mask (auto) or as the
    /// firmware's own all-bands mask: sending the mask under mode `00` narrows
    /// the network instead of restoring it.
    pub fn undo_payload(&self) -> Option<crate::router::bands::BandPayload> {
        let last = self.undo_full.as_ref()?;
        let all = self.snap.as_ref().and_then(|s| {
            s.network_bands_mask
                .clone()
                .or_else(|| s.all_bands_mask.clone())
        });
        let network_band = if last.net_mode == "00" {
            all.unwrap_or_else(|| last.mask.clone())
        } else {
            last.mask.clone()
        };
        Some(crate::router::bands::BandPayload {
            network_mode: last.net_mode.clone(),
            network_band,
            lte_band: last.mask.clone(),
            network_option: last.option.clone(),
        })
    }

    /// The same four values as an intent, so the confirmation dialog, the log
    /// line and the revert all speak about one write.
    pub fn undo_intent(&self) -> Option<WriteIntent> {
        let last = self.undo_full.as_ref()?;
        Some(WriteIntent::ApplyBands {
            mode: last.net_mode.clone(),
            bands: last.bands.clone(),
            mask: last.mask.clone(),
            option: last.option.clone(),
        })
    }

    /// Seconds left in the watch window, measured against `now` so a test can
    /// stand at any point in it. `None` once the window is over or was never
    /// opened: an expired window writes nothing at all.
    pub fn watch_secs_left(&self, now: Instant) -> Option<u64> {
        let until = self.watch_until?;
        let left = until.saturating_duration_since(now);
        (!left.is_zero()).then(|| left.as_secs() + 1)
    }

    /// The status-bar sentence, or nothing when no window is open.
    pub fn watch_line(&self) -> Option<String> {
        self.watch_line_at(Instant::now())
    }

    /// The same sentence measured against an injected clock, so the expired
    /// state is testable without waiting three minutes.
    pub fn watch_line_at(&self, now: Instant) -> Option<String> {
        let left = self.watch_secs_left(now)?;
        Some(format!("watching {left}s, u reverts"))
    }

    /// How old the displayed data is - the header turns this into `live`/`stale`.
    pub fn age(&self) -> Option<Duration> {
        self.last_ok.map(|t| t.elapsed())
    }

    /// How old the data of one tier is. `None` until that tier has answered, so
    /// the header can say nothing instead of saying zero.
    pub fn age_for(&self, tier: crate::worker::Tier) -> Option<Duration> {
        self.stamps.get(tier).map(|t| t.elapsed())
    }

    /// The frame that was just drawn is the frame the terminal holds.
    pub fn mark_drawn(&mut self) {
        self.needs_draw = false;
    }

    /// A key changed what the screen shows. The loop draws once per pass of the
    /// input queue, so a burst of keys still costs one frame, and no frame
    /// waits for the one second pulse that exists for the age text.
    pub fn mark_dirty(&mut self) {
        self.needs_draw = true;
    }

    pub fn stale(&self) -> bool {
        // Three periods, but never less than three seconds: the signal read is
        // bimodal on this firmware (measured 2026-11-19: p95 1575 ms, max
        // 1640 ms), so a 600 ms period with a bare 1.8 s rule would flag the
        // firmware's own tail as a dead session.
        let floor = self.refresh_ms.max(2_000);
        self.age()
            .map(|a| a.as_millis() as u64 > floor.saturating_mul(3))
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

    /// Clients in display order. Sorting never touches the snapshot itself, and
    /// the order is kept between frames: see `DerivedCache`.
    pub fn sorted_clients(&self) -> Vec<&Client> {
        let Some(s) = &self.snap else {
            return Vec::new();
        };
        let mut cache = self.derived.borrow_mut();
        let order = cache.ensure(&s.clients, self.client_sort);
        order.iter().filter_map(|&i| s.clients.get(i)).collect()
    }

    /// How many rows the display order holds, without building the list.
    pub fn sorted_len(&self) -> usize {
        let Some(s) = &self.snap else {
            return 0;
        };
        let mut cache = self.derived.borrow_mut();
        cache.ensure(&s.clients, self.client_sort).len()
    }

    pub fn selected_client(&self) -> Option<usize> {
        let len = self.sorted_len();
        if len == 0 {
            None
        } else {
            Some(self.client_sel.min(len - 1))
        }
    }

    pub fn client_cursor(&mut self, delta: isize) {
        let len = self.sorted_len() as isize;
        if len == 0 {
            return;
        }
        self.client_sel = (self.client_sel as isize + delta).clamp(0, len - 1) as usize;
    }

    pub fn sort_next(&mut self) {
        self.client_sort = self.client_sort.next();
        self.client_sel = 0;
    }

    /// The dashboard table's rows: the client list in display order, narrowed by
    /// the dashboard filter only.
    pub fn dash_clients(&self) -> Vec<&Client> {
        self.sorted_clients()
            .into_iter()
            .filter(|c| self.dash_filter.matches(c))
            .collect()
    }

    pub fn dash_cursor(&mut self, delta: isize) {
        let len = self.dash_clients().len() as isize;
        if len == 0 {
            self.dash_sel = 0;
            return;
        }
        self.dash_sel = (self.dash_sel as isize + delta).clamp(0, len - 1) as usize;
    }

    pub fn dash_filter_next(&mut self) {
        self.dash_filter = self.dash_filter.next();
        self.dash_sel = 0;
    }

    /// Open the row under the dashboard cursor in its own section, with that
    /// client still selected there. `false` when the table is empty.
    pub fn dash_open_client(&mut self) -> bool {
        let Some(ip) = self
            .dash_clients()
            .get(self.dash_sel)
            .and_then(|c| c.ip.clone())
        else {
            return false;
        };
        if let Some(i) = self
            .sorted_clients()
            .iter()
            .position(|c| c.ip.as_ref() == Some(&ip))
        {
            self.client_sel = i;
        }
        self.tab = Tab::Clients;
        true
    }

    /// One endpoint read finished (or started). A new scan is announced by the
    /// first phase starting again, which clears the previous scan's `Done`s.
    pub fn on_phase(&mut self, phase: Phase, state: PhaseState) {
        if state == PhaseState::Running && phase == Phase::ALL[0] {
            self.phases = [PhaseState::Waiting; 10];
        }
        self.phases[phase.index()] = state;
    }

    /// `(done, total)` for the current scan.
    pub fn phase_progress(&self) -> (usize, usize) {
        let done = self
            .phases
            .iter()
            .filter(|state| **state == PhaseState::Done)
            .count();
        (done, Phase::ALL.len())
    }

    /// The phase being read right now, if any.
    pub fn current_phase(&self) -> Option<Phase> {
        Phase::ALL
            .iter()
            .copied()
            .find(|phase| self.phases[phase.index()] == PhaseState::Running)
    }

    pub fn is_scanning(&self) -> bool {
        self.current_phase().is_some()
    }

    /// Which report mode `e` will write. Never defaults to `full`.
    pub fn report_redaction(&self) -> crate::report::Redaction {
        crate::report::Redaction {
            identity: self.report_identity,
        }
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), Instant::now()));
    }

    /// The UI never holds an unbounded limit: the confirmation modal and the
    /// run itself read the same clamped values.
    pub fn set_speed_cfg(&mut self, cfg: crate::config::Speedtest) {
        self.speed_cfg = cfg.sanitised();
    }

    /// `T` asks once: the test spends the user's data.
    pub fn begin_speed_test(&mut self) -> bool {
        if self.speed_running {
            self.note("a speed test is already running");
            return false;
        }
        self.speed_confirm = true;
        true
    }

    /// Starts a run and hands back what the worker needs: the id to tag its
    /// events with, the clamped config, and a flag no earlier run can touch.
    pub fn start_speed_test(
        &mut self,
    ) -> (u64, crate::config::Speedtest, std::sync::Arc<AtomicBool>) {
        self.speed_confirm = false;
        self.speed_run += 1;
        self.speed_cancel = std::sync::Arc::new(AtomicBool::new(false));
        self.speed_running = true;
        self.speed_started = Some(Instant::now());
        self.speed_error = None;
        self.speed_peak_dl = 0;
        self.speed_peak_ul = 0;
        self.speed_band_at_start = self.snap.as_ref().and_then(|s| s.signal.band.clone());
        self.speed_progress = Some(crate::speedtest::SpeedProgress {
            phase: crate::speedtest::Phase::Download,
            bytes: 0,
            rate_bps: 0.0,
        });
        (
            self.speed_run,
            self.speed_cfg.clone(),
            std::sync::Arc::clone(&self.speed_cancel),
        )
    }

    /// `Esc` during a run: both readers stop at their next chunk, and the
    /// thread still reports what it already measured.
    pub fn cancel_speed_test(&mut self) {
        if self.speed_running {
            self.speed_cancel.store(true, Ordering::Relaxed);
            self.note("stopping the speed test");
        }
    }

    /// A thread that dies without answering must not leave the panel spinning.
    /// The budget is `speedtest::worst_case_secs`, the same sum the agent's
    /// timeouts are built from: a shorter one would call a live thread dead.
    pub fn speed_watchdog(&mut self) {
        let Some(started) = self.speed_started else {
            return;
        };
        let limit = Duration::from_secs(crate::speedtest::worst_case_secs(&self.speed_cfg));
        if self.speed_running && started.elapsed() > limit {
            self.speed_running = false;
            self.speed_started = None;
            self.speed_progress = None;
            let msg = "the test stopped answering; try again".to_string();
            self.speed_error = Some(msg.clone());
            self.note(msg);
        }
    }

    pub fn on_speed_progress(&mut self, run: u64, p: crate::speedtest::SpeedProgress) {
        if self.speed_running && run == self.speed_run {
            self.speed_progress = Some(p);
        }
    }

    pub fn on_speed_done(&mut self, run: u64, out: Result<SpeedRun, String>) {
        if run != self.speed_run {
            // A run that was replaced by the watchdog still reports; its
            // numbers are stale by definition.
            return;
        }
        self.speed_running = false;
        self.speed_started = None;
        self.speed_progress = None;
        match out {
            Ok(SpeedRun {
                result: mut r,
                elapsed,
                pings,
            }) => {
                if self.speed_peak_dl > 0 {
                    let seen =
                        crate::speedtest::rate_bps(self.speed_peak_dl, Duration::from_secs(1));
                    if r.download_bps > 20.0 * seen {
                        // Formatted here, not through the UI helper: `app` must
                        // not depend on a render module to build a note.
                        r.note = Some(format!(
                            "the device counter did not see this rate: {:.1} Mbps against {:.1} Mbps it reported",
                            r.download_bps / 1e6,
                            seen / 1e6
                        ));
                    }
                }
                let label = if r.partial {
                    "speed test, partial"
                } else {
                    "speed test"
                };
                // `*_bps` is bits per second, so this division is the Mbps one.
                self.note(format!(
                    "{label}: down {:.1} Mbps · up {:.1} Mbps",
                    r.download_bps / 1e6,
                    r.upload_bps / 1e6
                ));
                self.log(LogLevel::Info, "speed test finished");
                // The context first: the row records the radio this run saw,
                // and `record_speed` reads it from the same place the UI does.
                self.speed_context = self.radio_now();
                self.record_speed(&r, elapsed, &pings);
                self.speed = Some(r);
            }
            Err(e) => {
                self.speed_error = Some(e.clone());
                self.log(LogLevel::Warn, format!("speed test failed: {e}"));
                self.note(format!("speed test failed: {e}"));
            }
        }
    }

    /// Re-derive the incident list from the logs on disk. Pure derivation: this
    /// reads files and writes nothing, so calling it while the TUI runs is safe.
    pub fn load_incidents(&mut self) {
        self.incidents = crate::incidents::detect(&crate::incidents::Source::load());
    }

    /// Read the ledger and trim it. Called by the TUI only, never by `App::new`,
    /// so unit tests touch no disk.
    pub fn load_history(&mut self) {
        let Some(path) = self.history_path.clone() else {
            self.history_warn = Some("no data dir; speed history is off".into());
            return;
        };
        let loaded = crate::history::load(&path);
        if loaded.total > crate::history::KEEP {
            if let Err(e) = crate::history::compact_if_needed(&path, crate::history::KEEP) {
                self.log(LogLevel::Warn, format!("speed history not trimmed: {e}"));
            }
        }
        self.history_total = loaded.total;
        self.history = loaded
            .rows
            .into_iter()
            .take(crate::history::SHOWN)
            .collect();
        if let Some(w) = &loaded.warn {
            self.log(LogLevel::Warn, w.clone());
        }
        self.history_warn = loaded.warn;
        self.history_writable = loaded.writable;
    }

    /// One row per finished run. A run that failed spent no bytes of its own,
    /// so it is not a measurement and is not stored.
    fn record_speed(
        &mut self,
        result: &crate::speedtest::SpeedResult,
        elapsed: Duration,
        pings: &crate::history::Pings,
    ) {
        let (Some(path), true) = (self.history_path.clone(), self.history_writable) else {
            return;
        };
        let ctx = self.speed_context.clone().unwrap_or_default();
        let radio = crate::history::Radio {
            net_type: Some(ctx.net_type.clone()).filter(|s| !s.is_empty()),
            band: Some(ctx.band.clone()).filter(|s| !s.is_empty() && s != "-"),
            band_at_start: ctx.band_at_start.clone(),
            rsrp_dbm: ctx.rsrp_dbm,
            rsrq_db: ctx.rsrq_db,
            sinr_db: ctx.sinr_db,
        };
        let entry = crate::history::Entry::from_result(
            result,
            &self.speed_cfg,
            crate::history::Source::Tui,
            Some(&radio),
            pings.clone(),
            elapsed.as_millis() as u64,
            unix_now(),
        );
        match crate::history::append(&path, &entry) {
            Ok(()) => {
                self.history_total += 1;
                self.history.insert(0, entry);
                self.history.truncate(crate::history::SHOWN);
                self.history_warn = None;
            }
            Err(e) => {
                let msg = format!("speed history not saved: {e}");
                self.log(LogLevel::Warn, msg.clone());
                self.history_warn = Some(msg);
            }
        }
    }

    fn radio_now(&self) -> Option<SpeedContext> {
        let s = self.snap.as_ref()?;
        Some(SpeedContext {
            net_type: s.connection.net_type.clone(),
            // `band` is the label ("LTE B3"); `earfcn` is the channel number and
            // would print a bare integer where a band belongs (`src/model.rs:35`).
            band_at_start: self.speed_band_at_start.clone(),
            band: s.signal.band.clone().unwrap_or_else(|| "-".into()),
            rsrp_dbm: s.signal.rsrp_dbm,
            rsrq_db: s.signal.rsrq_db,
            sinr_db: s.signal.sinr_db,
            router_peak_dl: self.speed_peak_dl,
            router_peak_ul: self.speed_peak_ul,
        })
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

/// Seconds since the epoch, for a result's age. Clock jumps are not an error.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The frozen snapshot every render and state test starts from. Public so the
/// UI tests draw the same app the state tests build.
#[cfg(test)]
pub(crate) fn fixture_snap() -> crate::model::Snapshot {
    serde_json::from_str(include_str!("../fixtures/sample-snapshot.json")).expect("fixture")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order is built once and reused: a frame that draws the same rows must
    /// not sort them again.
    #[test]
    fn derived_rows_are_cached_and_reused_between_draws() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        let rows = app.sorted_clients().len();
        for _ in 0..5 {
            assert_eq!(app.sorted_clients().len(), rows);
        }
        let cache = app.derived.borrow();
        assert_eq!(
            cache.rebuilds, 1,
            "the order is built once, not once a frame"
        );
        assert!(
            cache.hits >= 5,
            "later frames reuse it: {} hits",
            cache.hits
        );
    }

    /// A new sort key, or a row whose content moved with the count unchanged,
    /// must rebuild the order and draw the new one.
    #[test]
    fn a_cache_is_invalidated_when_its_inputs_change() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        app.sorted_clients();
        app.client_sort = SortKey::Time;
        app.sorted_clients();
        assert_eq!(app.derived.borrow().rebuilds, 2, "a new sort key rebuilds");
        if let Some(s) = app.snap.as_mut() {
            s.clients[0].name = Some("Aaa first".into());
        }
        app.client_sort = SortKey::Name;
        let first = app.sorted_clients().first().and_then(|c| c.name.clone());
        assert_eq!(first.as_deref(), Some("Aaa first"));
        assert_eq!(
            app.derived.borrow().rebuilds,
            3,
            "a changed row with an unchanged count rebuilds"
        );
    }

    #[test]
    fn a_successful_apply_remembers_the_mask_it_replaced() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        let before = app.bands_on_device();
        let intent = app.band_intent();
        assert!(app.begin_confirm());
        app.settle_write(WriteState::Pending {
            intent: intent.clone(),
            since: Instant::now(),
        });
        app.settle_write(WriteState::Success {
            intent,
            note: "verified, device mode 03 mask 0x800C5".into(),
        });
        assert_eq!(
            app.undo_mask.as_deref(),
            Some(&before[..]),
            "undo must hold the old mask"
        );
        let last = app.last_apply.as_ref().expect("last apply");
        assert_eq!(last.verdict, "verified");
        assert!(last.bands.contains(&1));
    }

    #[test]
    fn the_pending_states_do_not_erase_the_undo_point() {
        // The worker sends Pending and Verifying before the verdict, so the
        // real sequence is what this test replays, not a bare Success.
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        let intent = app.band_intent();
        assert!(app.begin_confirm());
        app.settle_write(WriteState::Pending {
            intent: intent.clone(),
            since: Instant::now(),
        });
        app.settle_write(WriteState::Verifying {
            intent: intent.clone(),
            since: Instant::now(),
        });
        assert!(app.undo_mask.is_none(), "nothing settled yet");
        app.settle_write(WriteState::Success {
            intent,
            note: "verified".into(),
        });
        assert!(
            app.undo_mask.is_some(),
            "Pending and Verifying must not clear the point they precede"
        );
    }

    #[test]
    fn a_noop_apply_offers_no_undo() {
        // "already active, nothing sent": there is no earlier mask to go back to.
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        let intent = app.band_intent();
        assert!(app.begin_confirm());
        app.settle_write(WriteState::Success {
            intent,
            note: "already active (mode + mask 0x88080095) - nothing sent".into(),
        });
        assert!(app.undo_mask.is_none());
        assert!(app.last_apply.is_none(), "nothing was applied");
    }

    #[test]
    fn a_failed_apply_leaves_the_undo_point_alone() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        app.undo_mask = Some(vec![1, 3]);
        let intent = app.band_intent();
        assert!(app.begin_confirm());
        app.settle_write(WriteState::Failed {
            intent,
            err: "router refused".into(),
        });
        assert_eq!(app.undo_mask.as_deref(), Some(&[1u8, 3][..]));
        assert!(app.last_apply.is_none(), "a failure is not an apply");
    }

    #[test]
    fn the_countdown_opens_when_an_apply_lands() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        app.bands_selected = vec![1, 3];
        assert!(app.begin_confirm());
        let intent = app.write.intent().cloned().expect("confirming intent");
        app.settle_write(WriteState::Pending {
            intent: intent.clone(),
            since: Instant::now(),
        });
        app.settle_write(WriteState::Success {
            intent,
            note: "verified".into(),
        });
        let left = app.watch_secs_left(Instant::now()).expect("a window");
        assert!(
            (WATCH_SECS - 2..=WATCH_SECS).contains(&left),
            "the window is {WATCH_SECS} seconds, got {left}"
        );
        let line = app.watch_line().expect("the status bar says so");
        assert!(line.contains("watching "), "{line}");
        assert!(line.contains("u reverts"), "{line}");
    }

    #[test]
    fn the_countdown_ending_writes_nothing() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        app.bands_selected = vec![1, 3];
        assert!(app.begin_confirm());
        let intent = app.write.intent().cloned().expect("confirming intent");
        app.settle_write(WriteState::Pending {
            intent: intent.clone(),
            since: Instant::now(),
        });
        app.settle_write(WriteState::Success {
            intent,
            note: "verified".into(),
        });
        let until = app.watch_until.expect("a window");
        let selected = app.bands_selected.clone();
        assert_eq!(
            app.watch_secs_left(until + Duration::from_secs(1)),
            None,
            "the window is over"
        );
        assert_eq!(
            app.watch_line_at(until + Duration::from_secs(1)),
            None,
            "and the line is gone with it"
        );
        assert_eq!(
            app.bands_selected, selected,
            "an expired window moves no tick"
        );
        assert!(
            matches!(app.write, WriteState::Success { .. }),
            "it writes nothing, so the verdict stands"
        );
        assert!(app.undo_mask.is_some(), "the point is still there to use");
    }

    #[test]
    fn undo_restores_the_mask_without_writing() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        app.undo_mask = Some(vec![1, 3, 5]);
        app.preset_cursor = Some(2);
        let label = app.band_undo().expect("undo applies");
        assert_eq!(app.bands_selected, vec![1, 3, 5]);
        assert!(matches!(app.write, WriteState::Idle), "undo never writes");
        assert!(
            app.preset_cursor.is_none(),
            "the preset highlight is stale now"
        );
        assert!(label.contains("0x15"), "{label}");
    }

    #[test]
    fn undo_is_refused_while_a_write_is_in_flight() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        app.undo_mask = Some(vec![1, 3]);
        app.bands_selected = vec![1];
        app.write = WriteState::Verifying {
            intent: app.band_intent(),
            since: Instant::now(),
        };
        assert!(
            app.band_undo().is_none(),
            "ticking while a write is in flight would lie about what is applied"
        );
        assert_eq!(app.bands_selected, vec![1]);
    }

    #[test]
    fn undo_drops_bands_the_device_no_longer_lists() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        app.bands_supported = vec![1, 3];
        app.undo_mask = Some(vec![1, 3, 66]);
        app.band_undo().expect("undo applies");
        assert_eq!(
            app.bands_selected,
            vec![1, 3],
            "66 is not in this device's list"
        );
    }

    #[test]
    fn bands_draw_order_groups_by_class_and_keeps_every_band() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.bands_supported = vec![1, 3, 5, 7, 8, 20, 28, 32, 34, 38, 39, 40, 41, 42, 43];
        let order = app.bands_in_draw_order();
        assert_eq!(
            order.len(),
            app.bands_supported.len(),
            "no band may be dropped"
        );
        assert_eq!(
            &order[..4],
            &[5, 8, 20, 28],
            "low first, ascending inside the class"
        );
        let last_low = order.iter().position(|b| *b == 28).unwrap();
        let first_mid = order.iter().position(|b| *b == 1).unwrap();
        let first_high = order.iter().position(|b| *b == 7).unwrap();
        assert!(last_low < first_mid && first_mid < first_high, "{order:?}");
    }

    #[test]
    fn an_unknown_band_is_ordered_last_not_hidden() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.bands_supported = vec![99, 1];
        assert_eq!(app.bands_in_draw_order(), vec![1, 99]);
    }

    #[test]
    fn the_cursor_ticks_the_band_it_is_drawn_on() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.bands_supported = vec![1, 5];
        app.band_cursor = 0;
        app.band_toggle();
        assert_eq!(
            app.bands_selected,
            vec![5],
            "cursor 0 is the first drawn band, B5"
        );
    }

    #[test]
    fn bands_diff_names_the_change_in_both_directions() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_snapshot(fixture_snap(), 0);
        assert!(app.bands_unchanged(), "the tick starts equal to the device");
        app.bands_selected.retain(|b| *b != 5);
        let (added, dropped) = app.bands_diff();
        assert!(added.is_empty());
        assert_eq!(dropped, vec![5]);
        app.bands_selected.push(34);
        app.bands_selected.sort_unstable();
        let (added, dropped) = app.bands_diff();
        assert_eq!(added, vec![34]);
        assert_eq!(dropped, vec![5]);
    }

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
            GraphStyle::Braille,
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
            GraphStyle::Braille,
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
            GraphStyle::Braille,
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
    fn a_redraw_happens_only_when_something_changed() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.mark_drawn();
        assert!(
            !app.needs_draw,
            "marking a frame drawn left the request open, so the loop repaints at the poll rate"
        );
        app.on_snapshot(fixture_snap(), 0);
        assert!(app.needs_draw, "a new snapshot must ask for a frame");
    }

    #[test]
    fn history_is_bounded() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
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
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            0,
        );
        assert!(!app.stale());
        app.last_ok = Some(Instant::now() - Duration::from_millis(5));
        assert!(!app.stale(), "five ms is not three periods at any period");
        app.last_ok = Some(Instant::now() - Duration::from_secs(7));
        assert!(
            app.stale(),
            "seven seconds is past three periods and the floor"
        );
    }

    /// A 600 ms period would put the stale line at 1.8 s, under the firmware's
    /// own tail, so the header would blink "stale" on a healthy link.
    #[test]
    fn a_fast_period_does_not_call_the_firmware_tail_stale() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            600,
        );
        app.last_ok = Some(Instant::now() - Duration::from_millis(1_900));
        assert!(!app.stale(), "1.9 s is not stale at a 600 ms period");
        app.last_ok = Some(Instant::now() - Duration::from_secs(7));
        assert!(app.stale(), "seven seconds is past three periods");
    }

    /// The history rings are the signal series: a clients tick carries the same
    /// radio reading again and must not push a second point.
    #[test]
    fn a_clients_tick_adds_no_signal_sample() {
        use crate::worker::Tier;
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            600,
        );
        let radio = Instant::now() - Duration::from_millis(900);
        app.stamps.set(Tier::Radio, radio);
        app.on_snapshot(fixture_snap(), 0);
        let points = app.history_rsrp.len();
        app.stamps.set(Tier::Clients, Instant::now());
        app.on_snapshot(fixture_snap(), 0);
        assert_eq!(
            app.history_rsrp.len(),
            points,
            "a clients tick pushed a second signal sample"
        );
        app.stamps.set(Tier::Radio, Instant::now());
        app.on_snapshot(fixture_snap(), 0);
        assert_eq!(
            app.history_rsrp.len(),
            points + 1,
            "a radio read must sample"
        );
    }

    #[test]
    fn clients_sort_numerically_and_by_time() {
        use crate::model::Client;
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
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
            quota: None,
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
            GraphStyle::Braille,
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
            quota: None,
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
            GraphStyle::Braille,
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
            GraphStyle::Braille,
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
            GraphStyle::Braille,
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
    fn demo_history_fills_the_charts_and_stops_at_the_cap() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.demo_history(120);
        assert_eq!(app.history_rsrp.len(), 120);
        assert_eq!(app.history_sinr.len(), 120);
        assert_eq!(app.history_dl.len(), 120);
        assert!(
            app.history_dl.iter().all(|v| *v >= 0.0),
            "a synthetic rate is never negative"
        );
        app.demo_history(HISTORY * 2);
        assert_eq!(app.history_rsrp.len(), HISTORY, "capped like on_snapshot");
        assert_eq!(app.history_dl.len(), HISTORY);
        app.demo_history(0);
        assert_eq!(app.history_dl.len(), HISTORY, "0 leaves the series alone");
    }

    #[test]
    fn cycling_views_remembers_the_return_point() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
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

        // Any one NR metric promotes the panel, not RSRP alone.
        snap.signal.nr = Some(crate::model::Nr {
            rsrp_dbm: Some(-88.0),
            ..Default::default()
        });
        assert_eq!(resolve_signal_panel(Some(&snap)), SignalPanel::NrGauges);

        snap.signal.nr = Some(crate::model::Nr {
            sinr_db: Some(12.0),
            ..Default::default()
        });
        assert_eq!(resolve_signal_panel(Some(&snap)), SignalPanel::NrGauges);

        // Secondary fields alone are not a level: bars stay, with their names.
        snap.signal.nr = Some(crate::model::Nr {
            earfcn: Some("DL:528980 UL:528980".into()),
            dl_bw: Some("100MHz".into()),
            ..Default::default()
        });
        assert_eq!(resolve_signal_panel(Some(&snap)), SignalPanel::NrBars);

        assert_eq!(resolve_signal_panel(None), SignalPanel::LteGauges);
    }

    #[test]
    fn signal_history_caps_at_120() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
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
    fn the_feed_keeps_the_selected_line_when_new_events_arrive() {
        let mut app = App::new(
            crate::theme::by_name("dark"),
            crate::theme::GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.events_open = true;
        app.events_paused = true;
        for i in 0..5 {
            app.log(LogLevel::Info, format!("event {i}"));
        }
        app.event_select(2);
        let before = app.event_cursor().expect("cursor");
        app.log(LogLevel::Warn, "newer event".to_string());
        assert_eq!(
            app.event_cursor(),
            Some(before),
            "the selection moved under the reader"
        );
    }

    #[test]
    fn a_running_feed_stays_on_the_newest_line() {
        let mut app = App::new(
            crate::theme::by_name("dark"),
            crate::theme::GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.events_open = true;
        for i in 0..4 {
            app.log(LogLevel::Info, format!("event {i}"));
        }
        assert_eq!(app.event_cursor(), Some(0), "an unpaused feed must follow");
        app.event_move(2);
        assert!(app.events_paused, "moving pauses");
        app.log(LogLevel::Info, "newer".to_string());
        assert_eq!(app.event_cursor(), Some(2), "a paused feed holds its place");
    }

    #[test]
    fn the_feed_cursor_never_leaves_the_log() {
        let mut app = App::new(
            crate::theme::by_name("dark"),
            crate::theme::GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        assert_eq!(app.event_cursor(), None, "an empty log has no cursor");
        for i in 0..3 {
            app.log(LogLevel::Info, format!("event {i}"));
        }
        app.event_move(-5);
        assert_eq!(app.event_cursor(), Some(0));
        app.event_move(99);
        assert_eq!(
            app.event_cursor(),
            Some(2),
            "the cursor stops at the oldest"
        );
    }

    #[test]
    fn event_log_is_bounded_and_ordered() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
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
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.mark_extras_requested();
        assert!(!app.needs_extras());
        let extras = crate::model::DeviceExtras {
            sms_error: Some("request rejected by the firmware (100005)".into()),
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
            GraphStyle::Braille,
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
    fn a_new_scan_resets_and_reports_its_progress() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        app.on_phase(Phase::Signal, PhaseState::Running);
        app.on_phase(Phase::Signal, PhaseState::Done);
        app.on_phase(Phase::Status, PhaseState::Done);
        app.on_phase(Phase::Traffic, PhaseState::Running);
        assert_eq!(app.phase_progress(), (2, 10));
        assert_eq!(app.current_phase(), Some(Phase::Traffic));
        assert!(app.is_scanning());

        // The next poll starts at the first phase again: yesterday's ticks go.
        app.on_phase(Phase::Signal, PhaseState::Running);
        assert_eq!(app.phase_progress(), (0, 10));

        for phase in Phase::ALL {
            app.on_phase(phase, PhaseState::Done);
        }
        assert!(!app.is_scanning());
        assert_eq!(app.current_phase(), None);
        assert_eq!(app.phase_progress(), (10, 10));
    }

    #[test]
    fn session_short_labels_fit_a_header() {
        let cases = [
            (SessionState::Connecting, "connecting"),
            (SessionState::Live, "live"),
            (SessionState::Restarting, "restarting"),
            (
                SessionState::Recovering {
                    attempt: 3,
                    wait_s: 8,
                },
                "recovering #3 8s",
            ),
            (
                SessionState::Disconnected("auth denied".into()),
                "disconnected",
            ),
        ];
        for (state, want) in cases {
            let short = state.short();
            assert_eq!(short, want, "short label for {state:?}");
            assert!(
                short.chars().count() <= 18,
                "{short} is too wide for a header"
            );
        }
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
    fn a_report_is_redacted_until_the_user_opts_in() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        assert!(
            !app.report_redaction().identity,
            "a fresh app must redact reports"
        );
        app.report_identity = true;
        assert!(app.report_redaction().identity);
    }

    #[test]
    fn a_confirmed_band_write_carries_its_preflight() {
        let mut app = App::new(
            Theme::get("terminal"),
            GraphStyle::Braille,
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

    /// Every poll feeds the ledger: one snapshot is the baseline, the next one
    /// is the delta. Two hundred seconds apart, so the gap guard (300 s) does
    /// not fire and the sample lands in today's bucket.
    #[test]
    fn a_snapshot_feeds_the_usage_ledger() {
        let mut app = App::new(
            crate::theme::Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        let mut snap = crate::model::Snapshot {
            router: "10.0.0.1".into(),
            generated_at: 1_000,
            ..Default::default()
        };
        snap.traffic.total_dl_bytes = 1_000;
        app.on_snapshot(snap.clone(), 2_000);
        snap.generated_at = 1_200;
        snap.traffic.total_dl_bytes = 1_200;
        app.on_snapshot(snap, 2_000);
        assert_eq!(app.usage.days.len(), 1);
        assert_eq!(app.usage.days[0].dl, 200);
        assert!(app.usage_dirty, "a new sample must be marked for flushing");
    }

    /// A new address is a new counter, not a new month: the table survives and
    /// only the baseline restarts.
    #[test]
    fn a_router_switch_does_not_wipe_the_days() {
        let mut app = App::new(
            crate::theme::Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        );
        let mut snap = crate::model::Snapshot {
            router: "10.0.0.1".into(),
            generated_at: 1_000,
            ..Default::default()
        };
        snap.traffic.total_dl_bytes = 1_000;
        app.on_snapshot(snap.clone(), 2_000);
        snap.generated_at = 1_200;
        snap.traffic.total_dl_bytes = 1_500;
        app.on_snapshot(snap.clone(), 2_000);
        snap.router = "10.0.0.2".into();
        snap.generated_at = 1_400;
        snap.traffic.total_dl_bytes = 9_000;
        app.on_snapshot(snap, 2_000);
        assert_eq!(app.usage.days.len(), 1, "the history survives a new router");
        assert_eq!(app.usage.router, "10.0.0.2");
    }

    fn fixture_app() -> App {
        // `Theme::get` is the public door (src/theme.rs:208); there is no by_id.
        App::new(
            crate::theme::Theme::get("terminal"),
            GraphStyle::Braille,
            View::Full,
            100,
            2000,
        )
    }

    /// `on_speed_done` takes the whole run, so a test builds one from the
    /// numbers it cares about.
    fn done(r: crate::speedtest::SpeedResult) -> SpeedRun {
        SpeedRun {
            result: r,
            elapsed: Duration::from_secs(1),
            pings: crate::history::Pings::default(),
        }
    }

    fn scratch_history(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rw-app-hist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("speed-history.jsonl")
    }

    #[test]
    fn a_finished_run_is_saved_once_and_capped_at_ten() {
        let mut a = fixture_app();
        a.history_writable = true;
        a.history_path = Some(scratch_history("once"));
        a.speed_context = Some(crate::app::SpeedContext::default());
        for _ in 0..12 {
            let r = crate::speedtest::SpeedResult {
                download_bps: 1.0,
                at_unix: unix_now(),
                ..crate::speedtest::SpeedResult::default()
            };
            a.speed_run += 1;
            a.on_speed_done(a.speed_run, Ok(done(r)));
        }
        assert_eq!(a.history.len(), crate::history::SHOWN);
        assert_eq!(a.history_total, 12);
        let p = a.history_path.clone().unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 12);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_failed_run_is_not_saved() {
        let mut a = fixture_app();
        a.history_writable = true;
        a.history_path = Some(scratch_history("failed"));
        a.speed_run += 1;
        a.on_speed_done(a.speed_run, Err("offline".into()));
        assert!(a.history.is_empty());
        assert!(!a.history_path.clone().unwrap().exists());
        let _ = std::fs::remove_dir_all(a.history_path.unwrap().parent().unwrap());
    }

    #[test]
    fn a_write_failure_is_logged_and_shown() {
        let mut a = fixture_app();
        a.history_writable = true;
        let dir = std::env::temp_dir().join(format!("rw-app-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        // Parent is a regular file: create_dir_all fails for every user, root too.
        a.history_path = Some(blocker.join("speed-history.jsonl"));
        let r = crate::speedtest::SpeedResult {
            at_unix: unix_now(),
            ..crate::speedtest::SpeedResult::default()
        };
        a.speed_run += 1;
        a.on_speed_done(a.speed_run, Ok(done(r)));
        assert!(a
            .history_warn
            .clone()
            .unwrap_or_default()
            .contains("not saved"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_finished_test_keeps_the_radio_it_ran_on() {
        let mut app = fixture_app();
        app.on_snapshot(fixture_snap(), 2000);
        let (run, _, _) = app.start_speed_test();
        assert!(app.speed_running);
        app.on_speed_done(
            run,
            Ok(done(crate::speedtest::SpeedResult {
                download_bps: 94_000_000.0,
                upload_bps: 12_000_000.0,
                ttfb_ms: 18.0,
                down_bytes: 50_000_000,
                up_bytes: 10_000_000,
                down_secs: 4.2,
                up_secs: 6.6,
                download_url: "http://x/".into(),
                upload_url: "http://y/".into(),
                partial: false,
                short_window: false,
                note: None,
                at_unix: 1_700_000_000,
            })),
        );
        assert!(!app.speed_running);
        assert_eq!(app.speed.as_ref().unwrap().download_bps, 94_000_000.0);
        let ctx = app.speed_context.as_ref().expect("radio snapshot");
        assert!(!ctx.net_type.is_empty());
        let band = app
            .snap
            .as_ref()
            .unwrap()
            .signal
            .band
            .clone()
            .unwrap_or_else(|| "-".into());
        assert_eq!(ctx.band, band);
    }

    #[test]
    fn a_second_t_is_refused_while_a_test_runs() {
        let mut app = fixture_app();
        app.start_speed_test();
        assert!(!app.begin_speed_test());
        assert!(!app.speed_confirm, "no second confirmation appears");
    }

    #[test]
    fn the_app_clamps_the_limits_it_holds() {
        // The clamp is the config crate's, and the app stores its result: 5_000
        // becomes 500, not the default 60. One rule, one place.
        let mut app = fixture_app();
        app.set_speed_cfg(crate::config::Speedtest {
            max_mb: 5_000,
            upload_mb: 0,
            ..crate::config::Speedtest::default()
        });
        assert_eq!(app.speed_cfg.max_mb, 500);
        assert_eq!(app.speed_cfg.upload_mb, 1);
    }

    #[test]
    fn a_result_from_an_older_run_is_ignored() {
        let mut app = fixture_app();
        let (old, _, _) = app.start_speed_test();
        app.on_speed_done(old, Err("cancelled".into()));
        let (current, _, _) = app.start_speed_test();
        // A late answer from the abandoned run must not clear the live one.
        app.on_speed_done(old, Err("late".into()));
        assert!(app.speed_running, "run {current} is still measuring");
        assert_eq!(app.speed_run, current);
    }

    #[test]
    fn every_run_gets_its_own_cancel_flag() {
        let mut app = fixture_app();
        let (first, _, flag) = app.start_speed_test();
        app.cancel_speed_test();
        assert!(flag.load(Ordering::Relaxed));
        let (second, _, fresh) = app.start_speed_test();
        assert_ne!(first, second);
        assert!(
            !fresh.load(Ordering::Relaxed),
            "the new run is not cancelled by the old flag"
        );
        assert!(flag.load(Ordering::Relaxed), "the old flag stays set");
    }

    #[test]
    fn a_test_that_outlives_its_budget_is_cleared() {
        let mut app = fixture_app();
        app.start_speed_test();
        let past = crate::speedtest::worst_case_secs(&app.speed_cfg) + 1;
        app.speed_started = Instant::now().checked_sub(Duration::from_secs(past));
        app.speed_watchdog();
        assert!(!app.speed_running);
        assert!(app.speed_error.is_some());
    }

    #[test]
    fn a_rate_the_device_never_saw_is_flagged() {
        let mut app = fixture_app();
        app.on_snapshot(fixture_snap(), 2000);
        let (run, _, _) = app.start_speed_test();
        // The device reports bytes per second; 100 kB/s is 0.8 Mbps. A measured
        // 94 Mbps cannot have crossed a link the counter never saw.
        app.speed_peak_dl = 100_000;
        app.on_speed_done(
            run,
            Ok(done(crate::speedtest::SpeedResult {
                download_bps: 94_000_000.0,
                down_secs: 4.2,
                ..crate::speedtest::SpeedResult::default()
            })),
        );
        let note = app.speed.as_ref().unwrap().note.clone().unwrap_or_default();
        assert!(note.contains("did not see"), "{note}");
    }
}
