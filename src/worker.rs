//! The router thread: one serialized session, one queue, no shared mutable state.
//!
//! The UI thread never blocks on the network - it drains events with `try_recv`
//! and keeps drawing at its own tick, so a hung request can only make the data
//! old (which the header reports as `stale`), never freeze the interface.

use crate::app::SessionState;
use crate::config::Config;
use crate::error::ApiError;
use crate::headless;
use crate::model::Snapshot;
use crate::router::bands::BandPayload;
use crate::router::HuaweiClient;
use crate::write::{apply_bands, apply_dns, WriteIntent, WriteState};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum Cmd {
    Refresh,
    Pause(bool),
    /// Change the poll interval without respawning the worker: the loop's wait
    /// is recomputed from this value, so `[`/`]` take effect on the next tick.
    SetInterval(u64),
    /// Ask the worker to finish: it logs out, acknowledges, then exits. The UI
    /// waits on the acknowledgement so the session is closed before the
    /// process ends (a bare `break` left the router with an open session).
    Stop(SyncSender<()>),
    /// Write path: the payload plus the intent it belongs to (for the UI).
    ApplyBands {
        payload: BandPayload,
        intent: WriteIntent,
    },
    ApplyDns {
        primary: String,
        secondary: String,
        reset: bool,
        intent: WriteIntent,
    },
    /// Local `ping` - never HTTP, runs on its own short-lived thread.
    Ping(String),
    /// A throughput test through this host: the config, the ping host, its run
    /// id, and the flag `Esc` sets. Runs on its own thread, never on the router
    /// thread.
    SpeedTest(crate::config::Speedtest, String, u64, Arc<AtomicBool>),
    /// Lazily fetch PIN / mobile-data / SMS inbox for the SMS and Device tabs.
    Extras,
    /// Restart the router. Only sent after a typed confirmation.
    Restart,
    /// Write a full JSON dump to this path.
    Dump(std::path::PathBuf),
}

#[derive(Debug)]
pub enum Ev {
    Snapshot {
        snap: Box<Snapshot>,
        poll_ms: u128,
    },
    Session(SessionState),
    Error(String),
    /// Intermediate and final write states (Pending → Verifying → Success/Failed).
    Write(WriteState),
    Ping(PingOutcome),
    /// Intermediate throughput while a test runs, tagged with its run id.
    SpeedProgress(u64, crate::speedtest::SpeedProgress),
    /// The finished test, or the reason it stopped.
    SpeedDone(u64, Result<Box<SpeedRun>, String>),
    /// PIN / mobile-data / inbox, fetched on demand.
    Extras(Box<crate::model::DeviceExtras>),
    /// One endpoint read inside the current scan: what the header shows.
    Phase {
        phase: crate::headless::Phase,
        state: crate::headless::PhaseState,
    },
    /// One-line message for the status bar (dump written, restart accepted…).
    Notice(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PingOutcome {
    pub host: String,
    pub sent: u32,
    pub received: u32,
    pub loss_pct: u32,
    pub avg_ms: Option<f64>,
    pub ok: bool,
    pub detail: String,
}

/// Parse `ping -c N` output without guessing: counts from the summary lines.
pub fn parse_ping(host: &str, output: &str) -> PingOutcome {
    let mut sent = 0u32;
    let mut received = 0u32;
    let mut loss_pct = 100u32;
    let mut avg_ms = None;

    for line in output.lines() {
        let l = line.trim();
        if l.contains("packets transmitted") {
            let nums: Vec<u32> = l
                .split(|c: char| !c.is_ascii_digit())
                .filter(|t| !t.is_empty())
                .filter_map(|t| t.parse().ok())
                .collect();
            if nums.len() >= 2 {
                sent = nums[0];
                received = nums[1];
            }
            if let Some(p) = l.split('%').next().and_then(|head| {
                head.split(|c: char| !c.is_ascii_digit())
                    .next_back()
                    .and_then(|n| n.parse::<u32>().ok())
            }) {
                loss_pct = p;
            }
        }
        if l.starts_with("rtt ") || l.starts_with("round-trip") {
            if let Some(vals) = l.split('=').nth(1) {
                avg_ms = vals
                    .trim()
                    .split('/')
                    .nth(1)
                    .and_then(|v| v.trim().parse::<f64>().ok());
            }
        }
    }

    PingOutcome {
        host: host.to_string(),
        sent,
        received,
        loss_pct,
        avg_ms,
        ok: received > 0,
        detail: if received > 0 {
            format!("{received}/{sent} replies · {loss_pct}% loss")
        } else {
            format!("no reply ({loss_pct}% loss)")
        },
    }
}

/// Arguments for the system `ping`.
///
/// `-c N` + `-W <seconds>` is the spelling shared by iputils (Arch, Fedora,
/// Debian, Ubuntu) and BusyBox (Alpine): busybox has no millisecond timer, and
/// `-W 2` means two seconds on every Linux distribution routwatch targets.
/// (BSD/macOS `-W` is milliseconds - out of scope, `docs/platforms.md` lists the
/// platforms this is built and tested on.)
pub fn ping_args(count: u32, host: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        count.to_string(),
        "-W".to_string(),
        "2".to_string(),
        host.to_string(),
    ]
}

/// Run the real ping binary off the UI thread and report one event.
fn spawn_ping(host: String, ev_tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let output = std::process::Command::new("ping")
            .args(ping_args(3, &host))
            .output();
        let outcome = match output {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout).to_string()
                    + &String::from_utf8_lossy(&o.stderr);
                let mut outcome = parse_ping(&host, &text);
                if !o.status.success() && outcome.received == 0 {
                    outcome.ok = false;
                    outcome.detail = format!("no reply ({})", o.status);
                }
                outcome
            }
            Err(e) => PingOutcome {
                host: host.clone(),
                sent: 0,
                received: 0,
                loss_pct: 100,
                avg_ms: None,
                ok: false,
                detail: format!("cannot run ping: {e}"),
            },
        };
        let _ = ev_tx.send(Ev::Ping(outcome));
    });
}

