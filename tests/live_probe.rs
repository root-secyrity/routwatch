//! Live measurements for the feasibility round. Ignored by default: these read
//! the owner's real CPE, so they run only on purpose and only when allowed.
//!
//! ```sh
//! ROUTWATCH_LIVE=1 cargo test --locked --test live_probe -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Budget, declared before the first request (reads only, one session, serial):
//!   per_endpoint_latency      32 endpoints x 30 samples = 960 reads
//!   the_fast_tier_fits        3 endpoints x 100 samples  = 300 reads
//!   a_sustained_cadence       500 ms x 30 s x 3 reads     = 180 reads
//!   one_session_survives      500 ms x 30 s x 3 reads     = 180 reads
//!   the_push_channel          one read per candidate      = 8 reads
//!   the_observed_cadence      six samples at 1 s          = 6 collects
//! Total under 1,700 reads, one login each, no writes beyond login and logout.
//!
//! Measurements land in `DOCK/live/` (untracked) and carry timings and firmware
//! codes only: no address, SSID, MAC, IMEI or client name is written anywhere.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use routwatch::config::{self, Config};
use routwatch::error::ApiError;
use routwatch::router::endpoints::PROBE;
use routwatch::router::HuaweiClient;
use routwatch::worker::{self, Cmd, Ev};

/// One session at a time: two tests reading the router at once would measure
/// each other, and this firmware serialises requests anyway.
/// The read that proves a session is still alive when another endpoint refuses.
const LIVENESS: &str = "device/information";

static LIVE_LOCK: Mutex<()> = Mutex::new(());

const FAST_TIER: [&str; 3] = [
    "device/signal",
    "monitoring/status",
    "monitoring/traffic-statistics",
];

// ------------------------------------------------------------------ harness

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("DOCK/live");
    std::fs::create_dir_all(&dir).expect("DOCK/live");
    dir
}

fn write_report(name: &str, body: &str) {
    let path = out_dir().join(name);
    std::fs::File::create(&path)
        .and_then(|mut f| f.write_all(body.as_bytes()))
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// `None` means "this run is not allowed to touch the device", with the reason
/// printed once. No test fails because the round was not armed.
fn gate(test: &str) -> Option<(Config, String)> {
    if std::env::var("ROUTWATCH_LIVE").ok().as_deref() != Some("1") {
        eprintln!("skip {test}: set ROUTWATCH_LIVE=1 to read the device");
        return None;
    }
    let cfg = match config::load_or_default(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skip {test}: no usable config ({e})");
            return None;
        }
    };
    match config::resolve_password(&cfg.router.password, None) {
        Ok((pass, _)) => Some((cfg, pass)),
        Err(e) => {
            eprintln!("skip {test}: no usable credential ({e})");
            None
        }
    }
}

fn connect(cfg: &Config, pass: &str) -> HuaweiClient {
    let mut client = HuaweiClient::new(&cfg.router.ip, cfg.app.timeout_ms);
    client
        .login(&cfg.router.username, pass)
        .expect("login for a live measurement");
    client
}

/// The codes that mean "the router is being asked too often": the round treats
/// any of them as a stop signal, not as a data point.
fn busy_reason(e: &ApiError) -> Option<String> {
    match e {
        ApiError::Busy(c) => Some(format!("busy {c}")),
        ApiError::Invalid(c) => Some(format!("invalid/session {c}")),
        ApiError::Http(s) if *s == 429 || *s >= 500 => Some(format!("http {s}")),
        ApiError::Auth(m) => Some(format!("auth {m}")),
        _ => None,
    }
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn to_ms(v: u128) -> f64 {
    v as f64 / 1000.0
}

/// Time `samples` reads of one endpoint. Returns the sorted per-call times and
/// stops the whole run on the first busy/refused/auth answer: a measurement
/// taken while the router is refusing is not a measurement of latency.
fn time_endpoint(
    client: &mut HuaweiClient,
    path: &str,
    samples: usize,
) -> Result<Vec<u128>, String> {
    let mut times = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let result = client.get(path);
        times.push(started.elapsed().as_micros());
        if let Err(e) = result {
            if let Some(why) = busy_reason(&e) {
                // A refusal on one endpoint and a lost session look identical in the
                // answer alone. One known-good read separates them.
                let alive = client.get(LIVENESS).is_ok();
                if alive {
                    return Err(format!("{why} (code {e})"));
                }
                panic!("stop: {path} answered {why} (code {e}) and {LIVENESS} failed too");
            }
        }
    }
    times.sort_unstable();
    Ok(times)
}

