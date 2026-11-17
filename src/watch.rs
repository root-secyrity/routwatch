//! The watch kept open after a band write: `bands --set ... --watch N`.
//!
//! A band write restarts the radio, and the damage (a link that does not come
//! back) shows up minutes later, which is why the offer to revert outlives the
//! command that made it. Nothing here writes on its own: the revert is offered
//! once, and only a `y` from the caller's confirmation takes it.

use crate::error::ApiError;
use crate::router::bands::BandPayload;
use crate::router::client::HuaweiClient;
use crate::router::parse;
use crate::write::{self, VerifyLimits, WriteIntent};

use std::time::Duration;

/// How often the link is read, and how long it may stay down before the revert
/// is offered. A radio restart drops the address for a few seconds as a matter
/// of course; a minute of it is not normal.
pub const POLL: Duration = Duration::from_secs(5);
pub const DOWN_SECS: u64 = 60;

/// Everything the loop needs from the outside world, so a test can run it with
/// no clock and no keyboard.
pub struct WatchIo<'a> {
    pub sleep: &'a mut dyn FnMut(Duration),
    pub confirm: &'a mut dyn FnMut() -> bool,
    pub log: &'a mut dyn FnMut(String),
}

/// What the watch did, for the caller's exit code and for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchResult {
    pub polls: usize,
    pub seconds_down: u64,
    /// A revert was offered and taken; the mask on the device is the old one.
    pub reverted: bool,
}

/// Watch the link for `secs` seconds.
///
/// A failed read is retried through one re-login: the restart drops the cookie
/// but not the credentials. The revert is offered after [`DOWN_SECS`] of a link
/// with neither a known state nor an address, and only `confirm` returning true
/// takes it, because the tool cannot tell a band it dropped from an ISP that
/// went away.
pub fn watch_link(
    client: &mut HuaweiClient,
    creds: (&str, &str),
    undo: Option<(&BandPayload, &WriteIntent)>,
    verify_secs: u64,
    secs: u64,
    io: &mut WatchIo,
) -> Result<WatchResult, ApiError> {
    let mut out = WatchResult {
        polls: 0,
        seconds_down: 0,
        reverted: false,
    };
    let mut left = secs;
    let mut offered = false;
    let mut reconnected = false;
    let read = |client: &mut HuaweiClient| -> Result<(Option<u32>, Option<String>), ApiError> {
        let status = client
            .get("monitoring/status")
            .ok()
            .and_then(|n| parse::connection_status(&n));
        let ip = parse::wan_addr(client)?;
        Ok((status, ip))
    };
    while left > 0 {
        (io.sleep)(POLL);
        left = left.saturating_sub(POLL.as_secs());
        out.polls += 1;
        let (status, ip) = match read(client) {
            Ok(v) => {
                reconnected = false;
                v
            }
            Err(e) => {
                if reconnected || client.login(creds.0, creds.1).is_err() {
                    return Err(e);
                }
                reconnected = true;
                (io.log)("session lost, signed in again".to_string());
                read(client)?
            }
        };
        let up = matches!(status, Some(900 | 901)) && ip.is_some();
        if up {
            out.seconds_down = 0;
        } else {
            out.seconds_down += POLL.as_secs();
        }
        (io.log)(format!(
            "{left}s left · connection {} · wan {}",
            status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".to_string()),
            if ip.is_some() { "up" } else { "none" }
        ));
        if !up && out.seconds_down >= DOWN_SECS && !offered {
            offered = true;
            match undo {
                Some((payload, intent)) => {
                    (io.log)(format!(
                        "link down {}s: put the previous mask back (0x{}, mode {})? [y/N]",
                        out.seconds_down, payload.lte_band, payload.network_mode
                    ));
                    if (io.confirm)() {
                        let limits = VerifyLimits::from_secs(verify_secs);
                        let mut progress = |st: write::WriteState| (io.log)(st.label().to_string());
                        let settled = write::apply_bands_with(
                            client,
                            payload,
                            intent.clone(),
                            true,
                            &mut progress,
                            limits,
                        );
                        (io.log)(format!("revert: {}", settled.label()));
                        out.reverted = matches!(settled, write::WriteState::Success { .. });
                        return Ok(out);
                    }
                    (io.log)("revert not taken, the mask stays as applied".to_string());
                }
                None => (io.log)(format!(
                    "link down {}s: no previous mask to put back",
                    out.seconds_down
                )),
            }
        }
    }
    (io.log)(format!("{secs}s over, the mask stays as applied"));
    Ok(out)
}
