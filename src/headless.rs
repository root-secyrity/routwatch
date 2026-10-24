//! Headless output: `snapshot`, `doctor`. Everything the TUI will draw comes
//! from here, so M2 cannot invent a second data path.

use crate::config::Config;
use crate::error::ApiError;
use crate::model::Snapshot;
use crate::router::{bands, endpoints, parse, HuaweiClient};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// One endpoint read inside `collect_traced`. The order is the wire order: a
/// phase only starts after the previous one answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Signal,
    Status,
    Traffic,
    Plmn,
    Device,
    NetMode,
    Clients,
    Sms,
    Dns,
    Bands,
}

impl Phase {
    pub const ALL: [Phase; 10] = [
        Phase::Signal,
        Phase::Status,
        Phase::Traffic,
        Phase::Plmn,
        Phase::Device,
        Phase::NetMode,
        Phase::Clients,
        Phase::Sms,
        Phase::Dns,
        Phase::Bands,
    ];

    pub fn index(self) -> usize {
        Phase::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    pub fn label(self) -> &'static str {
        match self {
            Phase::Signal => "signal",
            Phase::Status => "status",
            Phase::Traffic => "traffic",
            Phase::Plmn => "plmn",
            Phase::Device => "device",
            Phase::NetMode => "net-mode",
            Phase::Clients => "clients",
            Phase::Sms => "sms",
            Phase::Dns => "dns",
            Phase::Bands => "bands",
        }
    }
}

/// What the header needs to know about each phase. `Waiting` exists so a new
/// scan can be shown as "not read yet" instead of the previous scan's `Done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseState {
    Waiting,
    Running,
    Done,
}

/// Announce a phase around the work that belongs to it. `Done` is reported even
/// when the read failed — the phase is finished either way.
pub fn step<T>(
    trace: &mut impl FnMut(Phase, PhaseState),
    phase: Phase,
    work: impl FnOnce() -> T,
) -> T {
    trace(phase, PhaseState::Running);
    let value = work();
    trace(phase, PhaseState::Done);
    value
}

/// `collect` with no observer: the CLI and every existing call site.
pub fn collect(c: &mut HuaweiClient, ip: &str) -> Result<Snapshot, ApiError> {
    collect_traced(c, ip, &mut |_, _| {})
}

pub fn collect_traced(
    c: &mut HuaweiClient,
    ip: &str,
    trace: &mut impl FnMut(Phase, PhaseState),
) -> Result<Snapshot, ApiError> {
    let signal = parse::parse_signal(&step(trace, Phase::Signal, || c.get("device/signal"))?);
    let status =
        parse::parse_connection(&step(trace, Phase::Status, || c.get("monitoring/status"))?);
    let traffic = parse::parse_traffic(&step(trace, Phase::Traffic, || {
        c.get("monitoring/traffic-statistics")
    })?);
    let plmn = parse::parse_plmn(&step(trace, Phase::Plmn, || c.get("net/current-plmn"))?);
    let device = parse::parse_device(&step(trace, Phase::Device, || c.get("device/information"))?);
    let net_mode = parse::parse_net_mode(&step(trace, Phase::NetMode, || c.get("net/net-mode"))?);
    // Optional sensor: a firmware without it answers an error, which is not a
    // failed scan — the TEMP tile simply does not exist.
    let temperature = step(trace, Phase::Device, || c.get("device/temperature").ok())
        .and_then(|node| parse::parse_temperature(&node));
    // Optional extras: their absence must not fail the snapshot.
    let clients = step(trace, Phase::Clients, || c.get("wlan/host-list").ok())
        .map(|n| parse::parse_clients(&n))
        .unwrap_or_default();
    let sms =
        step(trace, Phase::Sms, || c.get("sms/sms-count").ok()).map(|n| parse::parse_sms_count(&n));
    let dns = step(trace, Phase::Dns, || c.get("dhcp/settings").ok()).map(|n| parse::parse_dns(&n));

    let (supported_bands, all_bands_mask) =
        match step(trace, Phase::Bands, || c.get("net/net-mode-list")) {
            Ok(node) => {
                let (defs, all) = bands::from_net_mode_list(&node);
                (
                    defs.iter().filter_map(|d| d.band.map(u32::from)).collect(),
                    all.map(bands::to_hex),
                )
            }
            Err(_) => (Vec::new(), None),
        };

    Ok(Snapshot {
        generated_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        router: ip.to_string(),
        signal,
        traffic,
        connection: status,
        plmn,
        device,
        net_mode,
        clients,
        sms,
        dns,
        temperature,
        supported_bands,
        all_bands_mask,
    })
}

