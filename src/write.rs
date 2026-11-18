//! The write path: one intent at a time, verified by reading the router back.
//!
//! Rules that matter more than the code:
//! - never retry a write on timeout - read `net/net-mode` first and compare the
//!   mask (idempotency guard);
//! - a verified no-op is a success, not a failure ("already active");
//! - the caller decides whether writing is allowed at all.

use crate::error::ApiError;
use crate::model::NetMode;
use crate::router::bands::{self, BandPayload};
use crate::router::client::HuaweiClient;
use crate::router::parse;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteIntent {
    ApplyBands {
        mode: String,
        bands: Vec<u8>,
        mask: String,
        option: Option<String>,
    },
    SetDns {
        primary: String,
        secondary: String,
    },
    ResetDns,
}

impl WriteIntent {
    pub fn label(&self) -> String {
        match self {
            WriteIntent::ApplyBands { mode, bands, .. } => format!(
                "mode {} · {} bands ({})",
                mode,
                bands.len(),
                bands
                    .iter()
                    .map(|b| format!("B{b}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            WriteIntent::SetDns { primary, secondary } => {
                format!("DNS {primary} / {secondary}")
            }
            WriteIntent::ResetDns => "DNS: router answers DNS itself".to_string(),
        }
    }
}

/// Strict IPv4 check - no DNS hostnames, no IPv6, no empty strings.
pub fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.chars().all(|c| c.is_ascii_digit())
                && p.parse::<u16>().map(|n| n <= 255).unwrap_or(false)
        })
}

