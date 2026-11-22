//! The advisor is a pure function, so most of these tests hand it rows and
//! read the advice back. Two of them run the command, because "it reads only"
//! and "it writes nothing" are claims about a process, not about a function.
//!
//! Every value here is synthetic: a made-up band, a made-up moment.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use routwatch::advisor::{advise, Advice, Confidence, Input, MIN_ROWS, WINDOW_SECS};
use routwatch::history::Entry;
use routwatch::scan::{Outcome, SetResult};
use routwatch::signal_history::{Row, VERSION as ROW_VERSION};

const NOW: u64 = 1_700_000_000;

fn row(at: u64, band: &str, rsrp: f32) -> Row {
    Row {
        v: ROW_VERSION,
        at_unix: at,
        net_type: None,
        band: Some(band.into()),
        nr_band: None,
        rsrp_dbm: Some(rsrp),
        rsrq_db: None,
        sinr_db: Some(10.0),
        pci: None,
        endc: None,
        earfcn_dl: None,
        earfcn_ul: None,
        tm: None,
    }
}

fn speed(at: u64, band: &str, mbps: f64) -> Entry {
    let mut e = Entry::blank();
    e.started_unix = at;
    e.ended_unix = at + 1;
    e.download_bps = mbps * 1_000_000.0;
    e.upload_bps = 1_000_000.0;
    e.band = Some(band.into());
    e.band_at_start = Some(band.into());
    e
}

fn measured(bands: &[u8], mbps: f64) -> SetResult {
    SetResult {
        bands: bands.to_vec(),
        outcome: Outcome::Measured {
            down_bps: mbps * 1_000_000.0,
            up_bps: None,
            ttfb_ms: 10.0,
        },
        note: None,
    }
}

struct Case {
    signal: Vec<Row>,
    speed: Vec<Entry>,
    scans: Vec<SetResult>,
    advertised: Vec<u8>,
}

impl Case {
    fn new() -> Case {
        Case {
            signal: Vec::new(),
            speed: Vec::new(),
            scans: Vec::new(),
            advertised: vec![1, 3, 20, 28],
        }
    }

    fn advise(&self) -> Advice {
        advise(&Input {
            signal: &self.signal,
            speed: &self.speed,
            scans: &self.scans,
            advertised: &self.advertised,
            presets: &[],
            now_unix: NOW,
            window_secs: WINDOW_SECS,
            min_rows: MIN_ROWS,
            smear_ms: 2_860,
        })
    }
}

// -------------------------------------------------------------------- purity

#[test]
fn a_served_band_is_ranked_from_the_rows_it_holds() {
    let mut case = Case::new();
    for i in 0..40 {
        case.signal.push(row(NOW - 400 + i, "3", -95.0));
    }
    for i in 0..10 {
        case.signal.push(row(NOW - 300 + i, "B20", -110.0));
    }
    case.speed.push(speed(NOW - 100, "3", 42.0));
    let advice = case.advise();
    assert_eq!(advice.rows_used, 50);
    assert_eq!(advice.served.len(), 2);
    assert_eq!(advice.served[0].band, "3", "the band with more rows leads");
    assert_eq!(advice.served[0].rows, 40);
    assert!(
        (advice.served[0].share - 0.8).abs() < 0.001,
        "{:?}",
        advice.served[0]
    );
    assert_eq!(advice.served[0].measurements, 1);
    assert_eq!(advice.served[0].download_median_mbps, Some(42.0));
    assert_eq!(
        advice.served[1].band, "20",
        "a prefix is a spelling, not a band"
    );
}

#[test]
fn a_row_inside_the_smear_of_a_change_is_not_attributed() {
    let mut case = Case::new();
    for i in 0..MIN_ROWS as u64 {
        case.signal.push(row(NOW - 400 + i, "3", -95.0));
    }
    for i in 0..MIN_ROWS as u64 {
        case.signal.push(row(NOW - 300 + i, "20", -110.0));
    }
    // One read a second after the change back: inside the measured smear, so
    // it may not be read as B3's behaviour.
    case.signal
        .push(row(NOW - 300 + MIN_ROWS as u64 + 1, "3", -95.0));
    let advice = case.advise();
    assert_eq!(advice.dropped_in_smear, 1, "{:?}", advice.notes);
    let mut served: Vec<(&str, usize)> = advice
        .served
        .iter()
        .map(|s| (s.band.as_str(), s.rows))
        .collect();
    served.sort();
    assert_eq!(served, vec![("20", MIN_ROWS), ("3", MIN_ROWS)]);
}

#[test]
fn the_window_bounds_what_is_counted() {
    let mut case = Case::new();
    for i in 0..MIN_ROWS as u64 {
        case.signal.push(row(NOW - 10_000 + i, "3", -95.0));
    }
    let advice = advise(&Input {
        signal: &case.signal,
        speed: &[],
        scans: &[],
        advertised: &case.advertised,
        presets: &[],
        now_unix: NOW,
        window_secs: 60,
        min_rows: MIN_ROWS,
        smear_ms: 2_860,
    });
    assert_eq!(advice.rows_used, 0);
    assert_eq!(advice.confidence, Confidence::Insufficient);
    assert!(
        advice.missing.iter().any(|m| m.contains("rows")),
        "what is missing is named: {:?}",
        advice.missing
    );
}