/// One measured run: the transfer, its two pings and its wall time.
#[derive(Debug, Clone)]
pub struct SpeedRun {
    pub result: crate::speedtest::SpeedResult,
    pub elapsed: Duration,
    pub pings: crate::history::Pings,
}

/// Quiet window before the idle ping, so a poll already in flight can finish.
const PING_WARM_SECS: u64 = 2;
const PING_IDLE_COUNT: u32 = 4;
const PING_LOADED_COUNT: u32 = 3;

/// `-w` bounds the whole ping, so a rate-limited ICMP path cannot outlast the
/// watchdog. Measured with iputils ping 20250625; busybox accepts `-w` too.
fn run_ping(host: &str, count: u32) -> Option<PingOutcome> {
    let args = [
        "-c".to_string(),
        count.to_string(),
        "-W".to_string(),
        "2".into(),
        "-w".to_string(),
        "6".into(),
        host.to_string(),
    ];
    let out = std::process::Command::new("ping")
        .args(&args)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(parse_ping(host, &text))
}

/// Ping, transfer, ping. The loaded ping runs on its own scoped thread and only
/// counts when it provably overlapped the transfer.
pub fn run_measured(
    cfg: &crate::config::Speedtest,
    host: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(crate::speedtest::SpeedProgress),
) -> Result<SpeedRun, String> {
    use std::sync::atomic::{AtomicBool as Flag, Ordering::Relaxed};
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs(PING_WARM_SECS));
    if cancel.load(Relaxed) {
        return Err("cancelled".into());
    }
    let idle = run_ping(host, PING_IDLE_COUNT);
    if cancel.load(Relaxed) {
        return Err("cancelled".into());
    }

    let busy = Arc::new(Flag::new(false));
    let seen = busy.clone();
    let (result, loaded) = std::thread::scope(|s| {
        let ping = s.spawn(move || {
            // Wait for the first byte, but never past this deadline: a transfer
            // that fails before it starts must not park the ping thread.
            let deadline = Instant::now() + Duration::from_secs(2);
            while !seen.load(Relaxed) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            let started = Instant::now();
            let out = if seen.load(Relaxed) {
                run_ping(host, PING_LOADED_COUNT)
            } else {
                None
            };
            (started, out)
        });
        let mut on_progress = |p: crate::speedtest::SpeedProgress| {
            if p.bytes > 0 {
                busy.store(true, Relaxed);
            }
            progress(p);
        };
        let r = crate::speedtest::run(cfg, cancel, &mut on_progress);
        let transfer_end = Instant::now();
        let (ping_started, out) = ping.join().unwrap_or((transfer_end, None));
        let overlapped = ping_started < transfer_end;
        (r, out.filter(|_| overlapped))
    });
    let result = result?;
    Ok(SpeedRun {
        result,
        elapsed: start.elapsed(),
        pings: crate::history::Pings {
            host: Some(host.to_string()),
            idle,
            loaded,
        },
    })
}