/// "1.1.1.1, 1.0.0.1" -> (primary, secondary)
pub fn parse_dns_pair(raw: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = raw
        .split([',', ' '])
        .filter(|p| !p.trim().is_empty())
        .collect();
    match parts.len() {
        2 => {
            let (a, b) = (parts[0].trim(), parts[1].trim());
            if !is_ipv4(a) {
                return Err(format!("primary DNS is not an IPv4 address: {a:?}"));
            }
            if !is_ipv4(b) {
                return Err(format!("secondary DNS is not an IPv4 address: {b:?}"));
            }
            Ok((a.to_string(), b.to_string()))
        }
        0 => Err("no DNS servers given; expected PRIMARY,SECONDARY".into()),
        _ => Err(format!(
            "expected exactly two servers (PRIMARY,SECONDARY), got {}",
            parts.len()
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteState {
    Idle,
    Confirming(WriteIntent),
    Pending { intent: WriteIntent, since: Instant },
    Verifying { intent: WriteIntent, since: Instant },
    Success { intent: WriteIntent, note: String },
    Failed { intent: WriteIntent, err: String },
}

impl WriteState {
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            WriteState::Pending { .. } | WriteState::Verifying { .. }
        )
    }

    pub fn is_settled(&self) -> bool {
        matches!(self, WriteState::Success { .. } | WriteState::Failed { .. })
    }

    pub fn intent(&self) -> Option<&WriteIntent> {
        match self {
            WriteState::Confirming(i)
            | WriteState::Pending { intent: i, .. }
            | WriteState::Verifying { intent: i, .. }
            | WriteState::Success { intent: i, .. }
            | WriteState::Failed { intent: i, .. } => Some(i),
            WriteState::Idle => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            WriteState::Idle => "idle".into(),
            WriteState::Confirming(i) => format!("confirm: {}", i.label()),
            WriteState::Pending { intent, since } => {
                format!(
                    "applying… {}s · {}",
                    since.elapsed().as_secs(),
                    intent.label()
                )
            }
            WriteState::Verifying { since, .. } => {
                format!("verifying read-back… {}s", since.elapsed().as_secs())
            }
            WriteState::Success { note, .. } => format!("applied · {note}"),
            WriteState::Failed { err, .. } => format!("failed · {err}"),
        }
    }
}

/// Read the current mask as a hex string (device spelling, no 0x).
pub fn read_mask(client: &mut HuaweiClient) -> Result<(NetMode, Option<String>), ApiError> {
    let node = client.get("net/net-mode")?;
    let mode = parse::parse_net_mode(&node);
    let mask = mode
        .lte_band
        .as_deref()
        .and_then(bands::from_hex)
        .map(bands::to_hex);
    Ok((mode, mask))
}

/// The whole device state a write must reproduce, not just the mask.
pub fn read_mode(client: &mut HuaweiClient) -> Result<NetMode, ApiError> {
    let node = client.get("net/net-mode")?;
    Ok(parse::parse_net_mode(&node))
}

/// True when the device answered an auto-mode write by reporting `NetworkBand 0`
/// while the LTE mask it kept is the one we asked for. H122-373 does exactly
/// this: it accepts `3FFFFFFF`, applies it, and then publishes 0 for the
/// network mask. Treating that as a mismatch turned a successful write into a
/// failed one on real hardware.
pub fn auto_reports_zero_band(current: &NetMode, payload: &BandPayload) -> bool {
    let zero = current.network_band.as_deref().and_then(bands::from_hex) == Some(0);
    let lte_ok = current
        .lte_band
        .as_deref()
        .and_then(bands::from_hex)
        .zip(bands::from_hex(&payload.lte_band))
        .map(|(a, b)| a == b)
        .unwrap_or(false);
    payload.network_mode == "00" && zero && lte_ok
}

/// Does the device now match every field the payload asked for?
///
/// The pre-write no-op guard keeps `same_band_target`; this one is the
/// post-write truth and compares `NetworkBand` with the payload as well,
/// because in Auto mode `NetworkBand` and `LTEBand` carry different masks.
pub fn matches_after(current: &NetMode, payload: &BandPayload) -> bool {
    let hex_eq = |have: Option<&str>, want: &str| {
        have.and_then(bands::from_hex)
            .zip(bands::from_hex(want))
            .map(|(a, b)| a == b)
            .unwrap_or(false)
    };
    let mode_ok = current.network_mode.as_deref() == Some(payload.network_mode.as_str());
    let band_ok = (hex_eq(current.network_band.as_deref(), &payload.network_band)
        || auto_reports_zero_band(current, payload))
        && hex_eq(current.lte_band.as_deref(), &payload.lte_band);
    let want_opt = payload
        .network_option
        .clone()
        .unwrap_or_else(|| "0".to_string());
    let have_opt = current
        .network_option
        .clone()
        .unwrap_or_else(|| "0".to_string());
    mode_ok && band_ok && want_opt == have_opt
}

/// A refused request and a lost response are different facts; the message says which.
fn write_cause(e: &ApiError) -> String {
    match e {
        ApiError::Network(_) => format!("response lost ({e})"),
        _ => format!("router refused ({e})"),
    }
}

/// The one code this firmware answers next to a band write that did land.
///
/// Huawei's published table names 100002 ERROR_NOT_SUPPORT, 100003 ERROR_NO_RIGHT,
/// 100005 ERROR_FORMAT_ERROR and 100006 ERROR_PARAMETER_ERROR; on the H122-373
/// 100006 arrives on `POST net/net-mode` after the radio accepted the payload. So
/// it is a note, never a verdict: only the read-back decides. Three device runs in
/// this repo's log agree, and every one of them applied.
const SWITCH_WARNING: u32 = 100006;

fn switch_warning(e: &ApiError) -> Option<u32> {
    match e {
        ApiError::Router(code) if *code == SWITCH_WARNING => Some(*code),
        _ => None,
    }
}

/// The no-op test: the device is only "already there" when the mode, the mask
/// and the NR option all match. Comparing the mask alone once treated a
/// 4G→auto switch as a no-op and silently sent nothing.
pub fn same_band_target(current: &NetMode, payload: &BandPayload) -> bool {
    let mask_matches = current
        .lte_band
        .as_deref()
        .and_then(bands::from_hex)
        .zip(bands::from_hex(&payload.lte_band))
        .map(|(a, b)| a == b)
        .unwrap_or(false);
    let mode_matches = current.network_mode.as_deref() == Some(payload.network_mode.as_str());
    let want_opt = payload
        .network_option
        .clone()
        .unwrap_or_else(|| "0".to_string());
    let have_opt = current
        .network_option
        .clone()
        .unwrap_or_else(|| "0".to_string());
    mask_matches && mode_matches && want_opt == have_opt
}

/// How long the radio gets to publish the mask it accepted. A band write
/// restarts the modem, and its own UI shows the new mask only after the radio
/// comes back; one read lands too early. Measured on H122-373 (WEBUI 11.0.2.21).
pub const VERIFY_REREAD: Duration = Duration::from_secs(5);

/// The closing window when the app config carries no value.
pub const VERIFY_WINDOW_DEFAULT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyLimits {
    pub reread: Duration,
    pub window: Duration,
}

impl Default for VerifyLimits {
    fn default() -> Self {
        VerifyLimits {
            reread: VERIFY_REREAD,
            window: VERIFY_WINDOW_DEFAULT,
        }
    }
}

/// Re-read until `accept` says yes, or until the window closes. Generic over the
/// value a read returns, so a WAN address is polled by the same loop as a band
/// mask. The last error is carried out, because "the device stopped answering"
/// and "the device answered the wrong thing" are different failures.
pub fn poll_with<T, F>(
    mut read: F,
    accept: impl Fn(&T) -> bool,
    limits: VerifyLimits,
    mut sleep: impl FnMut(Duration),
) -> (Option<T>, u32, Option<ApiError>)
where
    F: FnMut() -> Result<T, ApiError>,
{
    let mut elapsed = Duration::ZERO;
    let mut attempts = 0u32;
    let mut last_err: Option<ApiError>;
    loop {
        attempts += 1;
        match read() {
            Ok(v) if accept(&v) => return (Some(v), attempts, None),
            Ok(_) => last_err = None,
            Err(e) => last_err = Some(e),
        }
        if elapsed >= limits.window {
            return (None, attempts, last_err);
        }
        sleep(limits.reread);
        elapsed += limits.reread;
    }
}

impl VerifyLimits {
    /// A user-facing number is clamped: below 5 s the window cannot outlast the
    /// modem restart, and above 120 s the CLI looks hung.
    pub fn from_secs(secs: u64) -> VerifyLimits {
        VerifyLimits {
            window: Duration::from_secs(secs.clamp(5, 120)),
            ..Default::default()
        }
    }
}

/// The band-mask case: `poll_with` with the device model's own reader.
pub fn poll_until<F, M>(
    read: F,
    matches: M,
    limits: VerifyLimits,
    sleep: impl FnMut(Duration),
) -> (Option<NetMode>, u32, Option<ApiError>)
where
    F: FnMut() -> Result<NetMode, ApiError>,
    M: Fn(&NetMode) -> bool,
{
    poll_with(read, matches, limits, sleep)
}

/// Apply an intent, verifying against the device. Returns the settled state  -
/// never an intermediate one.
pub fn apply_bands(
    client: &mut HuaweiClient,
    payload: &BandPayload,
    intent: WriteIntent,
    force: bool,
    progress: &mut dyn FnMut(WriteState),
) -> WriteState {
    apply_bands_with(
        client,
        payload,
        intent,
        force,
        progress,
        VerifyLimits::default(),
    )
}

/// The same write with an explicit verification window, so a test can drive the
/// poll without waiting twenty seconds.
pub fn apply_bands_with(
    client: &mut HuaweiClient,
    payload: &BandPayload,
    intent: WriteIntent,
    force: bool,
    progress: &mut dyn FnMut(WriteState),
    limits: VerifyLimits,
) -> WriteState {
    // Guard 1: if the device already reports this mask, sending anything is a
    // no-op and a needless link reset.
    let target = bands::from_hex(&payload.lte_band).unwrap_or(0);
    match read_mask(client) {
        Ok((current, Some(mask_hex))) => {
            if !force && same_band_target(&current, payload) {
                return WriteState::Success {
                    intent,
                    note: format!("already active (mode + mask 0x{mask_hex}) - nothing sent"),
                };
            }
        }
        Ok((_, None)) => {}
        Err(e) => {
            return WriteState::Failed {
                intent,
                err: format!("cannot read current mode: {e}"),
            };
        }
    }

    progress(WriteState::Pending {
        intent: intent.clone(),
        since: Instant::now(),
    });

    let sent = client.set_net_mode(payload);
    progress(WriteState::Verifying {
        intent: intent.clone(),
        since: Instant::now(),
    });

    // Guard 2: the device is the truth. Never re-send: read back until the mode,
    // both masks and the NR option match what was asked for, or the window closes.
    //
    // A refusal is a verdict, not a delay. Only an accepted write, or the one
    // warning code this firmware sends after it applied the payload, is worth
    // polling for: `poll_with` reads once before it looks at the window, so an
    // empty window would still accept a mask the device already carried.
    let may_land = sent.is_ok() || sent.as_ref().err().and_then(switch_warning).is_some();

    let (hit, attempts, last_err) = if may_land {
        poll_until(
            || read_mode(client),
            |cur| matches_after(cur, payload),
            limits,
            std::thread::sleep,
        )
    } else {
        // Nothing was sent successfully: there is no answer to wait for.
        (None, 0u32, None)
    };

    if let Some(current) = &hit {
        let zero = if auto_reports_zero_band(current, payload) {
            "; device reports NetworkBand 0 in auto mode"
        } else {
            ""
        };
        return WriteState::Success {
            intent,
            note: format!(
                "{}{zero}",
                match &sent {
                    Ok(_) => format!(
                        "verified, device mode {} mask 0x{}",
                        current.network_mode.clone().unwrap_or_else(|| "-".into()),
                        bands::to_hex(target)
                    ),
                    Err(e) => match switch_warning(e) {
                        Some(code) => format!(
                            "router answered {code} while switching; read-back confirms the change"
                        ),
                        None => format!("{}; read-back confirms the change", write_cause(e)),
                    },
                }
            ),
        };
    }

    // What the device kept. A poll that never matched still leaves one question
    // open - what did it answer last? - so the failure path reads once more. A
    // refusal sent nothing, and saying "no read after the refusal" is the truth.
    let describe = |cur: &NetMode| {
        format!(
            "device still reports mode {} mask 0x{}",
            cur.network_mode.clone().unwrap_or_else(|| "-".into()),
            cur.lte_band
                .as_deref()
                .and_then(bands::from_hex)
                .map(bands::to_hex)
                .unwrap_or_else(|| "-".into())
        )
    };
    let kept = match (&hit, may_land) {
        (Some(cur), _) => describe(cur),
        (None, true) => match read_mode(client) {
            Ok(cur) => describe(&cur),
            Err(e) => format!("no read after the refusal ({e})"),
        },
        (None, false) => "no read after the refusal".to_string(),
    };
    WriteState::Failed {
        intent,
        err: match &sent {
            Err(e) => match switch_warning(e) {
                Some(code) => format!(
                    "router answered {code} and the change did not land within {}s after {attempts} reads: {kept}",
                    limits.window.as_secs()
                ),
                None => format!("{}; {kept}", write_cause(e)),
            },
            Ok(_) => format!(
                "write sent, but the device never reported the new mask after {attempts} reads: {} ({kept})",
                last_err
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "no read error".into())
            ),
        },
    }
}

/// What the write will send, decided in one place so `--dry-run` and the write
/// itself cannot drift apart.
pub struct DnsPlan {
    pub primary: String,
    pub secondary: String,
    pub body: String,
}

/// The two servers a DNS write will advertise, and the exact body it will POST.
///
/// "Automatic" on this firmware is not a mode bit: it is the router answering DNS
/// itself, which the DHCP panel advertises as `DhcpIPAddress` twice.
pub fn plan_dns(
    doc: &crate::model::DhcpSettings,
    reset: bool,
    primary: &str,
    secondary: &str,
) -> Result<DnsPlan, String> {
    let (p, s) = if reset {
        let lan = doc
            .dhcp_ip
            .clone()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                "the device did not report DhcpIPAddress, so automatic DNS has no target"
                    .to_string()
            })?;
        (lan.clone(), lan)
    } else {
        parse_dns_pair(&format!("{primary},{secondary}"))?;
        (primary.to_string(), secondary.to_string())
    };
    let body = doc.body_with_dns(&p, &s);
    Ok(DnsPlan {
        primary: p,
        secondary: s,
        body,
    })
}