/// The three fast reads at a fixed cadence. Returns (reads, busy codes, re-auth
/// signals, gap between iterations).
fn soak(
    cfg: &Config,
    pass: &str,
    cadence: Duration,
    duration: Duration,
) -> (usize, Vec<String>, usize, Vec<Duration>) {
    let mut client = connect(cfg, pass);
    let mut reads = 0usize;
    let mut bad = Vec::new();
    let mut relogins = 0usize;
    let mut gaps = Vec::new();
    let started = Instant::now();
    let mut last = Instant::now();
    while started.elapsed() < duration {
        let iter_start = Instant::now();
        for path in FAST_TIER {
            let result = client.get(path);
            reads += 1;
            if let Err(e) = result {
                if let Some(why) = busy_reason(&e) {
                    bad.push(format!("{path}: {why}"));
                }
            }
        }
        if !client.is_authed() {
            relogins += 1;
            let _ = client.login(&cfg.router.username, pass);
        }
        let now = Instant::now();
        gaps.push(now.duration_since(last));
        last = now;
        std::thread::sleep(cadence.saturating_sub(iter_start.elapsed()));
    }
    client.logout();
    (reads, bad, relogins, gaps)
}

// --------------------------------------------------------------- the tests

#[test]
#[ignore = "reads the real device; run with ROUTWATCH_LIVE=1"]
fn per_endpoint_latency_is_measured() {
    let Some((cfg, pass)) = gate("per_endpoint_latency_is_measured") else {
        return;
    };
    let _only_one = LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    const SAMPLES: usize = 30;
    eprintln!(
        "budget: {} endpoints x {SAMPLES} reads = {} reads, one session",
        PROBE.len(),
        PROBE.len() * SAMPLES
    );
    let mut client = connect(&cfg, &pass);

    let mut csv = String::from("group,endpoint,n,p50_ms,p95_ms,max_ms,status\n");
    let mut total = 0usize;
    for spec in PROBE {
        match time_endpoint(&mut client, spec.path, SAMPLES) {
            Ok(times) => {
                total += times.len();
                csv.push_str(&format!(
                    "{},{},{},{:.1},{:.1},{:.1},ok\n",
                    spec.group,
                    spec.path,
                    times.len(),
                    to_ms(percentile(&times, 0.50)),
                    to_ms(percentile(&times, 0.95)),
                    to_ms(times[times.len() - 1]),
                ));
                assert_eq!(
                    times.len(),
                    SAMPLES,
                    "{} must be timed {SAMPLES} times",
                    spec.path
                );
            }
            Err(refusal) => {
                eprintln!("{} refuses GET: {refusal} (session still alive)", spec.path);
                csv.push_str(&format!(
                    "{},{},0,,,,refused: {refusal}\n",
                    spec.group, spec.path
                ));
            }
        }
    }
    client.logout();
    write_report("latency.csv", &csv);
    eprintln!("per_endpoint_latency: {total} reads written to DOCK/live/latency.csv");
}

#[test]
#[ignore = "reads the real device; run with ROUTWATCH_LIVE=1"]
fn the_fast_tier_fits_inside_the_budget() {
    let Some((cfg, pass)) = gate("the_fast_tier_fits_inside_the_budget") else {
        return;
    };
    let _only_one = LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let budget_ms: f64 = std::env::var("ROUTWATCH_FAST_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(700.0);
    const SAMPLES: usize = 100;
    eprintln!("budget: {} reads, target p95 <= {budget_ms} ms", SAMPLES);

    let mut client = connect(&cfg, &pass);
    let mut times: Vec<u128> = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for path in FAST_TIER {
            match client.get(path) {
                Ok(_) => {}
                Err(e) => {
                    if let Some(why) = busy_reason(&e) {
                        panic!("stop: {path} answered {why} (code {e})");
                    }
                }
            }
        }
        times.push(started.elapsed().as_micros());
    }
    client.logout();
    times.sort_unstable();
    let p50 = percentile(&times, 0.50);
    let p95 = percentile(&times, 0.95);
    let max = times[times.len() - 1];
    let body = format!(
        "fast tier ({}), n={SAMPLES}, budget p95 {budget_ms} ms\np50 {:.1} ms\np95 {:.1} ms\nmax {:.1} ms\n",
        FAST_TIER.join(" + "),
        to_ms(p50),
        to_ms(p95),
        to_ms(max)
    );
    write_report("fast-tier.txt", &body);
    eprintln!("{body}");
    assert!(
        to_ms(p95) <= budget_ms,
        "p95 {:.1} ms is over the {budget_ms} ms budget",
        to_ms(p95)
    );
}

