//! The cells catalog, tested end to end against a fake firmware on a real socket.
//!
//! The claim under test is narrow and checkable: one radio read leaves one row in
//! `cells.json`, with the fields the firmware actually answered.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

mod common;

use common::isolate_data_home;
use routwatch::cells::read_records;
use routwatch::headless::{self, PhaseState};
use routwatch::model::Snapshot;
use routwatch::router::HuaweiClient;
use routwatch::worker::Tier;

const SES_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><SesInfo>SessionID=stub-session</SesInfo><TokInfo>stub-token</TokInfo></response>";
const OK_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>OK</response>";
const CELL_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><rsrp>-95dBm</rsrp><rsrq>-11dB</rsrq><sinr>9dB</sinr><band>1</band><pci>481</pci><earfcn>DL:100 UL:18100</earfcn><tac>516</tac></response>";

struct Stub {
    addr: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    _log: Arc<Mutex<Vec<String>>>,
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
                    "/api/device/signal" => CELL_DOC,
                    _ => "<response/>",
                };
                seen.lock().expect("log").push(path);
                respond(&mut stream, body);
            }
        });
        Stub {
            addr,
            stop,
            handle: Some(handle),
            _log: log,
        }
    }

    fn client(&self) -> HuaweiClient {
        let mut c = HuaweiClient::new(&self.addr, 3_000);
        c.login("admin", "pw").expect("stub login");
        c
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

#[test]
fn a_radio_read_records_the_serving_cell() {
    let home = isolate_data_home("a_radio_read_records_the_serving_cell");
    let cells: PathBuf = home.data_dir().join("cells.json");
    let log: PathBuf = home.data_dir().join("signal-history.jsonl");

    // The log switch must not gate the catalog: the catalog answers which cell was
    // read, which is a fact even when a script asked for no minute rows.
    std::env::set_var("ROUTWATCH_DISABLE_SIGNAL_LOG", "1");
    let stub = Stub::start();
    let mut c = stub.client();
    let mut snap = Snapshot::default();
    let at = 1_700_000_000u64;
    {
        let now = || at;
        let mut trace = |_: headless::Phase, _: PhaseState| {};
        headless::collect_tier(&mut c, &mut snap, Tier::Radio, &now, &mut trace)
            .expect("radio read");
    }

    assert!(
        !log.exists(),
        "the switch still silences the minute log, so the two are separate"
    );
    let (records, warn) = read_records(&cells);
    assert_eq!(warn, None, "a fresh catalog reads clean");
    assert_eq!(records.len(), 1, "one radio read, one cell");
    let r = &records[0];
    assert_eq!(r.band, "B1");
    assert_eq!(r.pci, 481);
    assert_eq!(r.earfcn_dl, Some(100));
    assert_eq!(r.earfcn_ul, Some(18100));
    assert_eq!(r.tac, Some(516));
    assert_eq!(r.rows, 1);
    assert_eq!(r.first_seen, at);
    assert_eq!(r.last_seen, at);
    assert_eq!(r.best_sinr, Some(9));
    assert!(!r.earfcn_unknown);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cells)
            .expect("file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the catalog is private");
    }
    std::env::remove_var("ROUTWATCH_DISABLE_SIGNAL_LOG");
}