/// What the device kept, in plain words. Never `{:?}` on an Option: the debug
/// quotes are what made the modal message longer than the modal.
fn dns_detail(d: &crate::model::DhcpSettings) -> String {
    format!(
        "device keeps {} / {}",
        d.primary.clone().unwrap_or_else(|| "nothing".into()),
        d.secondary.clone().unwrap_or_else(|| "nothing".into())
    )
}

/// DNS write: send the whole `dhcp/settings` document, verify the two addresses.
pub fn apply_dns(
    client: &mut HuaweiClient,
    primary: &str,
    secondary: &str,
    intent: WriteIntent,
    force: bool,
    progress: &mut dyn FnMut(WriteState),
) -> WriteState {
    let reset = matches!(intent, WriteIntent::ResetDns);

    // Ask the firmware whether it exposes LAN DHCP DNS before anything else. The
    // document POST is accepted either way: it applies the DHCP range and lease and
    // silently drops the DNS trio, which then reads back as a mismatch the user has
    // to decode. This check turns an impossible write into a plain answer.
    if client.dhcp_dns_editable() == Some(false) {
        return WriteState::Failed {
            intent,
            err: "this firmware has no LAN DHCP DNS setting (dhcpsdns_display=0), so the router would ignore it: nothing was sent".into(),
        };
    }

    // The POST is a whole-document write, so the document is read first, with its own
    // bytes. Without it there is nothing safe to send and we say so instead of sending
    // three fields.
    let before = match client.get_dhcp() {
        Ok(d) => d,
        Err(e) => {
            return WriteState::Failed {
                intent,
                err: format!("cannot read dhcp/settings before writing: {e}"),
            }
        }
    };
    let DnsPlan {
        primary: target_primary,
        secondary: target_secondary,
        body,
    } = match plan_dns(&before, reset, primary, secondary) {
        Ok(plan) => plan,
        Err(e) => {
            return WriteState::Failed { intent, err: e };
        }
    };

    let landed = |d: &crate::model::DhcpSettings| {
        d.primary.as_deref() == Some(target_primary.as_str())
            && d.secondary.as_deref() == Some(target_secondary.as_str())
    };

    if landed(&before) && !force {
        return WriteState::Success {
            intent,
            note: "already active - nothing sent".into(),
        };
    }

    progress(WriteState::Pending {
        intent: intent.clone(),
        since: Instant::now(),
    });
    let sent = client.post("dhcp/settings", &body);
    progress(WriteState::Verifying {
        intent: intent.clone(),
        since: Instant::now(),
    });

    let after = client
        .get("dhcp/settings")
        .ok()
        .map(|n| parse::parse_dhcp(&n));
    let verified = after.as_ref().map(landed);

    match (&sent, verified) {
        // The value is right and the POST failed: the response was lost, not the
        // setting. Say which, and never claim the write itself worked.
        (Err(e), Some(true)) => WriteState::Success {
            intent,
            note: format!("{}; read-back confirms the change", write_cause(e)),
        },
        (_, Some(true)) => WriteState::Success {
            intent,
            note: if reset {
                format!("verified, router answers DNS itself ({target_primary})")
            } else {
                format!("verified, device reports {target_primary} / {target_secondary}")
            },
        },
        (Err(e), _) => WriteState::Failed {
            intent,
            err: format!("{}; read-back did not confirm", write_cause(e)),
        },
        (Ok(_), Some(false)) => WriteState::Failed {
            intent,
            err: match &after {
                Some(d) => format!(
                    "mismatch after write: wanted {target_primary} / {target_secondary}, {}",
                    dns_detail(d)
                ),
                None => "mismatch after write: the read-back came back unreadable".into(),
            },
        },
        (Ok(_), None) => WriteState::Failed {
            intent,
            err: "write sent, but reading dhcp/settings back failed".into(),
        },
    }
}

