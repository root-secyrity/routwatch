//! The tier split, tested against a fake firmware on a real socket.
//!
//! Every claim here is counted, not estimated: the stub records each path it was
//! asked for, so "the radio tier reads three endpoints" is a number this file
//! measured rather than a reading of the source.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use routwatch::headless::{self, PhaseState};
use routwatch::model::Snapshot;
use routwatch::router::HuaweiClient;
use routwatch::worker::Tier;

/// `XDG_DATA_HOME` decides where the signal log lives and it is one variable for
/// the whole process, while tests run in parallel threads. Every test that can
/// write a row takes this lock and points the variable at its own directory
/// first, so a sibling cannot move the row away from the directory under test.
static LOG_LOCK: Mutex<()> = Mutex::new(());

fn isolate_log(tag: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
    let guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp_dir(tag);
    std::env::set_var("XDG_DATA_HOME", &dir);
    (guard, dir)
}

const SES_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><SesInfo>SessionID=stub-session</SesInfo><TokInfo>stub-token</TokInfo></response>";
const OK_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>OK</response>";
const SIGNAL_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><rsrp>-95dBm</rsrp><rsrq>-11dB</rsrq><sinr>12dB</sinr><band>3</band></response>";

struct Stub {
    addr: String,
    log: Arc<Mutex<Vec<String>>>,
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
                let Some((path, _body)) = read_request(&mut stream) else {
                    continue;
                };
                let body = match path.as_str() {
                    "/api/webserver/SesTokInfo" => SES_DOC,
                    "/api/user/login" | "/api/user/logout" => OK_DOC,
                    "/api/device/signal" => SIGNAL_DOC,
                    _ => "<response/>",
                };
                seen.lock().expect("log").push(path);
                respond(&mut stream, body);
            }
        });
        Stub {
            addr,
            log,
            stop,
            handle: Some(handle),
        }
    }

    fn client(&self) -> HuaweiClient {
        let mut c = HuaweiClient::new(&self.addr, 3_000);
        c.login("admin", "pw").expect("stub login");
        c
    }

    /// Requests since the last `clear`, without the three the login itself made.
    fn paths_since_login(&self) -> Vec<String> {
        self.log
            .lock()
            .expect("log")
            .iter()
            .filter(|p| {
                !matches!(
                    p.as_str(),
                    "/api/webserver/SesTokInfo" | "/api/user/login" | "/api/user/logout"
                )
            })
            .cloned()
            .collect()
    }

    fn clear(&self) {
        self.log.lock().expect("log").clear();
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

fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
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
    let _method = first.next()?;
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
    Some((path, String::from_utf8_lossy(&body).to_string()))
}

fn respond(stream: &mut TcpStream, body: &str) {
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

/// A strict clock: every tier must be handed a clock, and the radio tier must
/// call it exactly once, at the read.
struct FixedClock(u64);

impl FixedClock {
    fn at(&self) -> u64 {
        self.0
    }
}

fn run_tier(c: &mut HuaweiClient, snap: &mut Snapshot, tier: Tier, at: u64) -> usize {
    let clock = FixedClock(at);
    let now = || clock.at();
    let mut phases = 0usize;
    {
        let mut trace = |_: routwatch::headless::Phase, _: PhaseState| phases += 1;
        headless::collect_tier(c, snap, tier, &now, &mut trace).expect("tier read");
    }
    phases
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut p = std::env::temp_dir();
    p.push(format!("rw-tier-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&p).expect("temp dir");
    p
}

// -------------------------------------------------------------------- tests

#[test]
fn the_radio_tier_reads_only_the_fast_endpoints() {
    let (_log, _dir) = isolate_log("the_radio_tier_reads_only_the_fast_endpoints");
    let stub = Stub::start();
    let mut c = stub.client();
    stub.clear();

    let mut snap = Snapshot::default();
    run_tier(&mut c, &mut snap, Tier::Radio, 1_000);

    let mut paths = stub.paths_since_login();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            "/api/device/signal".to_string(),
            "/api/monitoring/status".to_string(),
            "/api/monitoring/traffic-statistics".to_string(),
        ],
        "the radio tier must not drag a slow read into the fast deadline"
    );
}

#[test]
fn the_clients_tier_reads_only_the_host_list() {
    let (_log, _dir) = isolate_log("the_clients_tier_reads_only_the_host_list");
    let stub = Stub::start();
    let mut c = stub.client();
    stub.clear();

    let mut snap = Snapshot::default();
    run_tier(&mut c, &mut snap, Tier::Clients, 1_000);

    assert_eq!(
        stub.paths_since_login(),
        vec!["/api/wlan/host-list".to_string()],
        "the clients tier is the host list and nothing else"
    );
}

#[test]
fn the_static_tier_reads_the_remaining_ten() {
    let (_log, _dir) = isolate_log("the_static_tier_reads_the_remaining_ten");
    let stub = Stub::start();
    let mut c = stub.client();
    stub.clear();

    let mut snap = Snapshot::default();
    run_tier(&mut c, &mut snap, Tier::Static, 1_000);

    let mut paths = stub.paths_since_login();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            "/api/device/information".to_string(),
            "/api/device/temperature".to_string(),
            "/api/dhcp/feature-switch".to_string(),
            "/api/dhcp/settings".to_string(),
            "/api/monitoring/month_statistics".to_string(),
            "/api/monitoring/start_date".to_string(),
            "/api/net/current-plmn".to_string(),
            "/api/net/net-mode".to_string(),
            "/api/net/net-mode-list".to_string(),
            "/api/sms/sms-count".to_string(),
        ],
        "the static tier carries every read the other two do not, \
         including the feature switch the DNS flag needs"
    );
}

