//! CLI-level regression tests.
//!
//! These exercise the real binary: exit codes, JSON shapes and the render path
//! that packaging depends on. Nothing here talks to the router: every test is
//! either a fixture render or a deliberate offline failure.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_routwatch"))
}

/// A config file that deletes its own directory when the test ends, panic or
/// not. Before this guard every run left a `routwatch-it-<pid>-*` tree in
/// `$TMPDIR`, one per test.
struct TempConfig(PathBuf);

impl std::ops::Deref for TempConfig {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for TempConfig {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempConfig {
    fn drop(&mut self) {
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A private config in its own temp dir. The password stays empty: every test
/// here must work without credentials (offline paths only).
fn config(name: &str, timeout_ms: u64) -> TempConfig {
    let dir = std::env::temp_dir().join(format!("routwatch-it-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            r#"[router]
ip = "192.168.8.1"
username = "admin"

[app]
refresh_ms = 2000
timeout_ms = {timeout_ms}
theme = "terminal"
view = "full"
graph_style = "dots"
budget_gb = 100
"#
        ),
    )
    .expect("write config");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    TempConfig(path)
}

/// One data home for every `run()` in this file, so no test writes into the
/// developer's real signal log. Created per run and removed when the run ends.
fn run(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let home = temp_data_home("runs");
    let mut cmd = bin();
    cmd.args(args);
    // A placeholder, never a real credential: the ladder only needs *a* value
    // so these tests do not depend on the machine's secret store.
    cmd.env("ROUTWATCH_PASSWORD", "test-placeholder");
    // Every CLI run that collects now writes a signal row. A test must not touch
    // the developer's real log, so it gets its own data dir.
    cmd.env("XDG_DATA_HOME", &*home);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn routwatch")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn version_and_shell_completion() {
    let out = run(&["--version"], &[]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("routwatch"), "{}", stdout(&out));

    let out = run(&["--completion", "fish"], &[]);
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("routwatch"),
        "completion must mention the binary"
    );

    let out = run(&["--completion", "tcsh"], &[]);
    assert!(
        !out.status.success(),
        "an unsupported shell must fail loudly"
    );
}

#[test]
fn preview_renders_a_fixture_without_a_router() {
    let cfg = config("preview", 250);
    let out = run(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "preview",
            "--tab",
            "bands",
            "--fixture",
            "tests/fixtures/snapshot_fixed.json",
            "--size",
            "120x28",
        ],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("SERVING CELL"), "{text}");
    assert!(text.contains("PRESETS"), "merge presets vanished\n{text}");
    assert!(
        text.contains("B1 + B3") || text.contains("B1+B3"),
        "the merge line vanished\n{text}"
    );
    assert!(text.contains("q quit"), "the status bar vanished\n{text}");
    assert!(
        !text.contains('\u{1b}'),
        "--ansi was not passed, so the frame must stay plain"
    );
}

#[test]
fn preview_ansi_paints_the_theme() {
    let cfg = config("ansi", 250);
    let args = [
        "--config",
        cfg.to_str().unwrap(),
        "--theme",
        "dark",
        "preview",
        "--tab",
        "dashboard",
        "--fixture",
        "tests/fixtures/snapshot_fixed.json",
        "--size",
        "80x20",
        "--ansi",
    ];
    let out = run(&args, &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("\u{1b}["), "expected SGR escapes\n{text}");
    // dark/accent = ANSI cyan on the active tab, so a 46 background is proof
    // the palette reached the frame instead of the default style.
    assert!(text.contains("46"), "no theme background in the frame");
}

#[test]
fn preview_needs_no_config_file_but_the_router_commands_still_do() {
    let absent = "/tmp/routwatch-absent-dir/config.toml";
    let out = run(
        &[
            "--config",
            absent,
            "preview",
            "--tab",
            "bands",
            "--fixture",
            "fixtures/sample-snapshot.json",
            "--size",
            "100x20",
        ],
        &[],
    );
    assert!(
        out.status.success(),
        "a layout review must work on a fresh machine: {}",
        stderr(&out)
    );
    assert!(stdout(&out).contains("PRESETS"));

    // The strict loader is only relaxed for the offline commands: a router
    // command with no config still exits 3.
    let out = run(&["--config", absent, "status", "--json"], &[]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "status must still demand a config"
    );
    assert!(stderr(&out).contains("No such file"), "{}", stderr(&out));
}

#[test]
fn invalid_theme_is_refused_with_the_family_message() {
    let cfg = config("theme", 250);
    let out = run(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "--theme",
            "pink",
            "doctor",
        ],
        &[],
    );
    assert_eq!(out.status.code(), Some(3), "config errors exit 3");
    let err = stderr(&out);
    assert!(
        err.contains("Invalid theme. Use: dark, terminal, auto, ocean"),
        "{err}"
    );