/// How a renew ended. An unchanged address is a result about the ISP, not a
/// failure; a link still down is a failure the caller has to say out loud.
#[derive(Debug, Clone, PartialEq)]
pub enum RenewOutcome {
    Changed {
        before: String,
        after: String,
        secs: u64,
    },
    Unchanged {
        ip: String,
        secs: u64,
    },
    /// The data connection was switched off and never came back on.
    LinkDown {
        why: String,
    },
    /// Back on, but no address within the cap.
    NoAddress {
        attempts: u32,
        last_err: Option<String>,
    },
}

impl RenewOutcome {
    /// The exit code a script reads: a kept lease is a result, a dead link is not.
    pub fn exit_code(&self) -> i32 {
        match self {
            RenewOutcome::Changed { .. } | RenewOutcome::Unchanged { .. } => 0,
            RenewOutcome::LinkDown { .. } | RenewOutcome::NoAddress { .. } => 1,
        }
    }
}

/// Bring the data connection back after it was dropped. A dropped link that is
/// never re-enabled takes the user offline until he opens the router page, so a
/// failed attempt is retried, and a link still down is a failure, not a warning.
fn re_enable(client: &mut HuaweiClient, mut sleep: impl FnMut(Duration)) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 1..=3u8 {
        match client.post(
            "dialup/mobile-dataswitch",
            HuaweiClient::dataswitch_payload(true),
        ) {
            Ok(_) => return Ok(()),
            Err(e) => last = format!("attempt {attempt}: {e}"),
        }
        // No wait after the last attempt: it buys nothing and the test sleeps
        // through it for nothing.
        if attempt < 3 {
            sleep(Duration::from_secs(3));
        }
    }
    Err(last)
}

/// Ask the network for a new address: drop the data connection, wait, bring it
/// back, then watch the device report a WAN IP. A link that is already down is
/// switched on, never off first.
///
/// The sleep is a parameter, not `std::thread::sleep` written inside: the
/// failure test passes its own and does not wait nine real seconds. The wait
/// that counts is `--settle`'s cap, spent as a poll that returns the moment an
/// address appears.
pub fn renew_ip(
    client: &mut HuaweiClient,
    limits: VerifyLimits,
    mut sleep: impl FnMut(Duration),
) -> RenewOutcome {
    // Seconds of wait actually spent, so the report is honest on an injected
    // sleep too and no caller has to read a wall clock.
    let mut slept = 0u64;
    let before = parse::wan_addr(client).unwrap_or(None);
    if before.is_some() {
        let _ = client.post(
            "dialup/mobile-dataswitch",
            HuaweiClient::dataswitch_payload(false),
        );
        sleep(Duration::from_secs(3));
        slept += 3;
    }
    let mut counting = |d: Duration| {
        slept += d.as_secs();
        sleep(d);
    };
    if let Err(why) = re_enable(client, &mut counting) {
        return RenewOutcome::LinkDown { why };
    }
    let (hit, attempts, last_err) = poll_with(
        || parse::wan_addr(client),
        |addr: &Option<String>| addr.as_deref().is_some_and(|s| !s.trim().is_empty()),
        limits,
        &mut counting,
    );
    match hit.flatten() {
        Some(after) => match before {
            Some(b) if b == after => RenewOutcome::Unchanged {
                ip: after,
                secs: slept,
            },
            Some(b) => RenewOutcome::Changed {
                before: b,
                after,
                secs: slept,
            },
            // The link was off, so the address is new and there is no before to
            // name: saying "none" is truer than inventing one.
            None => RenewOutcome::Changed {
                before: "none".to_string(),
                after,
                secs: slept,
            },
        },
        None => RenewOutcome::NoAddress {
            attempts,
            last_err: last_err.map(|e| e.to_string()),
        },
    }
}

/// The two codes that mean "this box has no such operation", not "try again".
/// Huawei's published table names 100002 ERROR_NOT_SUPPORT and 100003
/// ERROR_NO_RIGHT; a delete refused with either is a capability answer, and the
/// caller must disable its keys instead of retrying.
pub const SMS_UNSUPPORTED_CODES: [u32; 2] = [100002, 100003];

/// Which SMS operation a plan or a report belongs to. The two share the gate,
/// the confirmation and the read-back window, and differ only in what counts as
/// settled: a row that is gone, or a row the device now reports as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmsOp {
    Delete,
    MarkRead,
}

impl SmsOp {
    pub fn verb(&self) -> &'static str {
        match self {
            SmsOp::Delete => "deleted",
            SmsOp::MarkRead => "marked read",
        }
    }
}

/// What the reread proved, per row. `deleted` is the verified count only, never
/// the number of POSTs that came back OK: an answer is not a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsReport {
    pub op: SmsOp,
    pub attempted: usize,
    pub deleted: usize,
    /// (index, reason) for every row that is not proven settled.
    pub failed: Vec<(String, String)>,
    /// (index, code) when the firmware refused the whole operation.
    pub unsupported: Option<(String, String)>,
    pub rereads: u32,
}

impl SmsReport {
    fn new(op: SmsOp, attempted: usize) -> Self {
        SmsReport {
            op,
            attempted,
            deleted: 0,
            failed: Vec::new(),
            unsupported: None,
            rereads: 0,
        }
    }

    /// One line for the status bar: both counts, always.
    pub fn summary(&self) -> String {
        let mut line = format!("{} {} of {}", self.op.verb(), self.deleted, self.attempted);
        if !self.failed.is_empty() {
            line.push_str(&format!(" · {} failed", self.failed.len()));
        }
        if let Some((_, code)) = &self.unsupported {
            line.push_str(&format!(" · unsupported ({code})"));
        }
        line
    }

    /// Per-row detail, for the CLI and the write log.
    pub fn details(&self) -> Vec<String> {
        self.failed
            .iter()
            .map(|(index, why)| format!("index {index}: {why}"))
            .collect()
    }

    pub fn settled(&self) -> bool {
        self.failed.is_empty() && self.unsupported.is_none()
    }
}

/// Is this refusal the firmware saying the operation does not exist?
fn unsupported_code(e: &ApiError) -> Option<String> {
    // The code is what says "this box has no such path", whichever variant the
    // transport mapped it to (100002 not-support, 100003 no-right).
    let code = match e {
        ApiError::Unsupported(c)
        | ApiError::Denied(c)
        | ApiError::Invalid(c)
        | ApiError::Busy(c)
        | ApiError::Router(c) => *c,
        _ => return None,
    };
    SMS_UNSUPPORTED_CODES
        .contains(&code)
        .then(|| code.to_string())
}