/// True while a measured run holds the poll loop. The pause is an instant, not
/// a flag, so a thread that dies mid-run still lets polling resume on its own;
/// a restore message would never arrive from a dead sender.
fn pausing(pause_until: Option<Instant>, now: Instant) -> bool {
    pause_until.is_some_and(|t| now < t)
}

/// Run the transfer off both the UI thread and the router thread: a test can
/// last a minute and must not delay a poll or a redraw. `id` comes back on
/// every event, so a late answer can be recognised and dropped.
fn spawn_speed(
    cfg: crate::config::Speedtest,
    host: String,
    id: u64,
    ev_tx: Sender<Ev>,
    cancel: Arc<AtomicBool>,
) {
    let tx = ev_tx.clone();
    let spawned = std::thread::Builder::new()
        .name("routwatch-speed".into())
        .spawn(move || {
            // read_for and the upload meter already emit at most every 250 ms;
            // throttling again here only discarded ticks.
            let out = run_measured(&cfg, &host, &cancel, &mut |p| {
                let _ = tx.send(Ev::SpeedProgress(id, p));
            });
            let _ = tx.send(Ev::SpeedDone(id, out.map(Box::new)));
        });
    // A failed spawn must not take the router thread down with it; the UI gets
    // an answer it can render instead of a silent forever-running test.
    if let Err(e) = spawned {
        let _ = ev_tx.send(Ev::SpeedDone(
            id,
            Err(format!("could not start the test thread: {e}")),
        ));
    }
}

/// Backoff ladder for a lost session (plan §4.1): 2/5/10/20s inside a 60s window.
const BACKOFF: [u64; 4] = [2, 5, 10, 20];

pub fn spawn(cfg: Config, password: String) -> (Sender<Cmd>, Receiver<Ev>) {
    let (cmd_tx, cmd_rx) = channel::<Cmd>();
    let (ev_tx, ev_rx) = channel::<Ev>();
    std::thread::Builder::new()
        .name("routwatch-router".into())
        .spawn(move || run(cfg, password, cmd_rx, ev_tx))
        .expect("spawn router thread");
    (cmd_tx, ev_rx)
}

