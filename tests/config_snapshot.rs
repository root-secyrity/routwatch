//! Config snapshots, measured against a fake firmware on a real socket.
//!
//! The claim these tests exist for is the negative one: no setting value reaches
//! the disk. Every document the stub serves carries a sentinel, and each test
//! that writes a file greps the file for it.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const SES_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><SesInfo>SessionID=stub-session</SesInfo><TokInfo>stub-token</TokInfo></response>";
const OK_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>OK</response>";
const REFUSED_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100003</code><message>no right</message></error>";

/// Every value the stub serves that must never be written. The firmware string
/// is the one exception, and it is not in this list.
const SENTINELS: [&str; 6] = [
    "SENTINEL-SERIAL-42",
    "SENTINEL-IMEI-42",
    "SENTINEL-NAME",
    "SENTINEL-SSID",
    "SENTINEL-WIFI-KEY",
    "192.0.2.77",
];

fn device_doc(firmware: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><DeviceName>SENTINEL-NAME</DeviceName><SoftwareVersion>{firmware}</SoftwareVersion><SerialNumber>SENTINEL-SERIAL-42</SerialNumber><Imei>SENTINEL-IMEI-42</Imei></response>"
    )
}

fn wlan_doc() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><Ssid>SENTINEL-SSID</Ssid><WifiKey>SENTINEL-WIFI-KEY</WifiKey><WifiEnable>1</WifiEnable></response>".to_string()
}

fn dhcp_doc(ip: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><DhcpIPAddress>{ip}</DhcpIPAddress><PrimaryDns>203.0.113.53</PrimaryDns><SecondaryDns>203.0.113.54</SecondaryDns><DhcpStartIPAddress>192.0.2.100</DhcpStartIPAddress></response>"
    )
}

fn default_docs() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("/api/device/information".into(), device_doc("11.0.2.21"));
    m.insert("/api/wlan/basic-settings".into(), wlan_doc());
    m.insert("/api/dhcp/settings".into(), dhcp_doc("192.0.2.77"));
    m.insert(
        "/api/dhcp/feature-switch".into(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><DhcpDnsEnable>1</DhcpDnsEnable></response>".into(),
    );
    m.insert(
        "/api/security/firewall-switch".into(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><FirewallMainSwitch>1</FirewallMainSwitch></response>".into(),
    );
    m.insert(
        "/api/security/mac-filter".into(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><MacFilterMode>1</MacFilterMode></response>".into(),
    );
    m.insert(
        "/api/global/module-switch".into(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><ModuleSwitch>1</ModuleSwitch></response>".into(),
    );
    m.insert(
        "/api/net/net-mode".into(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><NetworkMode>03</NetworkMode><LTEBandList><LTEBand>3</LTEBand><LTEBand>1</LTEBand></LTEBandList></response>".into(),
    );
    m
}

struct Stub {
    addr: String,
    docs: Arc<Mutex<BTreeMap<String, String>>>,
    /// When set, every read of `dhcp/settings` answers one of two addresses in
    /// turn, which is the shape of a firmware tail this tool must not call a
    /// setting.
    alternate: Arc<AtomicBool>,
    log: Arc<Mutex<Vec<(String, String)>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Stub {
    fn start() -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let addr = listener.local_addr().expect("addr").to_string();
        let docs = Arc::new(Mutex::new(default_docs()));
        let alternate = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, halt) = (Arc::clone(&log), Arc::clone(&stop));
        let (served, flip, counted) = (
            Arc::clone(&docs),
            Arc::clone(&alternate),
            Arc::clone(&reads),
        );
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
                    "/api/dhcp/settings" if flip.load(Ordering::Relaxed) => {
                        let n = counted.fetch_add(1, Ordering::Relaxed);
                        let ip = if n % 2 == 0 {
                            "192.0.2.77"
                        } else {
                            "192.0.2.78"
                        };
                        (200, dhcp_doc(ip))
                    }
                    other => match served.lock().expect("docs").get(other) {
                        Some(doc) => (200, doc.clone()),
                        None => (
                            404,
                            "<error><code>125002</code><message>not found</message></error>"
                                .to_string(),
                        ),
                    },
                };
                respond(&mut stream, status, &body);
            }
        });
        Stub {
            addr,
            docs,
            alternate,
            log,
            stop,
            handle: Some(handle),
        }
    }

    fn set(&self, path: &str, doc: String) {
        self.docs
            .lock()
            .expect("docs")
            .insert(path.to_string(), doc);
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
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rw-cfgtest-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    TempDir(dir)
}

fn config(dir: &TempDir, ip: &str) -> PathBuf {
    let path = dir.0.join("config.toml");
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

fn snap_dir(home: &TempDir) -> PathBuf {
    home.0.join("routwatch").join("config-snapshots")
}

fn snapshots(home: &TempDir) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(snap_dir(home))
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn snapshot_once(home: &TempDir, cfg: &std::path::Path) -> Output {
    run(
        home,
        &["--config", cfg.to_str().unwrap(), "config", "snapshot"],
    )
}

// -------------------------------------------------------------------- tests

#[test]
fn a_snapshot_keeps_fingerprints_and_no_value() {
    let stub = Stub::start();
    let home = scratch("private");
    let cfg = config(&home, &stub.addr);
    let out = snapshot_once(&home, &cfg);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let files = snapshots(&home);
    assert_eq!(files.len(), 1, "one snapshot written");
    let body = std::fs::read_to_string(&files[0]).expect("read snapshot");
    for sentinel in SENTINELS {
        assert!(
            !body.contains(sentinel),
            "{sentinel} reached the snapshot file"
        );
    }
    assert!(
        body.contains("wlan/basic-settings#WifiKey"),
        "the secret field is recorded by name"
    );
    assert!(
        body.contains("dhcp/settings#DhcpIPAddress"),
        "a setting is recorded"
    );
    let mode = std::fs::metadata(&files[0])
        .expect("mode")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "a snapshot is private");
    let dir_mode = std::fs::metadata(snap_dir(&home))
        .expect("dir mode")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "the snapshot directory is private");
    let salt = home.0.join("routwatch").join("config-salt");
    assert_eq!(
        std::fs::metadata(&salt).expect("salt").permissions().mode() & 0o777,
        0o600,
        "the salt is private too"
    );
}