    // The aliases must be accepted, not treated as typos.
    let out = run(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "--theme",
            "system",
            "doctor",
        ],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
}

/// Config with a deliberately unknown theme name in it.
fn config_with_theme(name: &str, theme: &str) -> TempConfig {
    let dir = std::env::temp_dir().join(format!("routwatch-it-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            r#"[router]
ip = "192.168.8.1"
username = "admin"

[app]
refresh_ms = 2000
timeout_ms = 250
theme = "{theme}"
view = "full"
graph_style = "dots"
budget_gb = 100
"#
        ),
    )
    .expect("write config");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    TempConfig(path)
}

#[test]
fn doctor_flags_an_unknown_theme_instead_of_rendering_dark_silently() {
    let cfg = config_with_theme("badtheme", "neon");
    let out = run(
        &["--config", cfg.to_str().unwrap(), "doctor", "--json"],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let theme = doc["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "theme")
        .expect("a theme capability must be reported");
    assert_eq!(theme["state"], "degraded");
    assert_eq!(theme["reason"], "theme_unknown");
    assert!(
        theme["detail"].as_str().unwrap().contains("\"dark\""),
        "the detail must say which palette is actually used: {theme}"
    );

    // A built-in name is reported as ready.
    let cfg = config_with_theme("goodtheme", "nord");
    let out = run(
        &["--config", cfg.to_str().unwrap(), "doctor", "--json"],
        &[],
    );
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let theme = doc["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "theme")
        .unwrap()
        .clone();
    assert_eq!(theme["state"], "ready");
    assert_eq!(theme["reason"], "theme_valid");
}

#[test]
fn doctor_reports_the_capability_schema() {
    let cfg = config("doctor", 250);
    let out = run(
        &["--config", cfg.to_str().unwrap(), "doctor", "--json"],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["scope"], "static");
    let caps = doc["capabilities"].as_array().expect("capabilities array");
    assert!(!caps.is_empty());
    for cap in caps {
        for key in ["id", "state", "reason", "detail", "next_check"] {
            assert!(cap.get(key).is_some(), "capability missing {key}: {cap}");
        }
    }
    assert!(
        caps.iter()
            .any(|c| c["id"] == "config" && c["reason"] == "config_valid"),
        "config capability not reported: {caps:?}"
    );
    // Distribution support is a promise: doctor must say which optional
    // helpers this machine actually has (ping, a clipboard tool, keyring).
    let helpers = caps
        .iter()
        .find(|c| c["id"] == "helpers")
        .expect("helpers capability missing");
    let detail = helpers["detail"].as_str().unwrap();
    assert!(detail.contains("ping"), "helper detail: {detail}");
    assert!(
        matches!(helpers["state"].as_str(), Some("ready") | Some("degraded")),
        "unexpected helpers state: {helpers}"
    );
}

#[test]
fn unreachable_router_is_offline_and_exits_two() {
    let cfg = config("offline", 250);
    let out = run(
        &[
            "--config",
            cfg.to_str().unwrap(),
            "status",
            "--json",
            "--waybar",
        ],
        &[("ROUTWATCH_IP", "10.255.255.1")],
    );
    assert_eq!(out.status.code(), Some(2), "network failures exit 2");
    let text = stdout(&out);
    assert!(text.contains("\"class\":\"offline\""), "{text}");
}

/// A config directory that holds nothing, so the offline command reads the
/// defaults and never this machine's real file.
const NO_CONFIG: &str = "/nonexistent-routwatch-cli-test";

/// `privacy` talks to the router, so it takes the strict config loader: no
/// config file is a config error (exit 3, measured), not a network failure, and
/// nothing is printed half-built.
#[test]
fn privacy_without_a_router_fails_cleanly() {
    let out = run(
        &["privacy", "--json"],
        &[
            ("XDG_CONFIG_HOME", NO_CONFIG),
            ("ROUTWATCH_IP", "10.255.255.1"),
        ],
    );
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("config"),
        "the message must name the missing config: {}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

#[test]
fn speedtest_dry_run_names_the_plan_and_stops_before_sending() {
    let out = run(
        &["speedtest", "--dry-run", "--json"],
        &[("XDG_CONFIG_HOME", NO_CONFIG)],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(v["max_mb"], 60);
    assert_eq!(v["upload_mb"], 10);
    assert_eq!(v["download_only"], false);
}

#[test]
fn speedtest_without_the_data_flag_refuses_and_names_it() {
    let out = run(&["speedtest", "--json"], &[("XDG_CONFIG_HOME", NO_CONFIG)]);
    assert_eq!(out.status.code(), Some(3), "spending data needs consent");
    assert!(stderr(&out).contains("--allow-data"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "nothing was measured");
}

#[test]
fn speedtest_dry_run_honours_the_size_flags_and_clamps_them() {
    let out = run(
        &[
            "speedtest",
            "--dry-run",
            "--json",
            "--max-mb",
            "9999",
            "--upload-mb",
            "0",
            "--download-only",
        ],
        &[("XDG_CONFIG_HOME", NO_CONFIG)],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(v["max_mb"], 500);
    assert_eq!(v["upload_mb"], 1);
    assert_eq!(v["download_only"], true);
}

#[test]
fn a_world_readable_config_is_refused() {
    let cfg = config("perms", 250);
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let out = run(
        &["--config", cfg.to_str().unwrap(), "doctor", "--json"],
        &[],
    );
    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr(&out).contains("chmod 600"),
        "the message must say how to fix it: {}",
        stderr(&out)
    );
}

/// A throwaway XDG_DATA_HOME, removed on drop, so a test never writes into the
/// ledger of the person running it.
struct TempDataHome(PathBuf);

impl std::ops::Deref for TempDataHome {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDataHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_data_home(name: &str) -> TempDataHome {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("rw-xdg-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp data home");
    TempDataHome(d)
}

#[test]
fn history_lists_without_a_config_or_a_network() {
    let home = temp_data_home("hist");
    let out = bin()
        .env("XDG_DATA_HOME", &*home)
        .env("XDG_CONFIG_HOME", NO_CONFIG)
        .args(["speedtest", "--history"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("no saved speed runs"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_run_over_local_http_is_saved_and_no_save_keeps_the_file_absent() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            // The downloader wants its whole budget (5 MB even for --max-mb 1),
            // so answer with a body long enough to satisfy it, then let the
            // client close the socket.
            let chunk = vec![0u8; 64 * 1024];
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                12_000_000
            );
            if s.write_all(head.as_bytes()).is_err() {
                continue;
            }
            for _ in 0..183 {
                if s.write_all(&chunk).is_err() {
                    break;
                }
            }
        }
    });

    let home = temp_data_home("live");
    let url = format!("http://127.0.0.1:{port}/down?bytes=65536");
    let run = |extra: &[&str]| {
        let mut c = bin();
        c.env("XDG_DATA_HOME", &*home)
            .env("XDG_CONFIG_HOME", NO_CONFIG)
            .args([
                "speedtest",
                "--allow-data",
                "--download-only",
                "--seconds",
                "1",
                "--max-mb",
                "1",
                "--download-url",
                &url,
            ])
            .args(extra);
        c.output().expect("run")
    };

    let out = run(&["--no-save"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the local server must answer: {}",
        stderr(&out)
    );
    let file = home.join("routwatch/speed-history.jsonl");
    assert!(!file.exists(), "--no-save wrote {}", file.display());

    let out = run(&[]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(
        std::fs::read_to_string(&file)
            .expect("ledger")
            .lines()
            .count(),
        1,
        "one run is one row"
    );
    server.join().ok();
}

#[test]
fn twenty_sequential_runs_never_exhaust_the_sessions() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// The firmware keeps one session per login: while one is open a second
    /// login is refused with 108003 (H122-373, WEBUI 11.0.2.21, measured
    /// 2026-11-22). This stand-in does the same, so a command that forgets to
    /// log out fails the *next* run instead of quietly leaking.
    fn session_fake_router() -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let open = Arc::new(AtomicBool::new(false));
        let logins = Arc::new(AtomicUsize::new(0));
        let logouts = Arc::new(AtomicUsize::new(0));
        let (o, li, lo) = (Arc::clone(&open), Arc::clone(&logins), Arc::clone(&logouts));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/")
                    .to_string();
                let body = match path.as_str() {
                    "/" => "<html><input name=\"csrf_token\" value=\"t\" /></html>".to_string(),
                    "/api/user/state-login" => "<response><State>0</State></response>".to_string(),
                    "/api/webserver/SesTokInfo" => {
                        "<response><SesInfo>SessionID=s</SesInfo><TokInfo>t</TokInfo></response>"
                            .to_string()
                    }
                    "/api/user/login" => {
                        if o.swap(true, Ordering::SeqCst) {
                            "<error><code>108003</code></error>".to_string()
                        } else {
                            li.fetch_add(1, Ordering::SeqCst);
                            "<response>OK</response>".to_string()
                        }
                    }
                    "/api/user/logout" => {
                        o.store(false, Ordering::SeqCst);
                        lo.fetch_add(1, Ordering::SeqCst);
                        "<response>OK</response>".to_string()
                    }
                    "/api/device/signal" => concat!(
                        "<response><band>3</band><pci>481</pci><rsrp>-95dBm</rsrp>",
                        "<rsrq>-7dB</rsrq><sinr>5dB</sinr></response>"
                    )
                    .to_string(),
                    "/api/monitoring/status" => concat!(
                        "<response><ConnectionStatus>901</ConnectionStatus>",
                        "<CurrentNetworkType>19</CurrentNetworkType>",
                        "<SignalIcon>5</SignalIcon></response>"
                    )
                    .to_string(),
                    _ => "<response></response>".to_string(),
                };
                let head = format!(
                    "HTTP/1.1 200 Ok\r\nContent-Type: text/xml\r\nSet-Cookie: ***\r\n__Response_Code: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(body.as_bytes());
            }
        });
        (format!("127.0.0.1:{port}"), logins, logouts)
    }

    let (ip, logins, logouts) = session_fake_router();
    let cfg = config_for(&ip);
    for n in 1..=20 {
        let mut cmd = bin();
        cmd.env("ROUTWATCH_PASSWORD", "test-placeholder")
            .env("XDG_DATA_HOME", temp_data_home("sessions").to_path_buf())
            .args(["--config", cfg.to_str().unwrap(), "status", "--json"]);
        let out = cmd.output().expect("run status");
        assert_eq!(
            out.status.code(),
            Some(0),
            "run {n} failed, a session from an earlier run was still open: {}",
            stderr(&out)
        );
    }
    assert_eq!(logins.load(Ordering::SeqCst), 20, "every run logs in once");
    assert_eq!(
        logouts.load(Ordering::SeqCst),
        20,
        "every run must close its session, not only the first"
    );
}

#[test]
fn scan_without_allow_write_names_the_flag() {
    let out = run(&["scan", "--set", "1,3", "--allow-data"], &[]);
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("--allow-write"),
        "the refusal must name the flag: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("--dry-run"), "{}", stderr(&out));
}

#[test]
fn scan_dry_run_reports_sets_and_time_without_a_router() {
    let out = run(
        &["scan", "--set", "1,3", "--set", "20", "--dry-run"],
        &[("XDG_CONFIG_HOME", NO_CONFIG)],
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("2 sets"), "{}", s);
    assert!(s.contains("upper bound"), "{}", s);
    // The estimate is derived, not written down: the same three ceilings the
    // run prints, with the measurement ceiling coming from the module that
    // spends it. A change to a phase of the speed test moves this number.
    let plan = routwatch::config::Speedtest::default().sanitised();
    let per_set = 60 + 5 + routwatch::speedtest::worst_case_secs(&plan);
    assert!(
        s.contains(&format!("{} s", per_set * 2)),
        "the two set estimate must be {} s: {}",
        per_set * 2,
        s
    );
    assert!(
        s.contains(&format!("{} MB", plan.max_mb * 2)),
        "the data estimate must follow the plan: {}",
        s
    );
    assert!(s.contains("nothing was sent"), "{}", s);
}

/// A router stand-in for the collect path: the login handshake, the documents a
/// snapshot reads, and logout. Each document carries one real figure so a signal
/// row exists, and nothing else is invented.
fn fake_router() -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { return };
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .split('?')
                .next()
                .unwrap_or("/")
                .to_string();
            let body = match path.as_str() {
                "/" => "<html><input name=\"csrf_token\" value=\"t\" /></html>".to_string(),
                "/api/user/state-login" => "<response><State>0</State></response>".to_string(),
                "/api/webserver/SesTokInfo" => {
                    "<response><SesInfo>SessionID=s</SesInfo><TokInfo>t</TokInfo></response>"
                        .to_string()
                }
                "/api/user/login" => "<response>OK</response>".to_string(),
                "/api/user/logout" => "<response>OK</response>".to_string(),
                "/api/device/signal" => concat!(
                    "<response><band>3</band><pci>481</pci><rsrp>-95dBm</rsrp>",
                    "<rsrq>-7dB</rsrq><sinr>5dB</sinr></response>"
                )
                .to_string(),
                "/api/monitoring/status" => concat!(
                    "<response><ConnectionStatus>901</ConnectionStatus>",
                    "<CurrentNetworkType>19</CurrentNetworkType>",
                    "<SignalIcon>5</SignalIcon></response>"
                )
                .to_string(),
                "/api/net/net-mode-list" => "<response></response>".to_string(),
                _ => "<response></response>".to_string(),
            };
            let head = format!(
                "HTTP/1.1 200 Ok\r\nContent-Type: text/xml\r\nSet-Cookie: SessionID=s; Path=/\r\n__Response_Code: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body.as_bytes());
        }
    });
    (format!("127.0.0.1:{port}"), server)
}

fn config_for(ip: &str) -> TempConfig {
    // Its own directory per call: `TempConfig` removes the parent on drop, and two
    // tests in one process must not share one.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("routwatch-it-ip-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        concat!(
            "[router]\nip = \"{ip}\"\nusername = \"admin\"\n\n[app]\n",
            "refresh_ms = 2000\ntimeout_ms = 4000\ntheme = \"terminal\"\n",
            "view = \"full\"\ngraph_style = \"dots\"\nbudget_gb = 100\n"
        )
        .replace("{ip}", ip),
    )
    .expect("write config");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    TempConfig(path)
}