// --------------------------------------------------------------- set advice

#[test]
fn set_advice_is_a_median_of_the_measured_scans_only() {
    let mut case = Case::new();
    case.scans.push(measured(&[1, 3], 60.0));
    case.scans.push(measured(&[1, 3], 40.0));
    case.scans.push(SetResult {
        bands: vec![20, 28],
        outcome: Outcome::NoService,
        note: None,
    });
    let advice = case.advise();
    assert_eq!(advice.sets.len(), 1, "an unmeasured set carries no number");
    assert_eq!(advice.sets[0].bands, vec![1, 3]);
    assert_eq!(advice.sets[0].measurements, 2);
    assert_eq!(advice.sets[0].download_median_mbps, Some(50.0));
}

#[test]
fn a_set_the_firmware_does_not_advertise_is_marked_not_hidden() {
    let mut case = Case::new();
    case.advertised = vec![1, 3];
    case.scans.push(measured(&[20, 28], 55.0));
    // A band the firmware does not list, seen in the log: the advice says so.
    for i in 0..MIN_ROWS as u64 {
        case.signal.push(row(NOW - 300 + i, "20", -110.0));
    }
    for i in 0..MIN_ROWS as u64 {
        case.signal.push(row(NOW - 200 + i, "3", -95.0));
    }
    let advice = case.advise();
    assert_eq!(advice.sets.len(), 1);
    assert!(!advice.sets[0].advertised, "the firmware does not list it");
    assert!(
        advice
            .missing
            .iter()
            .any(|m| m.contains("B20") && m.contains("not in the advertised list")),
        "{:?}",
        advice.missing
    );
}

#[test]
fn the_advice_always_says_what_it_cannot_say() {
    let mut case = Case::new();
    for i in 0..MIN_ROWS as u64 {
        case.signal.push(row(NOW - 300 + i, "3", -95.0));
    }
    let notes = case.advise().notes;
    assert!(
        notes.iter().any(|n| n.contains("served, not the set")),
        "{notes:?}"
    );
    assert!(notes.iter().any(|n| n.contains("associated")), "{notes:?}");
}

#[test]
fn confidence_follows_the_evidence_and_never_exceeds_it() {
    let mut case = Case::new();
    for i in 0..MIN_ROWS as u64 {
        case.signal.push(row(NOW - 300 + i, "3", -95.0));
    }
    let advice = case.advise();
    assert_eq!(advice.served_confidence, Confidence::Low);
    assert_eq!(advice.set_confidence, Confidence::Insufficient);
    assert_eq!(
        advice.confidence,
        Confidence::Insufficient,
        "no scan means the weakest link decides"
    );
    assert_eq!(advice.confidence.as_str(), "insufficient");
}

// ------------------------------------------------------- the process itself

const SES_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><SesInfo>SessionID=stub-session</SesInfo><TokInfo>stub-token</TokInfo></response>";
const OK_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>OK</response>";
const BANDS_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><LTEBandList><LTEBand><Name>LTE BC1</Name><Value>0x1</Value></LTEBand><LTEBand><Name>LTE BC3</Name><Value>0x4</Value></LTEBand></LTEBandList></response>";

struct Stub {
    addr: String,
    log: Arc<Mutex<Vec<(String, String)>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Stub {
    fn start() -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let addr = listener.local_addr().expect("addr").to_string();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&log);
        let halt = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if halt.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = conn else { continue };
                let Some((method, path, _body)) = read_request(&mut stream) else {
                    continue;
                };
                seen.lock()
                    .expect("log")
                    .push((method.clone(), path.clone()));
                let (status, body) = match path.as_str() {
                    "/api/webserver/SesTokInfo" => (200, SES_DOC.to_string()),
                    "/api/user/login" | "/api/user/logout" => (200, OK_DOC.to_string()),
                    "/api/net/net-mode-list" => (200, BANDS_DOC.to_string()),
                    _ => (
                        404,
                        "<error><code>125002</code><message>not found</message></error>"
                            .to_string(),
                    ),
                };
                respond(&mut stream, status, &body);
            }
        });
        Stub {
            addr,
            log,
            stop,
            handle: Some(handle),
        }
    }

    fn posts(&self) -> Vec<String> {
        self.log
            .lock()
            .expect("log")
            .iter()
            .filter(|(m, _)| m == "POST")
            .map(|(_, p)| p.clone())
            .collect()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(&self.addr);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<(String, String, String)> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..n]);
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let want: usize = lines
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split_once(':').map(|(_, v)| v.trim().parse().ok()))
        .flatten()
        .unwrap_or(0);
    let mut body = raw[head_end..].to_vec();
    while body.len() < want {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, path, String::from_utf8_lossy(&body).to_string()))
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reply = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> TempDir {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rw-advice-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    TempDir(dir)
}

