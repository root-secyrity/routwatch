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
            WriteIntent::ResetDns => "DNS: back to automatic".to_string(),
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

    // Guard 2: verify by reading back. This is also the "response lost" path  -
    // we never re-send, we only look at what the device actually reports.
    let verification = read_mask(client);
    let got = match &verification {
        Ok((_, Some(m))) => bands::from_hex(m).unwrap_or(0),
        _ => 0,
    };

    match (sent, got == target) {
        (_, true) => WriteState::Success {
            intent,
            note: format!("verified, device mask 0x{}", bands::to_hex(target)),
        },
        (Err(e), false) => WriteState::Failed {
            intent,
            err: match &verification {
                Err(ve) => format!("{e}; read-back failed too: {ve}"),
                Ok(_) => format!("{e}; device still reports a different mask"),
            },
        },
        (Ok(_), false) => {
            let detail = match verification {
                Ok((_, Some(m))) => format!("wanted 0x{} got 0x{m}", bands::to_hex(target)),
                _ => "wanted a change, read-back gave nothing".to_string(),
            };
            WriteState::Failed {
                intent,
                err: format!("mismatch after write: {detail}"),
            }
        }
    }
}

/// DNS write: verify by reading `dhcp/settings` back.
pub fn apply_dns(
    client: &mut HuaweiClient,
    primary: &str,
    secondary: &str,
    intent: WriteIntent,
    force: bool,
    progress: &mut dyn FnMut(WriteState),
) -> WriteState {
    let target_manual = !matches!(intent, WriteIntent::ResetDns);
    if target_manual {
        if let Err(e) = parse_dns_pair(&format!("{primary},{secondary}")) {
            return WriteState::Failed { intent, err: e };
        }
    }

    let read = |c: &mut HuaweiClient| -> Option<crate::model::DnsInfo> {
        c.get("dhcp/settings")
            .ok()
            .map(|n| crate::router::parse::parse_dns(&n))
    };

    if let Some(cur) = read(client) {
        let matches_target = if target_manual {
            cur.is_manual()
                && cur.primary.as_deref() == Some(primary)
                && cur.secondary.as_deref() == Some(secondary)
        } else {
            !cur.is_manual()
        };
        if matches_target && !force {
            return WriteState::Success {
                intent,
                note: "already active - nothing sent".into(),
            };
        }
    }

    progress(WriteState::Pending {
        intent: intent.clone(),
        since: Instant::now(),
    });
    let (status, p, s2) = if target_manual {
        ("0", primary, secondary)
    } else {
        ("1", "", "")
    };
    let sent = client.set_dns(status, p, s2);
    progress(WriteState::Verifying {
        intent: intent.clone(),
        since: Instant::now(),
    });

    let after = read(client);
    let verified = after.as_ref().map(|d| {
        if target_manual {
            d.is_manual()
                && d.primary.as_deref() == Some(primary)
                && d.secondary.as_deref() == Some(secondary)
        } else {
            !d.is_manual()
        }
    });

    match (sent, verified) {
        (_, Some(true)) => WriteState::Success {
            intent,
            note: if target_manual {
                format!("verified, device reports {primary} / {secondary}")
            } else {
                "verified, device is back to automatic DNS".to_string()
            },
        },
        (Err(e), _) => WriteState::Failed {
            intent,
            err: format!("{e}; read-back did not confirm"),
        },
        (Ok(_), Some(false)) => {
            let detail = after
                .map(|d| format!("device reports {:?} / {:?}", d.primary, d.secondary))
                .unwrap_or_else(|| "read-back empty".into());
            WriteState::Failed {
                intent,
                err: format!("mismatch after write: {detail}"),
            }
        }
        (Ok(_), None) => WriteState::Failed {
            intent,
            err: "write sent, but dhcp/settings could not be read back".into(),
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
        assert_eq!(WriteIntent::ResetDns.label(), "DNS: back to automatic");
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
        );
        assert!(
            !same_band_target(&current, &auto),
            "mode change is not a no-op"
        );

        // same mode+mask but a different NR option
        let mut fiveg_now = current.clone();
        fiveg_now.network_mode = Some("08".into());
        fiveg_now.network_option = Some("0".into());
        let fiveg_sa = bands::resolve("08", &[1], None, Some("1"));
        assert!(!same_band_target(&fiveg_now, &fiveg_sa));
    }

    #[test]
    fn intent_label_lists_bands() {
        assert_eq!(intent().label(), "mode 03 · 3 bands (B1,B3,B5)");
    }
}