#[test]
fn snapshotting_posts_nothing_beyond_login_and_logout() {
    let stub = Stub::start();
    let home = scratch("posts");
    let cfg = config(&home, &stub.addr);
    let out = snapshot_once(&home, &cfg);
    assert!(out.status.success(), "snapshot failed");
    assert_eq!(
        stub.posts(),
        vec!["/api/user/login", "/api/user/logout"],
        "a snapshot reads"
    );
}

#[test]
fn a_changed_setting_is_reported_once_the_two_reads_agree() {
    let stub = Stub::start();
    let home = scratch("changed");
    let cfg = config(&home, &stub.addr);
    assert!(snapshot_once(&home, &cfg).status.success(), "baseline");
    stub.set("/api/dhcp/settings", dhcp_doc("192.0.2.90"));
    let out = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "config", "diff"],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "drift exits 1: {text}");
    assert!(
        text.contains("dhcp/settings#DhcpIPAddress"),
        "the moved field is named: {text}"
    );
    assert!(text.contains("changed"), "{text}");
}

#[test]
fn a_reading_the_two_reads_disagree_on_is_left_out() {
    let stub = Stub::start();
    let home = scratch("unsettled");
    let cfg = config(&home, &stub.addr);
    assert!(snapshot_once(&home, &cfg).status.success(), "baseline");
    stub.alternate.store(true, Ordering::Relaxed);
    let out = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "config", "diff"],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "an unsettled read is not drift: {text}"
    );
    assert!(text.contains("unsettled"), "{text}");
    assert!(
        !text.contains("dhcp/settings#DhcpIPAddress"),
        "the field both reads disagreed on must not be reported: {text}"
    );
}

#[test]
fn an_endpoint_that_stops_answering_reads_as_a_visibility_change() {
    let stub = Stub::start();
    let home = scratch("visibility");
    let cfg = config(&home, &stub.addr);
    assert!(snapshot_once(&home, &cfg).status.success(), "baseline");
    stub.set("/api/security/mac-filter", REFUSED_DOC.to_string());
    let out = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "config", "diff"],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("visibility-changed"),
        "a refusal is not a removed field: {text}"
    );
    assert!(
        text.contains("security/mac-filter"),
        "the endpoint is named: {text}"
    );
    assert!(
        !text.contains("security/mac-filter#MacFilterMode"),
        "the field is not reported as removed: {text}"
    );
}

#[test]
fn a_firmware_change_suppresses_field_alarms() {
    let stub = Stub::start();
    let home = scratch("firmware");
    let cfg = config(&home, &stub.addr);
    assert!(snapshot_once(&home, &cfg).status.success(), "baseline");
    stub.set("/api/device/information", device_doc("11.0.2.22"));
    stub.set("/api/dhcp/settings", dhcp_doc("192.0.2.91"));
    let out = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "config", "diff"],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a build change is not drift: {text}"
    );
    assert!(text.contains("firmware changed"), "{text}");
}

#[test]
fn a_change_is_logged_by_field_name_and_never_by_value() {
    let stub = Stub::start();
    let home = scratch("eventlog");
    let cfg = config(&home, &stub.addr);
    assert!(snapshot_once(&home, &cfg).status.success(), "baseline");
    stub.set("/api/dhcp/settings", dhcp_doc("192.0.2.92"));
    assert!(
        snapshot_once(&home, &cfg).status.success(),
        "second snapshot"
    );
    let log = home.0.join("routwatch").join("events.jsonl");
    let body = std::fs::read_to_string(&log).expect("event log written");
    assert!(
        body.contains("dhcp/settings#DhcpIPAddress"),
        "the field name is in the log: {body}"
    );
    assert!(body.contains("\"kind\":\"config\""), "{body}");
    for sentinel in SENTINELS {
        assert!(!body.contains(sentinel), "{sentinel} reached the event log");
    }
    assert!(
        !body.contains("192.0.2.92"),
        "the new value must not be in the log"
    );
}

#[test]
fn the_snapshot_directory_keeps_the_newest() {
    let stub = Stub::start();
    let home = scratch("keep");
    let cfg = config(&home, &stub.addr);
    std::fs::create_dir_all(snap_dir(&home)).expect("dir");
    let keep = routwatch::config_snapshot::KEEP;
    for i in 0..keep + 2 {
        let path = snap_dir(&home).join(format!("config-{:06}.json", 100 + i));
        std::fs::write(&path, "{}").expect("old snapshot");
    }
    let out = snapshot_once(&home, &cfg);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let files = snapshots(&home);
    assert_eq!(files.len(), keep, "the cap keeps the newest snapshots only");
    assert!(
        !files.iter().any(|p| p.ends_with("config-000100.json")),
        "the oldest snapshot was pruned"
    );
}