fn signal_log(home: &std::path::Path) -> PathBuf {
    home.join("routwatch/signal-history.jsonl")
}

#[test]
fn collect_writes_one_signal_row_a_minute_into_its_own_data_home() {
    let (ip, server) = fake_router();
    let cfg = config_for(&ip);
    let home = temp_data_home("sigrow");
    let run_status = |extra: &[(&str, &str)], home: &TempDataHome| {
        let mut cmd = bin();
        cmd.env("XDG_DATA_HOME", &**home)
            .env("ROUTWATCH_PASSWORD", "test-placeholder")
            .args(["--config", cfg.to_str().unwrap(), "status", "--json"]);
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd.output().expect("run status")
    };

    let out = run_status(&[], &home);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let log = signal_log(&home);
    let first = std::fs::read_to_string(&log).expect("one row");
    assert_eq!(first.lines().count(), 1, "{first}");
    assert!(first.contains("\"rsrp_dbm\":-95.0"), "{first}");

    // A second run inside the same minute must not add a row: the timestamp comes
    // from the file, not from this process's memory.
    let out = run_status(&[], &home);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        1,
        "two runs in one minute are one sample"
    );

    // The switch a script that reads every second needs.
    let quiet = temp_data_home("sigrow-off");
    let out = run_status(&[("ROUTWATCH_DISABLE_SIGNAL_LOG", "1")], &quiet);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        !signal_log(&quiet).exists(),
        "the switch must leave no file"
    );
    drop(server);
}