fn config(home: &TempDir, ip: &str) -> PathBuf {
    let path = home.0.join("config.toml");
    std::fs::write(
        &path,
        format!(
            "[router]\nip = \"{ip}\"\nusername = \"admin\"\n\n[app]\nrefresh_ms = 2000\ntimeout_ms = 3000\ntheme = \"terminal\"\nview = \"full\"\ngraph_style = \"dots\"\nbudget_gb = 100\n"
        ),
    )
    .expect("write config");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    path
}

fn run(home: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_routwatch"))
        .args(args)
        .env("ROUTWATCH_PASSWORD", "test-placeholder")
        .env("XDG_DATA_HOME", &home.0)
        .env("XDG_CONFIG_HOME", &home.0)
        .output()
        .expect("spawn routwatch")
}

fn signal_log(home: &TempDir) -> PathBuf {
    let dir = home.0.join("routwatch");
    std::fs::create_dir_all(&dir).expect("data dir");
    dir.join("signal-history.jsonl")
}

fn write_rows(home: &TempDir, rows: &[Row]) {
    let lines: Vec<String> = rows
        .iter()
        .map(|r| serde_json::to_string(r).expect("row json"))
        .collect();
    std::fs::write(signal_log(home), lines.join("\n") + "\n").expect("write log");
}

#[test]
fn the_advice_posts_nothing_beyond_login_and_logout() {
    let stub = Stub::start();
    let home = scratch("posts");
    let cfg = config(&home, &stub.addr);
    let now = routwatch::record::now_unix();
    let mut rows = Vec::new();
    for i in 0..MIN_ROWS as u64 {
        rows.push(row(now - 300 + i, "3", -95.0));
    }
    write_rows(&home, &rows);
    let out = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "bands", "advise"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stub.posts(), vec!["/api/user/login", "/api/user/logout"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("served bands"), "{text}");
    assert!(text.contains("30 rows"), "{text}");
}

#[test]
fn the_advice_writes_no_file_and_survives_an_unreachable_router() {
    let home = scratch("offline");
    let cfg = config(&home, "127.0.0.1:1");
    let now = routwatch::record::now_unix();
    let mut rows = Vec::new();
    for i in 0..MIN_ROWS as u64 {
        rows.push(row(now - 300 + i, "3", -95.0));
    }
    write_rows(&home, &rows);
    let before = std::fs::read(signal_log(&home)).expect("log");
    let out = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "bands", "advise"],
    );
    assert!(
        out.status.success(),
        "an unreachable router narrows the advice, it does not stop it: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("the router was not read"), "{text}");
    assert!(text.contains("30 rows"), "{text}");
    assert_eq!(
        std::fs::read(signal_log(&home)).expect("log"),
        before,
        "the advisor reads the log and never rewrites it"
    );
    assert!(
        !home.0.join("routwatch").join("events.jsonl").exists(),
        "advice is not an event"
    );
}

#[test]
fn a_scan_file_is_the_only_way_set_advice_gets_a_number() {
    let home = scratch("scans");
    let cfg = config(&home, "127.0.0.1:1");
    let now = routwatch::record::now_unix();
    let mut rows = Vec::new();
    for i in 0..MIN_ROWS as u64 {
        rows.push(row(now - 300 + i, "3", -95.0));
    }
    write_rows(&home, &rows);
    let scans = home.0.join("scan.json");
    let text = serde_json::to_string(&vec![measured(&[1, 3], 61.0), measured(&[1, 3], 59.0)])
        .expect("scan json");
    std::fs::write(&scans, text).expect("write scans");
    let out = run(
        &home,
        &[
            "--config",
            cfg.to_str().unwrap(),
            "bands",
            "advise",
            "--scans",
            scans.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("the advisor prints json");
    let sets = &parsed["sets"];
    assert_eq!(sets.as_array().map(|a| a.len()), Some(1), "{parsed}");
    assert_eq!(sets[0]["measurements"], 2);
    assert_eq!(sets[0]["download_median_mbps"], 60.0);
}

#[test]
fn an_idle_gap_does_not_mute_the_whole_window() {
    // A log can pause for hours (the TUI was closed). That pause is absence of
    // observation, not smear: if it set the smear, every real change would be
    // read as unattributable and the advice would say nothing.
    let home = scratch("smear");
    let cfg = config(&home, "127.0.0.1:1");
    let now = routwatch::record::now_unix();
    let mut rows = Vec::new();
    for i in 0..MIN_ROWS as u64 {
        rows.push(row(now - 7_260 + i, "3", -95.0));
    }
    for i in 0..MIN_ROWS as u64 {
        rows.push(row(now - 300 + i, "20", -108.0));
    }
    write_rows(&home, &rows);
    let out = run(
        &home,
        &[
            "--config",
            cfg.to_str().unwrap(),
            "bands",
            "advise",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("the advisor prints json");
    let smear = parsed["smear_ms"].as_u64().expect("smear is a number");
    assert!(
        smear <= 5_000,
        "a two hour idle gap set the smear to {smear} ms: {parsed}"
    );
    assert_eq!(
        parsed["rows_used"].as_u64(),
        Some(2 * MIN_ROWS as u64),
        "both dense runs are inside the window"
    );
}