/// Lazily-fetched extras (opened by the SMS/Device tabs, the `R` key, or `sms`).
/// Kept out of the 2s poll: each of these costs a round trip the dashboard
/// does not need.
pub fn fetch_extras(c: &mut HuaweiClient) -> crate::model::DeviceExtras {
    let pin = c.pin_status().ok().map(|n| parse::parse_pin(&n));
    let mobile_data = c
        .mobile_data_switch()
        .ok()
        .map(|n| parse::parse_mobile_data(&n));
    let (sms_inbox, sms_error) = match c.sms_list(1) {
        Ok(n) => (parse::parse_sms_list(&n), None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    crate::model::DeviceExtras {
        pin,
        mobile_data,
        sms_inbox,
        sms_error,
    }
}

/// Write every probed endpoint's raw XML plus a parsed snapshot to one JSON file.
pub fn dump_to(c: &mut HuaweiClient, ip: &str, path: &std::path::Path) -> Result<usize, ApiError> {
    let snap = collect(c, ip)?;
    let mut raw = serde_json::Map::new();
    for spec in crate::router::endpoints::PROBE {
        let value = match c.raw(spec.path) {
            Ok(body) => serde_json::Value::String(body),
            Err(e) => serde_json::Value::String(format!("<error: {e}>")),
        };
        raw.insert(spec.path.to_string(), value);
    }
    let doc = serde_json::json!({
        "schema_version": 1,
        "generated_at": snap.generated_at,
        "router": ip,
        "snapshot": snap,
        "raw": raw,
    });
    let text = serde_json::to_string_pretty(&doc)
        .map_err(|e| ApiError::Config(format!("serialize dump: {e}")))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| ApiError::Config(format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(path, &text)
        .map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
    Ok(text.len())
}

#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    pub id: String,
    pub state: String,
    pub reason: String,
    pub detail: String,
    pub next_check: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub scope: &'static str,
    pub platform: &'static str,
    pub router: Option<String>,
    pub capabilities: Vec<Capability>,
}

fn cap(
    id: &str,
    state: &str,
    reason: &str,
    detail: impl Into<String>,
    next: Option<&str>,
) -> Capability {
    Capability {
        id: id.to_string(),
        state: state.to_string(),
        reason: reason.to_string(),
        detail: detail.into(),
        next_check: next.map(str::to_string),
    }
}

/// Is this helper on PATH? Distros disagree on package names, so routwatch
/// probes for the binary instead of assuming a distribution.
fn tool_present(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(name);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

/// Optional helpers differ per distro and desktop: `iputils-ping` vs `iputils`,
/// `wl-clipboard` vs `xclip`/`xsel`, `libsecret-tools` for the keyring. Report
/// what this machine has instead of failing later with a silent no-op.
fn helpers_capability() -> Capability {
    let ping = tool_present("ping");
    let clipboard = ["wl-copy", "xclip", "xsel"]
        .into_iter()
        .find(|tool| tool_present(tool));
    let keyring = tool_present("secret-tool");
    let detail = format!(
        "ping {} · clipboard {} · keyring {}",
        if ping { "found" } else { "missing" },
        clipboard.unwrap_or("none"),
        if keyring {
            "secret-tool"
        } else {
            "env/config only"
        }
    );
    let ready = ping && clipboard.is_some();
    cap(
        "helpers",
        if ready { "ready" } else { "degraded" },
        if ready {
            "helpers_present"
        } else {
            "helpers_missing"
        },
        detail,
        if ready {
            None
        } else {
            Some(
                "install what is missing: iputils-ping (or iputils), wl-clipboard or xclip, libsecret-tools",
            )
        },
    )
}

fn credentials_source(cfg: &Config) -> &'static str {
    if std::env::var("ROUTWATCH_PASSWORD")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        "env:ROUTWATCH_PASSWORD"
    } else if crate::config::secret_lookup().is_some() {
        "secret-tool"
    } else if !cfg.router.password.is_empty() {
        "config.toml"
    } else {
        "none"
    }
}

/// Static checks never contact the router; `--live` adds reachability, auth and
/// the endpoint capability map (same shape as netwatch's doctor --json).
pub fn doctor(cfg: &Config, config_path: &str, live: bool) -> DoctorReport {
    let mut caps = Vec::new();

    caps.push(cap(
        "config",
        "ready",
        "config_valid",
        format!("parsed {config_path}; mode 0600 enforced"),
        Some("routwatch --generate-config writes a fresh file"),
    ));

    // A theme name the build does not ship silently renders as `dark`; say so
    // here instead of leaving the user to wonder why the colours changed.
    let wanted = cfg.app.theme.trim();
    let resolved = crate::theme::by_name(wanted);
    if crate::theme::is_known(wanted) {
        caps.push(cap(
            "theme",
            "ready",
            "theme_valid",
            format!("theme {wanted:?} → {}", resolved.id),
            None,
        ));
    } else {
        caps.push(cap(
            "theme",
            "degraded",
            "theme_unknown",
            format!(
                "theme {wanted:?} is not built in; rendering as {:?}",
                resolved.id
            ),
            Some("routwatch --theme <dark|terminal|ocean|solarized|dracula|nord|sky|paper>"),
        ));
    }

    caps.push(helpers_capability());

    let creds = credentials_source(cfg);
    if creds == "none" {
        caps.push(cap(
            "credentials",
            "failed",
            "no_credentials",
            "checked --password-stdin, ROUTWATCH_PASSWORD, secret-tool, config.toml",
            Some("export ROUTWATCH_PASSWORD or run `secret-tool store --label=routwatch service routwatch`"),
        ));
    } else {
        caps.push(cap(
            "credentials",
            "ready",
            "credentials_present",
            format!("source: {creds}"),
            None,
        ));
    }

    if !live {
        caps.push(cap(
            "reachability",
            "not_checked",
            "static_scope",
            "router not contacted in static mode",
            Some("routwatch doctor --live"),
        ));
        return DoctorReport {
            schema_version: 1,
            scope: "static",
            platform: std::env::consts::OS,
            router: Some(cfg.router.ip.clone()),
            capabilities: caps,
        };
    }

    let mut client = HuaweiClient::new(&cfg.router.ip, cfg.app.timeout_ms);

    match client.raw("/") {
        Ok(_) => caps.push(cap(
            "reachability",
            "ready",
            "http_answered",
            format!("http://{} answered", cfg.router.ip),
            None,
        )),
        Err(e) => {
            caps.push(cap(
                "reachability",
                "failed",
                e.class(),
                e.to_string(),
                Some("check the LAN path to the router; ping the configured IP"),
            ));
            return DoctorReport {
                schema_version: 1,
                scope: "live",
                platform: std::env::consts::OS,
                router: Some(cfg.router.ip.clone()),
                capabilities: caps,
            };
        }
    }

    let pass = crate::config::resolve_password(&cfg.router.password);
    match pass.and_then(|p| client.login(&cfg.router.username, &p)) {
        Ok(()) => caps.push(cap(
            "auth",
            "ready",
            "login_ok",
            format!("session established; client authed={}", client.is_authed()),
            None,
        )),
        Err(e) => {
            caps.push(cap(
                "auth",
                "failed",
                e.class(),
                e.to_string(),
                Some("credentials are never retried automatically; fix them and rerun"),
            ));
            return DoctorReport {
                schema_version: 1,
                scope: "live",
                platform: std::env::consts::OS,
                router: Some(cfg.router.ip.clone()),
                capabilities: caps,
            };
        }
    }

    let results = endpoints::probe_all(&mut client);
    let ready = results.iter().filter(|r| r.cap.state() == "ready").count();
    caps.push(cap(
        "endpoints",
        if ready == results.len() {
            "ready"
        } else {
            "degraded"
        },
        "capability_probe",
        format!("{ready}/{} endpoints answered", results.len()),
        Some("unsupported endpoints are marked per entry below"),
    ));
    for r in &results {
        caps.push(cap(
            r.path,
            r.cap.state(),
            &r.cap.reason(),
            r.cap
                .detail()
                .unwrap_or_else(|| format!("group: {}", r.group)),
            None,
        ));
    }

    match client.get("net/net-mode-list") {
        Ok(node) => {
            let (defs, all) = bands::from_net_mode_list(&node);
            caps.push(cap(
                "bands",
                "ready",
                "device_band_list",
                format!(
                    "{} LTE bands, all-bands mask {}",
                    defs.len(),
                    all.map(bands::to_hex).unwrap_or_else(|| "unknown".into())
                ),
                Some("routwatch bands --list shows the map the UI will use"),
            ));
        }
        Err(e) => caps.push(cap(
            "bands",
            "degraded",
            e.class(),
            format!("cannot read the device band list: {e}"),
            Some("fall back to a static band table"),
        )),
    }

    client.logout();

    DoctorReport {
        schema_version: 1,
        scope: "live",
        platform: std::env::consts::OS,
        router: Some(cfg.router.ip.clone()),
        capabilities: caps,
    }
}

// ---------------------------------------------------------------- status output

/// Waybar's custom-module contract: exactly these four keys.
#[derive(Debug, Clone, Serialize)]
pub struct WaybarStatus {
    pub text: String,
    pub tooltip: String,
    pub class: String,
    pub percentage: u8,
}

/// Bars glyph for a reading, using the same 3GPP edges as the TUI.
fn bars_glyph(rsrp: f32) -> &'static str {
    if rsrp >= -80.0 {
        "▇"
    } else if rsrp >= -90.0 {
        "▆"
    } else if rsrp >= -100.0 {
        "▄"
    } else if rsrp >= -110.0 {
        "▂"
    } else {
        "▁"
    }
}