#[test]
fn history_signal_reads_the_log_without_a_config_or_a_network() {
    let home = temp_data_home("siglist");
    let log = signal_log(&home);
    std::fs::create_dir_all(log.parent().unwrap()).expect("data dir");
    let row = |at: u64, rsrp: f32| {
        format!(
            "{{\"v\":1,\"at_unix\":{at},\"net_type\":\"LTE\",\"band\":\"B3\",\"nr_band\":null,\
             \"rsrp_dbm\":{rsrp},\"rsrq_db\":-7.0,\"sinr_db\":5.0,\"pci\":481,\"endc\":false}}\n"
        )
    };
    std::fs::write(
        &log,
        row(1_700_000_000, -95.0) + &row(1_700_000_060, -101.0),
    )
    .expect("log");
    let out = bin()
        .env("XDG_DATA_HOME", &*home)
        .env("XDG_CONFIG_HOME", NO_CONFIG)
        .args(["history", "--signal"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.contains("B3"), "{text}");
    assert!(text.contains("-101.0"), "{text}");
    // The newest row is last, and --limit takes the tail, not the head.
    let out = bin()
        .env("XDG_DATA_HOME", &*home)
        .env("XDG_CONFIG_HOME", NO_CONFIG)
        .args(["history", "--signal", "--limit", "1"])
        .output()
        .expect("run");
    let one = stdout(&out);
    assert_eq!(one.lines().count(), 1, "{one}");
    assert!(one.contains("-101.0"), "{one}");
}

#[test]
fn history_without_signal_says_why_and_exits_two() {
    let home = temp_data_home("sighint");
    let out = bin()
        .env("XDG_DATA_HOME", &*home)
        .env("XDG_CONFIG_HOME", NO_CONFIG)
        .args(["history"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("--signal"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

#[test]
fn ip_renew_without_allow_write_names_the_flag_and_exits_three() {
    let out = bin()
        .env("XDG_CONFIG_HOME", NO_CONFIG)
        .args(["ip", "renew"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("--allow-write"), "{}", stderr(&out));
}

#[test]
fn bands_watch_without_set_is_refused_before_any_connection() {
    // A valid config, pointed at a port nothing listens on: if the guard did not
    // fire first, this would fail as a network error instead of naming the flag.
    let cfg = config_for("127.0.0.1");
    let out = bin()
        .args(["--config", cfg.to_str().unwrap(), "bands", "--watch", "180"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("--watch needs --set"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

/// A data dir of its own: incidents read files, so the test writes them. The
/// config home is redirected too, so no run reads the developer's real config.
/// Removed when the test ends, so a run leaves no `rw-*` behind.
struct LogHome(std::path::PathBuf);

impl std::ops::Deref for LogHome {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for LogHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn log_home(name: &str) -> LogHome {
    let d = std::env::temp_dir().join(format!("rw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("routwatch")).expect("temp data home");
    // A run that finds a config home but no file fails before reaching the logs,
    // so the base config lives here too. No credential: nothing in these tests
    // is allowed to reach a router.
    let cfg = d.join("routwatch").join("config.toml");
    std::fs::write(
        &cfg,
        "[router]\nip = \"192.168.8.1\"\nusername = \"admin\"\n",
    )
    .expect("config");
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    LogHome(d)
}

fn write_events(home: &std::path::Path) {
    let body = concat!(
        r#"{"v":1,"at_unix":1000,"tier":"signal","kind":"read","outcome":"failed"}"#,
        "\n",
        r#"{"v":1,"at_unix":1060,"tier":"signal","kind":"read","outcome":"failed"}"#,
        "\n",
        r#"{"v":1,"at_unix":1200,"tier":"wan","kind":"wan","outcome":"up"}"#,
        "\n"
    );
    std::fs::write(home.join("routwatch").join("events.jsonl"), body).expect("events");
}

#[test]
fn incidents_come_from_the_logs_and_never_from_a_router() {
    let home = log_home("incidents");
    write_events(&home);
    let h = home.to_str().unwrap();
    let out = run(
        &["incidents", "list"],
        &[("XDG_DATA_HOME", h), ("XDG_CONFIG_HOME", h)],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("read-1000"), "{text}");
    assert!(
        text.contains("2 rows"),
        "the recovery row is not a row: {text}"
    );
}

#[test]
fn a_report_names_an_incident_and_can_hide_its_detail() {
    let home = log_home("report");
    write_events(&home);
    let h = home.to_str().unwrap();
    let envs = [("XDG_DATA_HOME", h), ("XDG_CONFIG_HOME", h)];
    let out = run(&["report", "--incident", "read-1000"], &envs);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("# routwatch incident"), "{text}");
    assert!(text.contains("signal read: failed"), "{text}");
    assert!(text.contains("No cause is named"), "{text}");

    let out = run(&["report", "--incident", "read-1000", "--redact"], &envs);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("(hidden)"), "{text}");
    assert!(
        !text.contains("signal read: failed"),
        "a redacted report must not carry the detail line: {text}"
    );

    let out = run(&["incidents", "show", "wan-9999"], &envs);
    assert!(!out.status.success(), "an unknown id must fail");
    assert!(
        stderr(&out).contains("no incident with id"),
        "{}",
        stderr(&out)
    );
}