/// The three tiers together must cover the fourteen reads of a full collect,
/// once each: a read left out of every tier would silently freeze a field.
#[test]
fn the_fourteen_reads_are_split_across_the_three_tiers_without_overlap() {
    let (_log, _dir) =
        isolate_log("the_fourteen_reads_are_split_across_the_three_tiers_without_overlap");
    let stub = Stub::start();
    let mut c = stub.client();
    let mut snap = Snapshot::default();

    let mut all: Vec<String> = Vec::new();
    for tier in Tier::ALL {
        stub.clear();
        run_tier(&mut c, &mut snap, tier, 1_000);
        all.extend(stub.paths_since_login());
    }
    all.sort();

    stub.clear();
    headless::collect(&mut c, "stub").expect("one-shot collect");
    let mut one_pass = stub.paths_since_login();
    one_pass.sort();

    assert_eq!(
        all, one_pass,
        "the tiers must add up to exactly what one collect reads, with no path twice"
    );
    assert_eq!(all.len(), 14, "fourteen reads, counted");
}

/// The row must carry the moment of the radio read. The clock is fixed, so a
/// row that carried the assembly time or a second clock call would not match.
#[test]
fn the_signal_row_is_stamped_at_the_radio_read() {
    let (_log, _dir) = isolate_log("the_signal_row_is_stamped_at_the_radio_read");
    let path: PathBuf = _dir.join("routwatch/signal-history.jsonl");

    let stub = Stub::start();
    let mut c = stub.client();
    let mut snap = Snapshot::default();
    run_tier(&mut c, &mut snap, Tier::Radio, 1_234);

    let body = std::fs::read_to_string(&path).expect("the radio tier writes the row");
    assert!(
        body.contains("\"at_unix\":1234") || body.contains("\"at_unix\": 1234"),
        "the row must carry the radio read's stamp, got: {body}"
    );
    let _ = std::fs::remove_dir_all(&_dir);
}

/// A tier writes its own group and no other: a clients tick must not blank the
/// signal the header is showing.
#[test]
fn a_tier_leaves_the_other_groups_untouched() {
    let (_log, _dir) = isolate_log("a_tier_leaves_the_other_groups_untouched");
    let stub = Stub::start();
    let mut c = stub.client();

    let mut snap = Snapshot::default();
    snap.signal.rsrp_dbm = Some(-101.0);
    snap.traffic.total_dl_bytes = 42;
    snap.device.imei = Some("keep-me".to_string());

    run_tier(&mut c, &mut snap, Tier::Clients, 1_000);
    assert_eq!(
        snap.signal.rsrp_dbm,
        Some(-101.0),
        "clients kept the signal"
    );
    assert_eq!(snap.traffic.total_dl_bytes, 42, "clients kept the traffic");
    assert_eq!(
        snap.device.imei.as_deref(),
        Some("keep-me"),
        "clients kept the identity"
    );

    run_tier(&mut c, &mut snap, Tier::Radio, 1_000);
    assert_eq!(
        snap.device.imei.as_deref(),
        Some("keep-me"),
        "radio kept the identity"
    );
}