/// RSRP mapped onto 0..100 over the physical domain (-140..-44 dBm).
pub fn rsrp_percent(rsrp: f32) -> u8 {
    let span = -44.0 - -140.0;
    (((rsrp - -140.0) / span) * 100.0).clamp(0.0, 100.0).round() as u8
}

/// Quality class shared by the bar and the TUI.
pub fn quality_class(rsrp: f32) -> &'static str {
    if rsrp >= -80.0 {
        "good"
    } else if rsrp >= -100.0 {
        "fair"
    } else {
        "poor"
    }
}

/// Build the Waybar payload. `None` (or a network failure) yields class
/// "offline" with a readable tooltip rather than an empty string.
pub fn status_waybar(snap: Option<&Snapshot>, err: Option<&str>) -> WaybarStatus {
    let Some(s) = snap else {
        return WaybarStatus {
            text: "no link".into(),
            tooltip: format!(
                "routwatch · offline\n{}",
                err.unwrap_or("router unreachable")
            ),
            class: "offline".into(),
            percentage: 0,
        };
    };
    let nr = s.signal.nr.as_ref();
    // Prefer the 5G reading when the link is actually carrying NR data.
    let rsrp = match (nr.and_then(|n| n.rsrp_dbm), s.connection.endc) {
        (Some(v), true) => Some(v),
        _ => s.signal.rsrp_dbm,
    };
    let net = if s.connection.endc && rsrp.is_some() && nr.is_some() {
        "5G"
    } else if s.connection.net_type.contains("NR") {
        "NR"
    } else {
        "LTE"
    };
    let Some(rsrp) = rsrp else {
        return WaybarStatus {
            text: format!("{net} —"),
            tooltip: "routwatch · this firmware reports no RSRP".into(),
            class: "offline".into(),
            percentage: 0,
        };
    };

    let text = format!("{net} {} {rsrp:.0}dBm", bars_glyph(rsrp));
    let band = s.signal.band.clone().unwrap_or_else(|| "—".into());
    let mut tooltip = vec![
        format!("{} ({})", s.plmn.name, s.plmn.numeric),
        format!(
            "{} · ENDC {} · {} bars",
            s.connection.net_type,
            if s.connection.endc { "on" } else { "off" },
            if s.connection.endc {
                s.connection.nr_bars
            } else {
                s.connection.signal_bars
            }
        ),
    ];
    if let Some(n) = nr {
        if n.rsrp_dbm.is_some() {
            tooltip.push(format!(
                "NR  RSRP {:.0} dBm · RSRQ {} dB · SINR {} dB",
                n.rsrp_dbm.unwrap_or(0.0),
                n.rsrq_db
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "—".into()),
                n.sinr_db
                    .map(|v| format!("{v:.0}"))
                    .unwrap_or_else(|| "—".into()),
            ));
        }
    }
    tooltip.push(format!(
        "LTE RSRP {:.0} dBm · RSRQ {} dB · SINR {} dB",
        s.signal.rsrp_dbm.unwrap_or(0.0),
        s.signal
            .rsrq_db
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "—".into()),
        s.signal
            .sinr_db
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "—".into()),
    ));
    tooltip.push(format!(
        "{band} · {} MHz · PCI {}",
        s.signal
            .dl_mhz
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".into()),
        s.signal
            .pci
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".into())
    ));
    tooltip.push(format!(
        "↓ {} · ↑ {}",
        crate::ui::widgets::bps(s.traffic.dl_rate_bytes as f64),
        crate::ui::widgets::bps(s.traffic.ul_rate_bytes as f64)
    ));
    tooltip.push(format!(
        "session ↓ {} · ↑ {}",
        crate::ui::widgets::bytes(s.traffic.session_dl_bytes),
        crate::ui::widgets::bytes(s.traffic.session_ul_bytes)
    ));
    tooltip.push(format!(
        "{} clients · mode {}",
        s.clients.len(),
        s.net_mode
            .network_mode
            .clone()
            .unwrap_or_else(|| "—".into())
    ));

    WaybarStatus {
        text,
        tooltip: tooltip.join("\n"),
        class: quality_class(rsrp).into(),
        percentage: rsrp_percent(rsrp),
    }
}

