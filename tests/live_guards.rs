//! Guards for the live-UI round: these pass today and hold the behaviour a
//! faster poll would have to preserve.
//!
//! Everything here goes through the public library surface (`tests/` cannot see
//! `#[cfg(test)] mod mock_cpe`), so the fake firmware below is a plain TCP
//! listener that answers every `/api/` path with one document. That makes each
//! claim below a measurement of this tree, not an estimate.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use routwatch::headless::{self, Phase, PhaseState};
use routwatch::model::Snapshot;
use routwatch::router::endpoints::PROBE;
use routwatch::router::HuaweiClient;
use routwatch::signal_history;
use routwatch::write::{VerifyLimits, VERIFY_REREAD, VERIFY_WINDOW_DEFAULT};

// ---------------------------------------------------------------- temp dirs

/// A directory that removes itself, so a failing assertion does not leave a
/// signal log behind in the scratch space.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "rw-live-guard-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).expect("temp dir");
        TempDir(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn log(&self) -> PathBuf {
        self.0.join("signal-history.jsonl")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ------------------------------------------------------------ fake firmware

/// One TCP listener that answers every request with one document and records
/// what it was asked for. `routes` overrides the default per exact path.
struct Stub {
    addr: String,
    log: Arc<Mutex<Vec<(String, String)>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Stub {
    fn with_default(routes: Vec<(&'static str, &'static str)>, default_body: &'static str) -> Stub {
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
                let body = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, b)| *b)
                    .unwrap_or(default_body);
                seen.lock()
                    .expect("log")
                    .push((method.clone(), path.clone()));
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
        HuaweiClient::new(&self.addr, 3_000)
    }

    fn seen(&self) -> Vec<(String, String)> {
        self.log.lock().expect("log").clone()
    }

    fn paths(&self) -> Vec<String> {
        self.seen().into_iter().map(|(_, p)| p).collect()
    }

    fn posts(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .filter(|(m, _)| m == "POST")
            .map(|(_, p)| p)
            .collect()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock `accept`; the thread then sees the flag and leaves.
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

fn respond(stream: &mut TcpStream, body: &str) {
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

/// The document login needs: a session cookie and a verification token.
const SES_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><SesInfo>SessionID=stub-session</SesInfo><TokInfo>stub-token</TokInfo></response>";
const OK_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>OK</response>";

fn login_stub() -> Stub {
    Stub::with_default(
        vec![
            ("/api/webserver/SesTokInfo", SES_DOC),
            ("/api/user/login", OK_DOC),
            ("/api/user/logout", OK_DOC),
        ],
        "<response/>",
    )
}

// ------------------------------------------------------------------ helpers

fn snapshot_with_signal() -> Snapshot {
    let mut s = Snapshot::default();
    s.signal.rsrp_dbm = Some(-95.0);
    s.signal.rsrq_db = Some(-11.0);
    s.signal.sinr_db = Some(12.0);
    s.signal.band = Some("B3".to_string());
    s.connection.net_type = "LTE".to_string();
    s
}

fn count_lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

// ------------------------------------------------------------------- guards

#[test]
fn the_signal_log_writes_one_row_a_minute() {
    assert_eq!(
        signal_history::SIGNAL_SAMPLE_SECS,
        60,
        "a row every poll would be thirty rows a minute describing the same cell"
    );
    let dir = TempDir::new("one-row");
    let log = dir.log();
    let snap = snapshot_with_signal();

    assert!(
        signal_history::record_if_due(&snap, 1_000, &log).expect("first write"),
        "the first sample of a fresh log is due"
    );
    assert!(
        !signal_history::record_if_due(&snap, 1_030, &log).expect("same minute"),
        "a sample 30s later is the same minute and must not add a row"
    );
    assert!(
        !signal_history::record_if_due(&snap, 1_059, &log).expect("last second"),
        "a row is written only once the minute has passed"
    );
    assert!(
        signal_history::record_if_due(&snap, 1_060, &log).expect("next minute"),
        "60s later the minute is over and the row is due"
    );
    assert_eq!(count_lines(&log), 2, "two minutes of samples, two rows");
}

#[test]
fn the_signal_log_cap_is_still_1440() {
    assert_eq!(signal_history::CAP, 1440, "about a day at one row a minute");
    let dir = TempDir::new("cap");
    let log = dir.log();
    let mut body = String::new();
    for i in 0..(signal_history::CAP + 60) as u64 {
        let row = signal_history::Row {
            v: signal_history::VERSION,
            at_unix: 1_000 + i * 60,
            net_type: Some("LTE".to_string()),
            band: Some("B3".to_string()),
            nr_band: None,
            rsrp_dbm: Some(-95.0),
            rsrq_db: None,
            sinr_db: None,
            pci: None,
            endc: None,
        };
        body.push_str(&serde_json::to_string(&row).expect("row json"));
        body.push('\n');
    }
    std::fs::write(&log, body).expect("seed log");

    let kept = signal_history::compact_if_needed(&log, signal_history::CAP).expect("trim");
    assert_eq!(
        kept,
        signal_history::CAP,
        "the cap is applied, not declared"
    );
    assert_eq!(count_lines(&log), signal_history::CAP);
}

#[test]
fn a_read_only_path_posts_nothing_beyond_login_and_logout() {
    let stub = login_stub();
    let mut client = stub.client();
    client
        .login("stub-user", "stub-pass")
        .expect("the stub accepts a login");
    for spec in PROBE {
        let _ = client.get(spec.path);
    }
    client.logout();

    assert_eq!(
        stub.posts(),
        vec![
            "/api/user/login".to_string(),
            "/api/user/logout".to_string()
        ],
        "reads must be GETs; only the session pair may POST"
    );
}

#[test]
fn the_verify_defaults_are_five_and_twenty() {
    assert_eq!(VERIFY_REREAD.as_secs(), 5, "one reread every five seconds");
    assert_eq!(
        VERIFY_WINDOW_DEFAULT.as_secs(),
        20,
        "a twenty second window"
    );
    let limits = VerifyLimits::default();
    assert_eq!(limits.reread.as_secs(), 5);
    assert_eq!(limits.window.as_secs(), 20);
}

#[test]
fn a_test_run_never_touches_the_real_signal_log() {
    let dir = TempDir::new("xdg");
    // `dirs::data_dir()` honours XDG_DATA_HOME on Linux, which is what lets a
    // test own the log path without touching the user's data directory.
    std::env::set_var("XDG_DATA_HOME", dir.path());
    let resolved = signal_history::path().expect("a data dir resolves");
    assert!(
        resolved.starts_with(dir.path()),
        "the log path follows XDG_DATA_HOME, got {resolved:?}"
    );
    assert!(
        signal_history::record_if_due(&snapshot_with_signal(), 1_000, &resolved)
            .expect("write under the temp data home"),
        "the first row lands under the redirected data home"
    );
    let written = std::fs::read_to_string(&resolved).expect("log body");
    assert!(
        written.contains("at_unix"),
        "the row is the log format, not an empty file"
    );
}

#[test]
fn the_wan_helper_reads_both_documents() {
    // The first document answers without the address, so the helper must go on
    // to the second one: a firmware that keeps the WAN block in either place.
    let stub = Stub::with_default(
        vec![
            ("/api/webserver/SesTokInfo", SES_DOC),
            ("/api/user/login", OK_DOC),
            (
                "/api/device/information",
                "<response><WanIPAddress>203.0.113.7</WanIPAddress></response>",
            ),
        ],
        "<response/>",
    );
    let mut client = stub.client();
    client.login("stub-user", "stub-pass").expect("login");
    let addr = routwatch::router::parse::wan_addr(&mut client).expect("the read itself succeeds");
    assert_eq!(
        addr.as_deref(),
        Some("203.0.113.7"),
        "the address lives in device/information here"
    );
    let paths = stub.paths();
    assert!(
        paths.iter().any(|p| p == "/api/monitoring/status"),
        "the first document is read first: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "/api/device/information"),
        "the second document is read when the first has no address: {paths:?}"
    );
}

// --------------------------------------------------- characterisation (pins)

/// The round assumed `collect` walks `PROBE`. It does not: it issues its own
/// hand-written list of reads. This test pins the real list so a faster tier is
/// built on what the code does, not on what the constant suggests.
#[test]
fn collect_reads_its_own_endpoint_list_not_probe() {
    let stub = login_stub();
    let mut client = stub.client();
    client.login("stub-user", "stub-pass").expect("login");

    let mut phases = 0usize;
    let snap = headless::collect_traced(&mut client, &stub.addr, &mut |p: Phase, s: PhaseState| {
        if s == PhaseState::Done {
            phases += 1;
        }
        let _ = p;
    })
    .expect("the stub answers every read");

    let read: Vec<String> = stub
        .paths()
        .into_iter()
        .filter(|p| !p.starts_with("/api/user/") && p != "/api/webserver/SesTokInfo")
        .collect();
    let probe_paths: Vec<String> = PROBE.iter().map(|s| format!("/api/{}", s.path)).collect();
    let outside: Vec<&String> = read.iter().filter(|p| !probe_paths.contains(p)).collect();

    println!("collect read {} paths, phases done {}", read.len(), phases);
    for p in &read {
        println!("  {p}");
    }
    assert!(
        !read.is_empty(),
        "collect issues no reads the stub can see: the harness is wrong"
    );
    assert!(
        !outside.is_empty(),
        "collect reads only PROBE paths now, so the split can be derived from PROBE: {outside:?}"
    );
    assert!(
        snap.router == stub.addr,
        "the snapshot names the router it was read from"
    );
    assert!(
        phases >= 8,
        "a snapshot announces its phases: {phases} Done events seen"
    );
}

/// `record_if_due` takes the sidecar lock and reads the tail of the file on
/// every call, even when it decides not to write. The cost of that is measured
/// here, because a fast tier multiplies it.
#[test]
fn the_signal_log_call_cost_is_measured() {
    let dir = TempDir::new("cost");
    let log = dir.log();
    let snap = snapshot_with_signal();
    signal_history::record_if_due(&snap, 1_000, &log).expect("seed");

    let calls = 1_000u32;
    let started = Instant::now();
    let mut wrote = 0u32;
    for _ in 0..calls {
        if signal_history::record_if_due(&snap, 1_030, &log).expect("call") {
            wrote += 1;
        }
    }
    let per_call = started.elapsed().as_micros() as f64 / f64::from(calls);
    println!("signal log: {per_call:.1} us per call, {wrote} writes in {calls} calls");
    assert_eq!(wrote, 0, "every call is inside the same minute");
    assert_eq!(count_lines(&log), 1, "no row is added inside the minute");
    assert!(
        per_call < 5_000.0,
        "a call that costs 5 ms is a call to cache"
    );
}