#[test]
#[ignore = "reads the real device; run with ROUTWATCH_LIVE=1"]
fn a_sustained_cadence_produces_no_busy_code() {
    let Some((cfg, pass)) = gate("a_sustained_cadence_produces_no_busy_code") else {
        return;
    };
    let _only_one = LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let secs: u64 = std::env::var("ROUTWATCH_SOAK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let cadence = Duration::from_millis(500);
    eprintln!(
        "budget: {} s at 500 ms x {} reads = about {} reads",
        secs,
        FAST_TIER.len(),
        secs * 2 * FAST_TIER.len() as u64
    );
    let (reads, bad, relogins, _gaps) = soak(&cfg, &pass, cadence, Duration::from_secs(secs));
    let body = format!(
        "soak {secs}s at 500 ms: {reads} reads, {} busy codes, {relogins} re-auths\n{}\n",
        bad.len(),
        if bad.is_empty() {
            "no code recorded".to_string()
        } else {
            bad.join("\n")
        }
    );
    write_report("soak.txt", &body);
    eprintln!("{body}");
    assert!(bad.is_empty(), "the cadence was refused: {bad:?}");
}

#[test]
#[ignore = "reads the real device; run with ROUTWATCH_LIVE=1"]
fn one_session_survives_the_whole_soak() {
    let Some((cfg, pass)) = gate("one_session_survives_the_whole_soak") else {
        return;
    };
    let _only_one = LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let secs: u64 = std::env::var("ROUTWATCH_SOAK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let (reads, bad, relogins, gaps) = soak(
        &cfg,
        &pass,
        Duration::from_millis(500),
        Duration::from_secs(secs),
    );
    let mut gap_ms: Vec<u128> = gaps.iter().map(|g| g.as_millis()).collect();
    gap_ms.sort_unstable();
    let med = if gap_ms.is_empty() {
        0
    } else {
        gap_ms[gap_ms.len() / 2]
    };
    let worst = gap_ms.last().copied().unwrap_or(0);
    let body = format!(
        "one session, {reads} reads: {relogins} re-auths, {} refusals\ngap median {med} ms, worst {worst} ms\n",
        bad.len()
    );
    write_report("session.txt", &body);
    eprintln!("{body}");
    assert_eq!(relogins, 0, "the session did not survive: {bad:?}");
}

#[test]
#[ignore = "reads the real device; run with ROUTWATCH_LIVE=1"]
fn the_signal_read_shape_is_measured() {
    let Some((cfg, pass)) = gate("the_signal_read_shape_is_measured") else {
        return;
    };
    let _only_one = LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    const SAMPLES: usize = 60;
    eprintln!("budget: {SAMPLES} reads of device/signal, one session");
    let mut client = connect(&cfg, &pass);
    let mut raw: Vec<u128> = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        if let Err(e) = client.get("device/signal") {
            if let Some(why) = busy_reason(&e) {
                panic!("stop: device/signal answered {why} (code {e})");
            }
        }
        raw.push(started.elapsed().as_micros());
    }
    client.logout();
    let mut csv = String::from("sample,ms\n");
    for (i, v) in raw.iter().enumerate() {
        csv.push_str(&format!("{i},{:.1}\n", to_ms(*v)));
    }
    let slow = raw.iter().filter(|v| **v > 800_000).count();
    let mut sorted = raw.clone();
    sorted.sort_unstable();
    let line = format!(
        "device/signal n={SAMPLES}: p10 {:.0} ms, p50 {:.0} ms, p90 {:.0} ms, p95 {:.0} ms, max {:.0} ms, slow(>800ms) {slow}/{SAMPLES}\n",
        to_ms(percentile(&sorted, 0.10)),
        to_ms(percentile(&sorted, 0.50)),
        to_ms(percentile(&sorted, 0.90)),
        to_ms(percentile(&sorted, 0.95)),
        to_ms(sorted[sorted.len() - 1]),
    );
    eprint!("{line}");
    write_report("signal-shape.csv", &csv);
    write_report("signal-shape.txt", &line);
}