/// Compact one-line JSON for scripts: stable keys, no ANSI, no secrets.
pub fn status_json(s: &Snapshot) -> serde_json::Value {
    let nr = s.signal.nr.as_ref();
    serde_json::json!({
        "router": s.router,
        "operator": s.plmn.name,
        "operator_numeric": s.plmn.numeric,
        "net_type": s.connection.net_type,
        "endc": s.connection.endc,
        "nr_bars": s.connection.nr_bars,
        "rsrp_dbm": s.signal.rsrp_dbm,
        "rsrq_db": s.signal.rsrq_db,
        "sinr_db": s.signal.sinr_db,
        "rssi_dbm": s.signal.rssi_dbm,
        "nr_rsrp_dbm": nr.and_then(|n| n.rsrp_dbm),
        "nr_sinr_db": nr.and_then(|n| n.sinr_db),
        "nr_band": nr.and_then(|n| n.band.clone()),
        "band": s.signal.band,
        "dl_mhz": s.signal.dl_mhz,
        "pci": s.signal.pci,
        "dl_bps": s.traffic.dl_rate_bytes,
        "ul_bps": s.traffic.ul_rate_bytes,
        "session_dl_bytes": s.traffic.session_dl_bytes,
        "session_ul_bytes": s.traffic.session_ul_bytes,
        "clients": s.clients.len(),
        "net_mode": s.net_mode.network_mode,
        "dns_primary": s.dns.as_ref().and_then(|d| d.primary.clone()),
        "generated_at": s.generated_at,
    })
}

