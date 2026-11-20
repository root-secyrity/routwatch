//! The capability matrix, measured against a fake firmware on a real socket.
//!
//! Every claim here is counted: the stub records each request with its method,
//! so "the probe posts nothing beyond login and logout" is a list this file
//! measured, not a reading of the source.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const SES_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><SesInfo>SessionID=stub-session</SesInfo><TokInfo>stub-token</TokInfo></response>";
const OK_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>OK</response>";
/// A value that must never reach a file. Only the firmware string may be kept.
const DEVICE_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><SoftwareVersion>11.0.2.21</SoftwareVersion><SerialNumber>SENTINEL-SERIAL-42</SerialNumber><Imei>SENTINEL-IMEI-42</Imei></response>";
const SIGNAL_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><rsrp>-95dBm</rsrp><band>3</band></response>";
const REFUSED_DOC: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100003</code><message>no right</message></error>";

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
                    "/api/device/information" => (200, DEVICE_DOC.to_string()),
                    "/api/device/signal" => (200, SIGNAL_DOC.to_string()),
                    "/api/security/mac-filter" => (200, REFUSED_DOC.to_string()),
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

    /// POSTs this run made, in order.
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

/// A private config pointing at the stub, removed with its directory.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> TempDir {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rw-captest-{tag}-{}-{n}", std::process::id()));
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

fn matrix_path(home: &TempDir) -> PathBuf {
    home.0.join("routwatch").join("capabilities.json")
}

// -------------------------------------------------------------------- tests

#[test]
fn a_run_without_a_probe_says_there_is_no_matrix() {
    let home = scratch("empty");
    let cfg = config(&home, "127.0.0.1:1");
    let out = run(&home, &["--config", cfg.to_str().unwrap(), "capabilities"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("no matrix yet"), "{text}");
    assert!(
        !matrix_path(&home).exists(),
        "reading an unmeasured matrix must not create one"
    );
}

#[test]
fn the_probe_writes_a_private_matrix_keyed_by_firmware() {
    let stub = Stub::start();
    let home = scratch("probe");
    let cfg = config(&home, &stub.addr);
    let out = run(
        &home,
        &[
            "--config",
            cfg.to_str().unwrap(),
            "capabilities",
            "--probe",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = matrix_path(&home);
    let mode = std::fs::metadata(&path)
        .expect("matrix written")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the matrix is private");

    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("json");
    assert_eq!(doc["firmware"], "11.0.2.21", "{doc}");
    assert_eq!(doc["entries"]["device/signal"]["state"], "answered");
    assert_eq!(doc["entries"]["security/mac-filter"]["state"], "refused");
    assert_eq!(
        doc["entries"]["security/mac-filter"]["code"], 100003,
        "a refusal keeps the code the firmware gave"
    );
    assert_eq!(doc["entries"]["wlan/host-list"]["state"], "not-served");
    assert_eq!(doc["entries"]["wlan/host-list"]["status"], 404);

    // The second run reads the file, not the router.
    let again = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "capabilities", "--json"],
    );
    assert!(again.status.success());
    let same: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&again.stdout)).expect("json");
    assert_eq!(same["firmware"], "11.0.2.21");
    assert_eq!(same["entries"]["device/signal"]["state"], "answered");
}

#[test]
fn the_matrix_keeps_no_setting_value() {
    let stub = Stub::start();
    let home = scratch("secrets");
    let cfg = config(&home, &stub.addr);
    let out = run(
        &home,
        &[
            "--config",
            cfg.to_str().unwrap(),
            "capabilities",
            "--probe",
            "--json",
        ],
    );
    assert!(out.status.success());
    let written = std::fs::read_to_string(matrix_path(&home)).expect("matrix");
    for sentinel in ["SENTINEL-SERIAL-42", "SENTINEL-IMEI-42", "-95dBm"] {
        assert!(
            !written.contains(sentinel),
            "{sentinel} reached the matrix file"
        );
    }
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(!printed.contains("SENTINEL-"), "--json leaked a value");
}

#[test]
fn the_probe_posts_nothing_beyond_login_and_logout() {
    let stub = Stub::start();
    let home = scratch("posts");
    let cfg = config(&home, &stub.addr);
    let out = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "capabilities", "--probe"],
    );
    assert!(out.status.success());
    let posts = stub.posts();
    assert_eq!(
        posts,
        vec![
            "/api/user/login".to_string(),
            "/api/user/logout".to_string()
        ],
        "the probe is a read: only the session itself may post"
    );
}

#[test]
fn a_consumer_repeats_what_the_matrix_recorded_without_blocking() {
    let stub = Stub::start();
    let home = scratch("consumer");
    let dir = home.0.join("routwatch");
    std::fs::create_dir_all(&dir).expect("data dir");
    std::fs::write(
        dir.join("capabilities.json"),
        r#"{"v":1,"firmware":"11.0.2.21","measured_at":7,"entries":{"security/firewall-switch":{"state":"refused","code":100003}}}"#,
    )
    .expect("matrix");
    let cfg = config(&home, &stub.addr);
    let out = run(
        &home,
        &["--config", cfg.to_str().unwrap(), "privacy", "--json"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("json");
    let firewall = doc["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|c| c["name"] == "firewall")
        .expect("firewall check");
    let detail = firewall["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("this firmware refused the read with 100003"),
        "the matrix note is missing: {detail}"
    );
    // The outcome is unchanged: the matrix explains, it does not decide.
    assert_eq!(
        firewall["state"], "notexposed",
        "the matrix must not change what the check concluded"
    );
}