fn run(cfg: Config, password: String, cmd_rx: Receiver<Cmd>, ev_tx: Sender<Ev>) {
    let mut interval = Duration::from_millis(cfg.app.refresh_ms.max(500));
    let mut client = HuaweiClient::new(&cfg.router.ip, cfg.app.timeout_ms);
    let mut authed = false;
    let mut paused = false;
    // Set while a speed run measures the link; it lapses on its own so a dead
    // speed thread cannot stop polling for good.
    let mut pause_until: Option<Instant> = None;
    let mut attempt: usize = 0;
    let mut next_try = Instant::now();

    let _ = ev_tx.send(Ev::Session(SessionState::Connecting));

    loop {
        // Wait for the tick, or handle a command immediately.
        let wait = if !authed {
            let now = Instant::now();
            next_try.saturating_duration_since(now).min(interval)
        } else {
            interval
        };
        match cmd_rx.recv_timeout(wait) {
            Ok(Cmd::Stop(ack)) => {
                if client.is_authed() {
                    client.logout();
                }
                let _ = ack.send(());
                break;
            }
            Ok(Cmd::Pause(p)) => {
                paused = p;
                continue;
            }
            Ok(Cmd::SetInterval(ms)) => {
                interval = Duration::from_millis(ms.clamp(500, 10_000));
                continue;
            }
            Ok(Cmd::ApplyDns {
                primary,
                secondary,
                reset,
                intent,
            }) => {
                let mut progress = |st: WriteState| {
                    let _ = ev_tx.send(Ev::Write(st));
                };
                let (p, sec) = if reset {
                    (String::new(), String::new())
                } else {
                    (primary, secondary)
                };
                let settled = apply_dns(&mut client, &p, &sec, intent, false, &mut progress);
                let reset_ok = matches!(&settled, WriteState::Success { .. });
                let _ = ev_tx.send(Ev::Write(settled));
                if reset_ok {
                    // DNS does not restart the radio; just refresh on the next tick.
                    let _ = ev_tx.send(Ev::Session(SessionState::Live));
                }
                continue;
            }
            Ok(Cmd::Ping(host)) => {
                spawn_ping(host, ev_tx.clone());
                continue;
            }
            Ok(Cmd::SpeedTest(cfg, host, id, cancel)) => {
                // While the run measures the link, polling would make the
                // "idle" ping a lie. The pause expires by itself, so a dead
                // speed thread cannot freeze the poller.
                pause_until = Some(
                    Instant::now()
                        + Duration::from_secs(crate::speedtest::worst_case_secs(&cfg) + 5),
                );
                spawn_speed(cfg, host, id, ev_tx.clone(), cancel);
                continue;
            }
            Ok(Cmd::Extras) => {
                let extras = headless::fetch_extras(&mut client);
                let _ = ev_tx.send(Ev::Extras(Box::new(extras)));
                continue;
            }
            Ok(Cmd::Dump(path)) => {
                // The TUI dump is a local file the user asked for; `x` has no
                // redaction flag, and `dump --redact` is the shareable path.
                match headless::dump_to(&mut client, &cfg.router.ip, &path, false, false) {
                    Ok(bytes) => {
                        let _ = ev_tx.send(Ev::Notice(format!(
                            "dump written: {} ({bytes} bytes)",
                            path.display()
                        )));
                    }
                    Err(e) => {
                        let _ = ev_tx.send(Ev::Error(format!("dump failed: {e}")));
                    }
                }
                continue;
            }
            Ok(Cmd::Restart) => {
                match client.restart() {
                    Ok(_) => {
                        let _ = ev_tx.send(Ev::Notice(
                            "restart accepted - waiting for the router to come back".into(),
                        ));
                        authed = false;
                        attempt = 0;
                        let _ = ev_tx.send(Ev::Session(SessionState::Restarting));
                        next_try = Instant::now() + Duration::from_secs(5);
                    }
                    Err(e) => {
                        let _ = ev_tx.send(Ev::Error(format!("restart refused: {e}")));
                    }
                }
                continue;
            }
            Ok(Cmd::ApplyBands { payload, intent }) => {
                let mut progress = |st: WriteState| {
                    let _ = ev_tx.send(Ev::Write(st));
                };
                let settled = apply_bands(&mut client, &payload, intent, false, &mut progress);
                let link_dropped = matches!(&settled, WriteState::Success { note, .. } if !note.contains("nothing sent"));
                let _ = ev_tx.send(Ev::Write(settled));
                if link_dropped {
                    // Expected: the radio restarts. Do not call it an error.
                    authed = false;
                    attempt = 0;
                    let _ = ev_tx.send(Ev::Session(SessionState::Restarting));
                    next_try = Instant::now() + Duration::from_secs(2);
                }
                continue;
            }
            Ok(Cmd::Refresh) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if paused || pausing(pause_until, Instant::now()) {
            continue;
        }

        if !authed {
            if Instant::now() < next_try {
                continue;
            }
            match client.login(&cfg.router.username, &password) {
                Ok(()) => {
                    authed = true;
                    attempt = 0;
                    let _ = ev_tx.send(Ev::Session(SessionState::Live));
                }
                Err(e) => {
                    let wait_s = BACKOFF[attempt.min(BACKOFF.len() - 1)];
                    attempt = (attempt + 1).min(BACKOFF.len());
                    // A hard auth failure will not fix itself: report it and stop
                    // retrying, so we never lock the account with a loop.
                    if matches!(e, ApiError::Auth(_)) || matches!(e, ApiError::Config(_)) {
                        let _ = ev_tx.send(Ev::Session(SessionState::Disconnected(e.to_string())));
                        let _ = ev_tx.send(Ev::Error(e.to_string()));
                        paused = true;
                        continue;
                    }
                    let _ = ev_tx.send(Ev::Session(SessionState::Recovering {
                        attempt: attempt as u8,
                        wait_s,
                    }));
                    let _ = ev_tx.send(Ev::Error(e.to_string()));
                    next_try = Instant::now() + Duration::from_secs(wait_s);
                    continue;
                }
            }
        }

        let started = Instant::now();
        let mut trace = |phase, state| {
            let _ = ev_tx.send(Ev::Phase { phase, state });
        };
        match headless::collect_traced(&mut client, &cfg.router.ip, &mut trace) {
            Ok(snap) => {
                let _ = ev_tx.send(Ev::Snapshot {
                    snap: Box::new(snap),
                    poll_ms: started.elapsed().as_millis(),
                });
            }
            Err(e) => {
                let _ = ev_tx.send(Ev::Error(e.to_string()));
                // Session-level failures mean the next tick must log in again.
                if matches!(
                    e,
                    ApiError::Invalid(_)
                        | ApiError::Denied(_)
                        | ApiError::Network(_)
                        | ApiError::Http(_)
                ) {
                    authed = false;
                    let wait_s = BACKOFF[attempt.min(BACKOFF.len() - 1)];
                    attempt = (attempt + 1).min(BACKOFF.len());
                    let _ = ev_tx.send(Ev::Session(SessionState::Recovering {
                        attempt: attempt as u8,
                        wait_s,
                    }));
                    next_try = Instant::now() + Duration::from_secs(wait_s);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_for(mock: &crate::mock_cpe::MockCpe) -> Config {
        let mut cfg = Config::default();
        cfg.router.ip = mock.addr().to_string();
        cfg.router.password = "pw".into();
        cfg
    }

    #[test]
    fn stopping_the_worker_logs_out_before_acknowledging() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::H122373);
        let (tx, _rx) = spawn(cfg_for(&mock), "pw".into());

        // Give the worker its first login; it retries on its own schedule.
        let mut waited = 0u32;
        while mock.logins() == 0 && waited < 60 {
            std::thread::sleep(Duration::from_millis(50));
            waited += 1;
        }
        assert!(mock.logins() >= 1, "the worker never logged in");

        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(0);
        tx.send(Cmd::Stop(ack_tx)).unwrap();
        ack_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("the worker must acknowledge the stop");
        assert_eq!(
            mock.logouts(),
            1,
            "the TUI promised logout on quit, and only one session was open"
        );
    }

    #[test]
    fn changing_the_interval_takes_effect_without_a_respawn() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::H122373);
        let (tx, _rx) = spawn(cfg_for(&mock), "pw".into());

        let mut waited = 0u32;
        while mock.signals() == 0 && waited < 100 {
            std::thread::sleep(Duration::from_millis(50));
            waited += 1;
        }
        assert!(mock.signals() >= 1, "the worker never polled");

        tx.send(Cmd::SetInterval(600)).unwrap();
        let base = mock.signals();
        std::thread::sleep(Duration::from_millis(2000));
        let extra = mock.signals() - base;
        assert!(
            extra >= 2,
            "the new interval was ignored: {extra} polls in 2s at 600ms"
        );

        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(0);
        tx.send(Cmd::Stop(ack_tx)).unwrap();
        let _ = ack_rx.recv_timeout(Duration::from_secs(3));
    }

    const SAMPLE: &str = "PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data.\n\
64 bytes from 1.1.1.1: icmp_seq=1 ttl=57 time=12.3 ms\n\
64 bytes from 1.1.1.1: icmp_seq=2 ttl=57 time=12.5 ms\n\
\n--- 1.1.1.1 ping statistics ---\n\
3 packets transmitted, 3 received, 0% packet loss, time 2003ms\n\
rtt min/avg/max/mdev = 12.100/12.400/12.800/0.300 ms\n";

    #[test]
    fn parses_a_real_ping_summary() {
        let p = parse_ping("1.1.1.1", SAMPLE);
        assert_eq!(p.sent, 3);
        assert_eq!(p.received, 3);
        assert_eq!(p.loss_pct, 0);
        assert_eq!(p.avg_ms, Some(12.4));
        assert!(p.ok);
        assert_eq!(p.detail, "3/3 replies · 0% loss");
    }

    #[test]
    fn parses_loss_and_no_reply() {
        let lossy = "--- 10.0.0.1 ping statistics ---\n3 packets transmitted, 1 received, 66% packet loss, time 2003ms\nrtt min/avg/max/mdev = 1.0/1.2/1.5/0.2 ms";
        let p = parse_ping("10.0.0.1", lossy);
        assert_eq!((p.sent, p.received, p.loss_pct), (3, 1, 66));
        assert!(p.ok);

        let dead = "--- 10.0.0.2 ping statistics ---\n3 packets transmitted, 0 received, 100% packet loss, time 2010ms";
        let p = parse_ping("10.0.0.2", dead);
        assert!(!p.ok);
        assert_eq!(p.avg_ms, None);
    }

    /// The thread, the run id, the events and the result, driven against the
    /// mock: a bare mpsc round-trip would pass even if `run` ignored the
    /// command.
    #[test]
    fn a_speed_test_sends_its_result_through_the_event_channel() {
        use crate::mock_cpe::{MockCpe, Profile};
        let cpe = MockCpe::start(Profile::H122373);
        let (tx, rx) = channel();
        let cfg = crate::config::Speedtest {
            download_url: format!("http://{}/__down?bytes={{bytes}}", cpe.addr()),
            upload_url: format!("http://{}/__up", cpe.addr()),
            duration_secs: 1,
            max_mb: 5,
            upload_mb: 1,
            ..crate::config::Speedtest::default()
        };
        spawn_speed(
            cfg,
            "127.0.0.1".into(),
            7,
            tx,
            Arc::new(AtomicBool::new(false)),
        );
        let mut seen = Vec::new();
        loop {
            let out = rx.recv_timeout(Duration::from_secs(15)).expect("an answer");
            if let Ev::SpeedDone(id, res) = &out {
                assert_eq!(*id, 7, "the answer carries the run it belongs to");
                let run = res.as_ref().expect("a completed run");
                // The idle ping is measured before the first byte, so it is
                // always there; the loaded one is dropped when the transfer
                // ended before it could start, which this mock may do.
                assert_eq!(run.pings.host.as_deref(), Some("127.0.0.1"));
                assert!(
                    run.pings.idle.is_some(),
                    "the quiet ping runs before the transfer"
                );
                break;
            }
            seen.push(out);
        }
        assert!(
            !seen.is_empty(),
            "a five megabyte transfer through the mock emits progress"
        );
    }

    /// The poll pause expires on its own: nothing has to be sent back from the
    /// measuring thread for polling to resume.
    #[test]
    fn the_pause_expires_without_a_restore_message() {
        let now = Instant::now();
        assert!(!pausing(None, now), "no run, no pause");
        assert!(pausing(Some(now + Duration::from_secs(5)), now));
        assert!(
            !pausing(
                Some(now + Duration::from_secs(5)),
                now + Duration::from_secs(6)
            ),
            "past the budget the loop polls again"
        );
    }
}