#[cfg(test)]
mod status_tests {
    use super::*;

    fn snap_with(rsrp: Option<f32>, endc: bool, nr_rsrp: Option<f32>) -> Snapshot {
        let mut s = Snapshot::default();
        s.signal.rsrp_dbm = rsrp;
        s.signal.band = Some("B1".into());
        s.signal.pci = Some(481);
        s.connection.net_type = "LTE".into();
        s.connection.endc = endc;
        s.connection.nr_bars = 2;
        s.plmn.name = "Test Net".into();
        s.plmn.numeric = "00101".into();
        if let Some(v) = nr_rsrp {
            s.signal.nr = Some(crate::model::Nr {
                rsrp_dbm: Some(v),
                rsrq_db: Some(-12.0),
                sinr_db: Some(4.0),
                band: Some("N41".into()),
                ..Default::default()
            });
        }
        s
    }

    #[test]
    fn waybar_payload_matches_the_documented_shape() {
        let s = snap_with(Some(-96.0), false, None);
        let w = status_waybar(Some(&s), None);
        assert!(w.text.starts_with("LTE ▄"), "{}", w.text);
        assert_eq!(w.class, "fair");
        assert_eq!(w.percentage, rsrp_percent(-96.0));
        assert!((0..=100).contains(&w.percentage));
        assert!(w.tooltip.contains("Test Net") && w.tooltip.contains("B1"));
        assert!(w.text.chars().count() <= 24, "bar text must stay short");
        let json = serde_json::to_value(&w).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            keys.len(),
            4,
            "exactly text/tooltip/class/percentage: {keys:?}"
        );
    }

    #[test]
    fn waybar_prefers_nr_when_endc_carries_it() {
        let s = snap_with(Some(-96.0), true, Some(-115.0));
        let w = status_waybar(Some(&s), None);
        assert!(w.text.starts_with("5G"), "{}", w.text);
        assert_eq!(w.class, "poor");
        assert!(w.tooltip.contains("NR  RSRP -115"));
    }

    #[test]
    fn offline_is_a_class_not_an_empty_string() {
        let w = status_waybar(None, Some("network: timed out"));
        assert_eq!(w.class, "offline");
        assert_eq!(w.percentage, 0);
        assert!(w.tooltip.contains("timed out"));
        assert!(!w.text.is_empty());
    }

    #[test]
    fn percentage_spans_the_whole_domain() {
        assert_eq!(rsrp_percent(-140.0), 0);
        assert_eq!(rsrp_percent(-44.0), 100);
        assert_eq!(rsrp_percent(-92.0), 50);
        assert_eq!(rsrp_percent(-200.0), 0, "clamped below the domain");
        assert_eq!(rsrp_percent(0.0), 100, "clamped above the domain");
    }

    #[test]
    fn status_json_is_flat_and_secret_free() {
        let s = snap_with(Some(-96.0), false, None);
        let v = status_json(&s);
        assert_eq!(v["band"], "B1");
        assert_eq!(v["operator"], "Test Net");
        assert!(v["rsrp_dbm"].is_number());
        let text = serde_json::to_string(&v).unwrap();
        for banned in ["password", "cookie", "SessionID", "sid"] {
            assert!(
                !text.to_lowercase().contains(&banned.to_lowercase()),
                "{banned} must never appear"
            );
        }
        assert!(!text.contains('\n'), "status --json is one line");
    }
}

