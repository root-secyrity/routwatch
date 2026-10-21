//! The router thread: one serialized session, one queue, no shared mutable state.
//!
//! The UI thread never blocks on the network — it drains events with `try_recv`
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
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum Cmd {
    Refresh,
    Pause(bool),
    Stop,
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
    /// Local `ping` — never HTTP, runs on its own short-lived thread.
    Ping(String),
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
    /// PIN / mobile-data / inbox, fetched on demand.
    Extras(Box<crate::model::DeviceExtras>),
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
/// (BSD/macOS `-W` is milliseconds — out of scope, see README.)
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
    let interval = Duration::from_millis(cfg.app.refresh_ms.max(500));
    let mut client = HuaweiClient::new(&cfg.router.ip, cfg.app.timeout_ms);
    let mut authed = false;
    let mut paused = false;
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
            Ok(Cmd::Stop) => break,
            Ok(Cmd::Pause(p)) => {
                paused = p;
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
            Ok(Cmd::Extras) => {
                let extras = headless::fetch_extras(&mut client);
                let _ = ev_tx.send(Ev::Extras(Box::new(extras)));
                continue;
            }
            Ok(Cmd::Dump(path)) => {
                match headless::dump_to(&mut client, &cfg.router.ip, &path) {
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
                            "restart accepted — waiting for the router to come back".into(),
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
        if paused {
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
        match headless::collect(&mut client, &cfg.router.ip) {
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
}