#[test]
#[ignore = "reads the real device; run with ROUTWATCH_LIVE=1"]
fn the_push_channel_is_absent_or_named() {
    let Some((cfg, pass)) = gate("the_push_channel_is_absent_or_named") else {
        return;
    };
    let _only_one = LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Every candidate the web UI or a Huawei firmware could plausibly serve.
    // Each one is asked once; the answer is recorded whether it exists or not.
    const CANDIDATES: [&str; 8] = [
        "websocket",
        "ws",
        "events",
        "webserver/events",
        "notification",
        "sse",
        "event-stream",
        "user/state-login",
    ];
    let mut client = connect(&cfg, &pass);
    let mut lines = String::new();
    for path in CANDIDATES {
        let answer = match client.get(path) {
            Ok(node) => format!("served, root <{}>", node.name),
            Err(e) => format!("{e}"),
        };
        lines.push_str(&format!("{path}: {answer}\n"));
    }
    client.logout();
    write_report("push-channel.txt", &lines);
    eprintln!("{lines}");
    assert_eq!(
        lines.lines().count(),
        CANDIDATES.len(),
        "every candidate gets one recorded answer; absence is announced, not assumed"
    );
}

#[test]
#[ignore = "reads the real device; run with ROUTWATCH_LIVE=1"]
fn the_observed_cadence_is_measured_end_to_end() {
    let Some((mut cfg, pass)) = gate("the_observed_cadence_is_measured_end_to_end") else {
        return;
    };
    let _only_one = LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    const SAMPLES: usize = 6;
    cfg.app.refresh_ms = 1_000;
    eprintln!("budget: {SAMPLES} collects at a 1000 ms interval");

    let (tx, rx) = worker::spawn(cfg.clone(), pass);
    let mut arrivals: Vec<(Instant, u128)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    while arrivals.len() < SAMPLES && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ev::Snapshot { poll_ms, .. }) => {
                arrivals.push((Instant::now(), poll_ms));
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
    let _ = tx.send(Cmd::Stop(ack_tx));
    let _ = ack_rx.recv_timeout(Duration::from_secs(5));

    assert!(
        arrivals.len() >= SAMPLES,
        "only {} snapshots arrived, cannot measure a cadence",
        arrivals.len()
    );
    let mut gaps: Vec<u128> = arrivals
        .windows(2)
        .map(|w| w[1].0.duration_since(w[0].0).as_millis())
        .collect();
    gaps.sort_unstable();
    let polls: Vec<u128> = arrivals.iter().map(|(_, p)| *p).collect();
    let med = gaps[gaps.len() / 2];
    let worst = gaps[gaps.len() - 1];
    let body = format!(
        "refresh 1000 ms, {SAMPLES} snapshots\ngap median {med} ms, worst {worst} ms\npoll_ms {:?}\ninterval + poll = {}\n",
        polls,
        1_000 + polls[polls.len() / 2] as u64
    );
    write_report("cadence.txt", &body);
    eprintln!("{body}");
    assert!(med > 0, "a zero gap means the samples were not spaced");
}

/// L0 of the tiered round: `collect_traced` reads `dhcp/feature-switch` through
/// `HuaweiClient::dhcp_dns_editable`, and that path is not in `PROBE`, so the
/// feasibility round never measured it. A tier cannot hold an unmeasured
/// endpoint, so this measures it before any tier is written.
///
/// Budget, declared before the first request: 30 reads of one endpoint, one
/// session, serial, read only.
#[test]
#[ignore = "reads the real device; run with ROUTWATCH_LIVE=1"]
fn the_dhcp_feature_switch_read_is_measured() {
    let Some((cfg, pass)) = gate("the_dhcp_feature_switch_read_is_measured") else {
        return;
    };
    let _only_one = LIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    const SAMPLES: usize = 30;
    const PATH: &str = "dhcp/feature-switch";
    eprintln!("budget: {SAMPLES} reads of {PATH}, one session");

    let mut client = connect(&cfg, &pass);
    let mut ms: Vec<f64> = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let t0 = Instant::now();
        let ok = client.get(PATH).is_ok();
        let took = t0.elapsed().as_secs_f64() * 1_000.0;
        assert!(ok, "sample {i} of {PATH} failed, aborting the run");
        ms.push(took);
    }
    ms.sort_by(|a, b| a.partial_cmp(b).expect("finite timings"));
    let p50 = ms[ms.len() / 2];
    let p95 = ms[(ms.len() * 95).div_ceil(100) - 1];
    let max = ms[ms.len() - 1];
    let body = format!(
        "group,endpoint,n,p50_ms,p95_ms,max_ms,status\nnetwork,{PATH},{SAMPLES},{p50:.1},{p95:.1},{max:.1},ok\n"
    );
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("DOCK/tiered");
    std::fs::create_dir_all(&dir).expect("DOCK/tiered");
    std::fs::write(dir.join("latency-extra.csv"), &body).expect("latency-extra.csv");
    eprintln!("{body}");
    assert_eq!(ms.len(), SAMPLES, "every declared sample was taken");
}
