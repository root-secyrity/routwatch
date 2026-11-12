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
use std::time::Instant;

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
/// Huawei's public WebAPI list defines 100002 (busy), 100003 (unsupported) and
/// 100005 (format), and leaves 100006 undefined; on the H122-373 it arrives on
/// `POST net/net-mode` after the radio accepted the payload. So it is a note,
/// never a verdict: only the read-back decides. Three device runs in this repo's
/// log agree, and every one of them applied.
const SWITCH_WARNING: u32 = 100006;

/// How long the radio gets, after a 100006, before the read-back is believed.
/// A band write restarts the modem; the new mask is not always visible on the
/// very next GET.
const SWITCH_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

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

/// Apply an intent, verifying against the device. Returns the settled state  -
/// never an intermediate one.
pub fn apply_bands(
    client: &mut HuaweiClient,
    payload: &BandPayload,
    intent: WriteIntent,
    force: bool,
    progress: &mut dyn FnMut(WriteState),
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

    // Guard 2: the device is the truth. Never re-send: read back and compare
    // mode, both masks and the NR option with what was asked for.
    let mut after = read_mode(client);

    // One extra read, and only for this case: the router said 100006 and the
    // first read-back still shows the old state. The radio is mid-restart, so
    // give it the grace period instead of calling a landing switch a failure.
    // The write itself is never repeated.
    let mismatched = matches!(&after, Ok(cur) if !matches_after(cur, payload));
    if mismatched {
        if let Err(e) = &sent {
            if switch_warning(e).is_some() {
                std::thread::sleep(SWITCH_GRACE);
                after = read_mode(client);
            }
        }
    }

    if let Ok(current) = &after {
        if matches_after(current, payload) {
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
    }

    let err = match (&sent, &after) {
        (Err(e), Err(ve)) => format!("{}; read-back failed too: {ve}", write_cause(e)),
        (Err(e), Ok(current)) => {
            let kept = format!(
                "device still reports mode {} mask 0x{}",
                current.network_mode.clone().unwrap_or_else(|| "-".into()),
                current
                    .lte_band
                    .as_deref()
                    .and_then(bands::from_hex)
                    .map(bands::to_hex)
                    .unwrap_or_else(|| "-".into())
            );
            match switch_warning(e) {
                Some(code) => format!(
                    "router answered {code} and the change did not land within {}s: {kept}",
                    SWITCH_GRACE.as_secs_f32()
                ),
                None => format!("{}; {kept}", write_cause(e)),
            }
        }
        (Ok(_), Ok(current)) => format!(
            "mismatch after write: wanted mode {} mask 0x{}, device reports mode {} mask 0x{}",
            payload.network_mode,
            payload.network_band,
            current.network_mode.clone().unwrap_or_else(|| "-".into()),
            current.network_band.clone().unwrap_or_else(|| "-".into())
        ),
        (Ok(_), Err(ve)) => format!("write sent, but reading net/net-mode failed: {ve}"),
    };
    WriteState::Failed { intent, err }
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
}