#[cfg(test)]
mod phase_tests {
    use super::*;

    #[test]
    fn a_snapshot_without_a_sensor_omits_the_temperature_key() {
        let cold = Snapshot::default();
        let json = serde_json::to_string(&cold).expect("serialize");
        assert!(
            !json.contains("temperature"),
            "absent temperature must not be serialized: {json}"
        );

        let warm = Snapshot {
            temperature: Some(41.0),
            ..Snapshot::default()
        };
        let json = serde_json::to_string(&warm).expect("serialize");
        assert!(json.contains("\"temperature\":41.0"), "{json}");
    }

    #[test]
    fn a_phase_is_running_before_the_work_and_done_after_it() {
        let mut seen = Vec::new();
        let mut trace = |p: Phase, s: PhaseState| seen.push((p, s));
        let value = step(&mut trace, Phase::Signal, || 7);
        assert_eq!(value, 7);
        assert_eq!(
            seen,
            vec![
                (Phase::Signal, PhaseState::Running),
                (Phase::Signal, PhaseState::Done)
            ]
        );
    }

    #[test]
    fn a_finished_phase_is_reported_even_when_the_work_fails() {
        let mut seen = Vec::new();
        let mut trace = |p: Phase, s: PhaseState| seen.push((p, s));
        let failed: Result<(), &str> = step(&mut trace, Phase::Device, || Err("boom"));
        assert!(failed.is_err());
        assert_eq!(seen.last(), Some(&(Phase::Device, PhaseState::Done)));
    }

    #[test]
    fn the_phase_order_matches_the_endpoints_we_call() {
        let labels: Vec<&str> = Phase::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(
            labels,
            vec![
                "signal", "status", "traffic", "plmn", "device", "net-mode", "clients", "sms",
                "dns", "bands"
            ]
        );
        for (i, phase) in Phase::ALL.iter().enumerate() {
            assert_eq!(phase.index(), i, "{phase:?} is out of order");
        }
    }
}