/// Delete the rows one index at a time, then prove it with one reread loop.
///
/// The firmware takes a single index per request, so a bulk delete is a loop
/// with a result per row; the first refusal that names the operation stops it,
/// because carrying on would keep writing to a path this box does not have.
pub fn run_delete<D, R, S>(
    indexes: &[String],
    mut post: D,
    reread: R,
    limits: VerifyLimits,
    sleep: S,
) -> SmsReport
where
    D: FnMut(&str) -> Result<(), ApiError>,
    R: FnMut() -> Result<Vec<String>, ApiError>,
    S: FnMut(Duration),
{
    let mut report = SmsReport::new(SmsOp::Delete, indexes.len());
    let mut posted: Vec<String> = Vec::new();
    for (at, index) in indexes.iter().enumerate() {
        match post(index) {
            Ok(()) => posted.push(index.clone()),
            Err(e) => {
                if let Some(code) = unsupported_code(&e) {
                    report
                        .failed
                        .push((index.clone(), format!("unsupported ({code})")));
                    report.unsupported = Some((index.clone(), code));
                    for skipped in &indexes[at + 1..] {
                        report
                            .failed
                            .push((skipped.clone(), "not attempted".to_string()));
                    }
                    break;
                }
                report
                    .failed
                    .push((index.clone(), format!("refused ({e})")));
            }
        }
    }
    if posted.is_empty() {
        return report;
    }
    let (hit, attempts, last_err) = poll_with(
        reread,
        |present: &Vec<String>| !present.iter().any(|i| posted.contains(i)),
        limits,
        sleep,
    );
    report.rereads = attempts;
    if hit.is_some() {
        report.deleted += posted.len();
    } else {
        for index in &posted {
            let why = match &last_err {
                Some(e) => format!("still present ({e})"),
                None => "still present".to_string(),
            };
            report.failed.push((index.clone(), why));
        }
    }
    report
}

/// Mark one message read, then prove it: the reread must report it not unread.
/// The flag is a device fact, so this is the same poll the band write uses.
pub fn run_mark_read<D, R, S>(
    index: &str,
    mut post: D,
    reread: R,
    limits: VerifyLimits,
    sleep: S,
) -> SmsReport
where
    D: FnMut(&str) -> Result<(), ApiError>,
    R: FnMut() -> Result<bool, ApiError>,
    S: FnMut(Duration),
{
    let mut report = SmsReport::new(SmsOp::MarkRead, 1);
    if let Err(e) = post(index) {
        match unsupported_code(&e) {
            Some(code) => {
                report
                    .failed
                    .push((index.to_string(), format!("unsupported ({code})")));
                report.unsupported = Some((index.to_string(), code));
            }
            None => report
                .failed
                .push((index.to_string(), format!("refused ({e})"))),
        }
        return report;
    }
    let (hit, attempts, _) = poll_with(reread, |still_unread: &bool| !*still_unread, limits, sleep);
    report.rereads = attempts;
    match hit {
        Some(_) => report.deleted = 1,
        None => report.failed.push((
            index.to_string(),
            "still unread after the write".to_string(),
        )),
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> WriteIntent {
        WriteIntent::ApplyBands {
            mode: "03".into(),
            bands: vec![1, 3, 5],
            mask: "15".into(),
            option: None,
        }
    }

    #[test]
    fn state_machine_transitions() {
        let i = intent();
        let mut s = WriteState::Idle;
        assert!(!s.is_busy());

        s = WriteState::Confirming(i.clone());
        assert!(!s.is_busy() && !s.is_settled());

        s = WriteState::Pending {
            intent: i.clone(),
            since: Instant::now(),
        };
        assert!(s.is_busy());
        assert!(s.label().starts_with("applying"));

        s = WriteState::Verifying {
            intent: i.clone(),
            since: Instant::now(),
        };
        assert!(s.is_busy());
        assert!(s.label().contains("verifying"));

        s = WriteState::Success {
            intent: i.clone(),
            note: "verified".into(),
        };
        assert!(s.is_settled() && !s.is_busy());
        assert!(s.label().starts_with("applied"));

        s = WriteState::Failed {
            intent: i,
            err: "boom".into(),
        };
        assert!(s.is_settled());
        assert!(s.label().contains("failed"));
    }

    #[test]
    fn dns_validation_is_strict() {
        assert!(is_ipv4("1.1.1.1"));
        assert!(is_ipv4("255.255.255.255"));
        assert!(!is_ipv4("256.1.1.1"));
        assert!(!is_ipv4("1.1.1"));
        assert!(!is_ipv4("dns.google"));
        assert!(!is_ipv4(""));
        assert_eq!(
            parse_dns_pair("1.1.1.1, 1.0.0.1").unwrap(),
            ("1.1.1.1".to_string(), "1.0.0.1".to_string())
        );
        assert!(parse_dns_pair("1.1.1.1").is_err());
        assert!(parse_dns_pair("1.1.1.1,x").is_err());
        assert!(parse_dns_pair("").is_err());
    }

    #[test]
    fn dns_intent_labels() {
        let i = WriteIntent::SetDns {
            primary: "1.1.1.1".into(),
            secondary: "1.0.0.1".into(),
        };
        assert_eq!(i.label(), "DNS 1.1.1.1 / 1.0.0.1");
        assert_eq!(
            WriteIntent::ResetDns.label(),
            "DNS: router answers DNS itself"
        );
    }

    #[test]
    fn noop_guard_compares_mode_mask_and_option() {
        let current = NetMode {
            network_mode: Some("03".into()),
            network_band: Some("88080095".into()),
            lte_band: Some("88080095".into()),
            network_option: Some("0".into()),
        };
        let same_4g = bands::resolve(
            "03",
            &[1, 3, 5, 8, 20, 28, 32],
            Some("7fffffffffffffff"),
            None,
            None,
        );
        assert!(
            same_band_target(&current, &same_4g),
            "identical mode+mask is a no-op"
        );

        // same mask, different mode: this is exactly the 4G→auto case that was
        // silently skipped before
        let auto = bands::resolve(
            "00",
            &[1, 3, 5, 8, 20, 28, 32],
            Some("7fffffffffffffff"),
            None,
            None,
        );
        assert!(
            !same_band_target(&current, &auto),
            "mode change is not a no-op"
        );

        // same mode+mask but a different NR option
        let mut fiveg_now = current.clone();
        fiveg_now.network_mode = Some("08".into());
        fiveg_now.network_option = Some("0".into());
        let fiveg_sa = bands::resolve("08", &[1], None, None, Some("1"));
        assert!(!same_band_target(&fiveg_now, &fiveg_sa));
    }

    #[test]
    fn intent_label_lists_bands() {
        assert_eq!(intent().label(), "mode 03 · 3 bands (B1,B3,B5)");
    }

    /// The whole point of reading back: a refused POST must never be reported
    /// as applied, even when the mask happens to match.
    #[test]
    fn a_refused_write_is_never_reported_as_verified() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::H122373);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let payload = bands::resolve("00", &[1, 3, 7, 8, 20], None, Some("3FFFFFFF"), None);
        let state = apply_bands(&mut c, &payload, intent(), false, &mut |_| {});
        match state {
            WriteState::Failed { err, .. } => {
                assert!(err.contains("refused"), "a 404 POST is a refusal: {err}")
            }
            other => panic!("a refused write must fail, got {other:?}"),
        }
    }

    /// Same mask, different mode: comparing the mask alone called this applied.
    #[test]
    fn a_mode_only_change_is_not_reported_as_applied() {
        let current = NetMode {
            network_mode: Some("03".into()),
            network_band: Some("800C5".into()),
            lte_band: Some("800C5".into()),
            network_option: Some("0".into()),
        };
        let payload = bands::resolve("00", &[1, 3, 7, 8, 20], None, Some("3FFFFFFF"), None);
        assert!(
            !matches_after(&current, &payload),
            "the device still reports mode 03, so nothing was applied"
        );
    }

    #[test]
    fn a_matching_device_state_is_reported_as_applied() {
        let current = NetMode {
            network_mode: Some("00".into()),
            network_band: Some("3FFFFFFF".into()),
            lte_band: Some("800C5".into()),
            network_option: None,
        };
        let payload = bands::resolve("00", &[1, 3, 7, 8, 20], None, Some("3FFFFFFF"), None);
        assert!(matches_after(&current, &payload));
    }

    /// H122-373 accepts NetworkBand=3FFFFFFF in auto mode and then publishes
    /// 0 for that field. The write did land, so the verdict must be success.
    #[test]
    fn an_auto_write_that_reads_back_zero_is_a_success() {
        let current = NetMode {
            network_mode: Some("00".into()),
            network_band: Some("0".into()),
            lte_band: Some("800D5".into()),
            network_option: None,
        };
        let payload = bands::resolve("00", &[1, 3, 5, 7, 8, 20], None, Some("3FFFFFFF"), None);
        assert!(auto_reports_zero_band(&current, &payload));
        assert!(matches_after(&current, &payload));
    }

    /// The exception is narrow: a zero NetworkBand with a different LTE mask is
    /// still a device that did not land on what was asked.
    #[test]
    fn a_zero_band_with_the_wrong_lte_mask_is_still_a_failure() {
        let current = NetMode {
            network_mode: Some("00".into()),
            network_band: Some("0".into()),
            lte_band: Some("800C5".into()),
            network_option: None,
        };
        let payload = bands::resolve("00", &[1, 3, 5, 7, 8, 20], None, Some("3FFFFFFF"), None);
        assert!(!auto_reports_zero_band(&current, &payload));
        assert!(!matches_after(&current, &payload));
    }

    /// Outside auto mode a zero mask is never an acceptable answer.
    #[test]
    fn a_zero_band_outside_auto_mode_is_still_a_failure() {
        let current = NetMode {
            network_mode: Some("03".into()),
            network_band: Some("0".into()),
            lte_band: Some("1".into()),
            network_option: None,
        };
        let payload = bands::resolve("03", &[1], None, None, None);
        assert!(!auto_reports_zero_band(&current, &payload));
        assert!(!matches_after(&current, &payload));
    }

    /// A lost response is not a refusal; the message has to say which one it was.
    #[test]
    fn a_lost_response_is_named_as_such() {
        let lost = ApiError::Network("timed out".into());
        assert!(
            write_cause(&lost).contains("response lost"),
            "{}",
            write_cause(&lost)
        );
        let refused = ApiError::Http(404);
        assert!(
            write_cause(&refused).contains("refused"),
            "{}",
            write_cause(&refused)
        );
    }

    /// The case from the log: `POST net/net-mode` answers 100006 and the radio
    /// applied the payload anyway. The verdict is success and the message must
    /// not read like an error, because nothing is wrong.
    #[test]
    fn a_switch_that_answers_100006_and_lands_is_a_success() {
        for (mode, bands_sel, option) in [
            ("08", vec![1, 3, 5, 8, 20, 28], Some("0")),
            ("00", vec![1, 3, 5, 8, 20, 28], None),
            ("03", vec![1, 3, 5, 8, 20, 28], None),
        ] {
            let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::SwitchWarns);
            let mut c = HuaweiClient::new(mock.addr(), 2000);
            c.login("admin", "demo").expect("mock login");
            let payload = bands::resolve(mode, &bands_sel, None, Some("3FFFFFFF"), option);
            let state = apply_bands(&mut c, &payload, intent(), false, &mut |_| {});
            match state {
                WriteState::Success { note, .. } => {
                    assert!(note.contains("read-back confirms"), "mode {mode}: {note}");
                    assert!(
                        !note.contains("refused"),
                        "mode {mode} reads as a refusal: {note}"
                    );
                    assert!(
                        !note.contains("unknown router error"),
                        "mode {mode} leaks the raw error: {note}"
                    );
                }
                other => panic!("mode {mode} landed, so this is a success: {other:?}"),
            }
        }
    }

    /// Same code, but the new mask takes a moment to appear. The extra read
    /// inside the grace period turns this from a false failure into a success,
    /// and the write itself is still never repeated.
    #[test]
    fn a_late_switch_is_not_a_failure() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::SwitchWarnsLate);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let payload = bands::resolve("08", &[1, 3, 5, 8, 20, 28], None, None, Some("0"));
        let state = apply_bands(&mut c, &payload, intent(), false, &mut |_| {});
        match state {
            WriteState::Success { note, .. } => {
                assert!(note.contains("read-back confirms"), "{note}")
            }
            other => panic!("the device caught up, so this is a success: {other:?}"),
        }
    }

    /// The same code with no change behind it is still a failure, and it has to
    /// say what the device kept. Read-back is the truth in both directions.
    #[test]
    fn a_100006_that_never_lands_is_a_failure() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::SwitchRefuses);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let payload = bands::resolve("08", &[1, 3, 5, 8, 20, 28], None, None, Some("0"));
        let state = apply_bands(&mut c, &payload, intent(), false, &mut |_| {});
        match state {
            WriteState::Failed { err, .. } => {
                assert!(err.contains("100006"), "the code must be named: {err}");
                assert!(
                    err.contains("device still reports"),
                    "the message must say what the device kept: {err}"
                );
            }
            other => panic!("nothing landed, so this must fail: {other:?}"),
        }
    }

    fn dns_intent() -> WriteIntent {
        WriteIntent::SetDns {
            primary: "9.9.9.9".into(),
            secondary: "8.8.8.8".into(),
        }
    }

    /// A firmware that refuses only short bodies has to accept this one: the write
    /// carries the whole document, so nothing is refused and the read-back proves it.
    #[test]
    fn a_full_document_is_not_refused_by_a_short_body_only_firmware() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::DhcpIgnoresShort);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let state = apply_dns(
            &mut c,
            "9.9.9.9",
            "8.8.8.8",
            dns_intent(),
            true,
            &mut |_| {},
        );
        match state {
            WriteState::Success { note, .. } => {
                assert!(note.contains("9.9.9.9"), "{note}");
                assert!(!note.contains("refused"), "{note}");
            }
            other => panic!("the full document must land: {other:?}"),
        }
        assert_eq!(mock.dhcp_posts(), 1);
    }

    /// And when the read-back does not match, a refused POST is a failure with
    /// the real cause, not "unknown router error".
    #[test]
    fn a_refused_dns_post_with_a_stale_device_fails() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::H122373);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let state = apply_dns(
            &mut c,
            "9.9.9.9",
            "8.8.8.8",
            dns_intent(),
            true,
            &mut |_| {},
        );
        match state {
            WriteState::Failed { err, .. } => {
                assert!(err.contains("refused"), "{err}");
                assert!(!err.contains("verified"), "{err}");
            }
            other => panic!("a stale device after a refused POST must fail: {other:?}"),
        }
    }

    /// The whole point of the fix: the POST carries the DHCP document, so a firmware
    /// that validates field names accepts it and the read-back proves the change.
    #[test]
    fn a_dns_write_sends_the_whole_document_and_verifies() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::DhcpStrict);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let state = apply_dns(
            &mut c,
            "1.1.1.1",
            "1.0.0.1",
            dns_intent(),
            false,
            &mut |_| {},
        );
        match state {
            WriteState::Success { note, .. } => {
                assert!(note.contains("1.1.1.1"), "{note}");
                assert!(!note.contains("refused"), "{note}");
            }
            other => panic!("the full document must land: {other:?}"),
        }
        assert_eq!(mock.dhcp_posts(), 1);
    }

    /// A firmware that answers OK and stores nothing is a mismatch, and the message
    /// names both sides in plain text so a 66-column modal can show all of it.
    #[test]
    fn an_ignored_write_reports_both_sides_in_plain_words() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::DhcpIgnoresAll);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let state = apply_dns(
            &mut c,
            "1.1.1.1",
            "1.0.0.1",
            dns_intent(),
            false,
            &mut |_| {},
        );
        match state {
            WriteState::Failed { err, .. } => {
                assert!(err.contains("wanted 1.1.1.1 / 1.0.0.1"), "{err}");
                assert!(
                    err.contains("device keeps 192.168.8.1 / 192.168.8.1"),
                    "{err}"
                );
                assert!(
                    !err.contains("Some("),
                    "no debug quotes in a user message: {err}"
                );
            }
            other => panic!("an ignored write must fail: {other:?}"),
        }
    }

    /// Reset is not a mode bit either: it puts the router's own address in both
    /// slots, and it is idempotent, so a second reset sends nothing.
    #[test]
    fn reset_writes_the_lan_address_and_is_idempotent() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::DhcpStrict);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let set = apply_dns(
            &mut c,
            "1.1.1.1",
            "1.0.0.1",
            dns_intent(),
            false,
            &mut |_| {},
        );
        assert!(matches!(set, WriteState::Success { .. }), "{set:?}");
        let back = apply_dns(&mut c, "", "", WriteIntent::ResetDns, false, &mut |_| {});
        match back {
            WriteState::Success { note, .. } => {
                assert!(note.contains("192.168.8.1"), "{note}");
            }
            other => panic!("reset must land: {other:?}"),
        }
        let again = apply_dns(&mut c, "", "", WriteIntent::ResetDns, false, &mut |_| {});
        match again {
            WriteState::Success { note, .. } => assert!(note.contains("already active"), "{note}"),
            other => panic!("a second reset is a no-op: {other:?}"),
        }
        assert_eq!(mock.dhcp_posts(), 2);
    }

    /// No document means no write: sending a guess is how the bug happened.
    #[test]
    fn an_unreadable_document_stops_the_write_before_it_is_sent() {
        let doc = crate::model::DhcpSettings::default();
        assert!(plan_dns(&doc, true, "", "").is_err());
        let plan = plan_dns(&doc, false, "1.1.1.1", "1.0.0.1").expect("valid pair");
        assert_eq!(plan.primary, "1.1.1.1");
        assert_eq!(plan.secondary, "1.0.0.1");
        assert!(plan_dns(&doc, false, "nonsense", "").is_err());
    }

    /// The live H122-373 hides the DNS row: `dhcpsdns_display=0` means the firmware
    /// accepts the document and drops the three DNS fields, so a write there can
    /// never land. The refusal comes before the POST, so the device is not
    /// touched at all.
    #[test]
    fn a_firmware_without_the_dns_row_is_refused_before_the_post() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::DhcpNoDns);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let state = apply_dns(
            &mut c,
            "1.1.1.1",
            "1.0.0.1",
            dns_intent(),
            false,
            &mut |_| {},
        );
        match state {
            WriteState::Failed { err, .. } => {
                assert!(err.contains("dhcpsdns_display=0"), "{err}");
                assert!(err.contains("nothing was sent"), "{err}");
            }
            other => panic!("a firmware with no DNS row must be refused: {other:?}"),
        }
        assert_eq!(
            mock.dhcp_posts(),
            0,
            "the refusal must not touch the device"
        );
    }

    /// The refusal is about the firmware, not about the write: the same intent on a
    /// router that does expose the page still lands and is still verified.
    #[test]
    fn the_same_write_lands_on_a_firmware_that_has_the_dns_row() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::DhcpWithDns);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let state = apply_dns(
            &mut c,
            "1.1.1.1",
            "1.0.0.1",
            dns_intent(),
            false,
            &mut |_| {},
        );
        match state {
            WriteState::Success { note, .. } => assert!(note.contains("1.1.1.1"), "{note}"),
            other => panic!("a firmware with the DNS row must accept the write: {other:?}"),
        }
        assert_eq!(mock.dhcp_posts(), 1);
    }

    fn net_mode(mode: &str, band: &str, lte: &str, option: Option<&str>) -> NetMode {
        NetMode {
            network_mode: Some(mode.into()),
            network_band: Some(band.into()),
            lte_band: Some(lte.into()),
            network_option: option.map(Into::into),
        }
    }

    /// The payload the tests compare against. `0x8000001` sets bits 0 and 27, i.e.
    /// bands 1 and 28, so the mask literal in `net_mode(...)` must be this same value.
    fn payload_4g() -> BandPayload {
        bands::resolve("03", &[1, 28], None, Some("3FFFFFFF"), None)
    }

    #[test]
    fn the_first_read_wins_and_nothing_sleeps() {
        let mut reads = 0;
        let mut slept = Vec::new();
        let (hit, attempts, err) = poll_until(
            || {
                reads += 1;
                Ok(net_mode("03", "8000001", "8000001", None))
            },
            |m| matches_after(m, &payload_4g()),
            VerifyLimits::default(),
            |d| slept.push(d),
        );
        assert!(hit.is_some());
        assert_eq!(attempts, 1);
        assert!(slept.is_empty(), "{slept:?}");
        assert!(err.is_none());
    }

    #[test]
    fn a_late_mask_is_waited_for_and_the_gap_is_five_seconds() {
        let mut n = 0;
        let mut slept = Vec::new();
        let (hit, attempts, _) = poll_until(
            || {
                n += 1;
                if n < 4 {
                    Ok(net_mode("03", "1", "1", None))
                } else {
                    Ok(net_mode("03", "8000001", "8000001", None))
                }
            },
            |m| matches_after(m, &payload_4g()),
            VerifyLimits::default(),
            |d| slept.push(d),
        );
        assert!(hit.is_some());
        assert_eq!(attempts, 4);
        assert_eq!(slept, vec![Duration::from_secs(5); 3]);
    }

    #[test]
    fn the_window_closes_after_twenty_seconds() {
        let mut slept = Vec::new();
        let (hit, attempts, _) = poll_until(
            || Ok(net_mode("03", "1", "1", None)),
            |m| matches_after(m, &payload_4g()),
            VerifyLimits::default(),
            |d| slept.push(d),
        );
        assert!(hit.is_none());
        assert_eq!(attempts, 5);
        assert_eq!(slept.iter().sum::<Duration>(), Duration::from_secs(20));
    }

    #[test]
    fn a_lost_response_does_not_end_the_poll() {
        let mut n = 0;
        let (hit, attempts, _) = poll_until(
            || {
                n += 1;
                if n == 1 {
                    Err(ApiError::Network("timed out".into()))
                } else {
                    Ok(net_mode("03", "8000001", "8000001", None))
                }
            },
            |m| matches_after(m, &payload_4g()),
            VerifyLimits::default(),
            |_| {},
        );
        assert!(hit.is_some());
        assert_eq!(attempts, 2);
    }

    #[test]
    fn the_last_error_is_reported_when_the_window_closes() {
        let (hit, attempts, err) = poll_until(
            || Err(ApiError::Network("refused".into())),
            |_| false,
            VerifyLimits {
                reread: Duration::from_millis(1),
                window: Duration::ZERO,
            },
            |_| {},
        );
        assert!(hit.is_none());
        assert_eq!(attempts, 1);
        assert!(err.is_some());
    }

    #[test]
    fn the_window_is_clamped_between_five_and_one_hundred_twenty() {
        assert_eq!(VerifyLimits::from_secs(0).window, Duration::from_secs(5));
        assert_eq!(
            VerifyLimits::from_secs(600).window,
            Duration::from_secs(120)
        );
        assert_eq!(VerifyLimits::from_secs(30).window, Duration::from_secs(30));
        assert_eq!(VerifyLimits::from_secs(30).reread, VERIFY_REREAD);
    }

    #[test]
    fn the_generic_poll_accepts_a_type_that_is_not_a_band_mask() {
        let mut reads = 0;
        let (hit, attempts, err) = poll_with(
            || {
                reads += 1;
                Ok::<_, ApiError>(if reads < 3 {
                    None
                } else {
                    Some("203.0.113.9".to_string())
                })
            },
            |ip: &Option<String>| ip.is_some(),
            VerifyLimits::default(),
            |_| {},
        );
        assert_eq!(hit.flatten().as_deref(), Some("203.0.113.9"));
        assert_eq!(attempts, 3);
        assert!(err.is_none());
    }

    // A renew: the switch, the address and the status document come from one
    // state in the mock, so these run without a router and without a second.
    mod renew {
        use super::*;
        use crate::mock_cpe::{MockCpe, Profile};

        fn fast() -> VerifyLimits {
            VerifyLimits {
                reread: Duration::from_millis(1),
                window: Duration::from_secs(2),
            }
        }

        fn client(m: &MockCpe) -> HuaweiClient {
            let mut c = HuaweiClient::new(m.addr(), 5000);
            c.login("admin", "x").expect("login");
            c
        }

        #[test]
        fn a_renew_that_gets_a_new_lease_says_before_and_after() {
            let m = MockCpe::start(Profile::RenewChanges);
            let mut c = client(&m);
            let out = renew_ip(&mut c, fast(), |_| {});
            match &out {
                RenewOutcome::Changed { before, after, .. } => {
                    assert_eq!(before, "203.0.113.77");
                    assert_eq!(after, "203.0.113.99");
                }
                other => panic!("expected a change, got {other:?}"),
            }
            assert_eq!(out.exit_code(), 0);
            assert_eq!(m.link_downs(), 1, "an up link is dropped exactly once");
        }

        #[test]
        fn the_same_lease_back_is_unchanged_and_still_exit_zero() {
            let m = MockCpe::start(Profile::RenewKeeps);
            let mut c = client(&m);
            let out = renew_ip(&mut c, fast(), |_| {});
            match &out {
                RenewOutcome::Unchanged { ip, .. } => assert_eq!(ip, "203.0.113.77"),
                other => panic!("expected unchanged, got {other:?}"),
            }
            assert_eq!(
                out.exit_code(),
                0,
                "the ISP keeping the lease is not a failure"
            );
        }

        #[test]
        fn a_link_that_stays_down_is_a_failure_with_the_last_attempt_named() {
            let m = MockCpe::start(Profile::RenewRefuses);
            let mut c = client(&m);
            let mut slept = Vec::new();
            let out = renew_ip(&mut c, fast(), |d| slept.push(d));
            match &out {
                RenewOutcome::LinkDown { why } => {
                    assert!(why.contains("attempt 3"), "{why}");
                }
                other => panic!("expected a dead link, got {other:?}"),
            }
            assert_eq!(out.exit_code(), 1);
            // One wait after dropping the link, then two between the three enable
            // attempts: the injected sleep proves the retry without costing nine
            // real seconds, and there is no wait after the last attempt.
            assert_eq!(
                slept,
                vec![
                    Duration::from_secs(3),
                    Duration::from_secs(3),
                    Duration::from_secs(3)
                ]
            );
            assert_eq!(m.link_downs(), 1);
        }

        #[test]
        fn a_link_already_down_is_switched_on_and_never_off_first() {
            let m = MockCpe::start(Profile::RenewStartsDown);
            let mut c = client(&m);
            let out = renew_ip(&mut c, fast(), |_| {});
            assert_eq!(
                m.link_downs(),
                0,
                "switching an off link off is the no-op this path exists to avoid"
            );
            assert_eq!(out.exit_code(), 0);
        }
    }
}
