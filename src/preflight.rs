//! Both sides of a pending write, computed before the user confirms.
//!
//! Pure: no I/O and no clock, so the confirm modal can show what will change
//! and what will drop out without touching the router again.

use crate::model::Snapshot;
use crate::router::bands;
use crate::write::WriteIntent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preflight {
    pub rows: Vec<(String, String)>,
    pub warning: Option<String>,
}

/// `B1,B3,B5` or `none` - the only shape the router speaks for a mask.
pub fn bands_list(bands: &[u8]) -> String {
    if bands.is_empty() {
        "none".to_string()
    } else {
        bands
            .iter()
            .map(|b| format!("B{b}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The LTE bands the router reports right now. Empty when the firmware never
/// answered `net/net-mode` - never guessed from the checkboxes.
pub fn current_bands(snap: Option<&Snapshot>) -> Vec<u8> {
    snap.and_then(|s| s.net_mode.lte_band.as_deref())
        .and_then(bands::from_hex)
        .map(bands::bands_from_mask)
        .unwrap_or_default()
}

pub fn for_intent(intent: &WriteIntent, snap: Option<&Snapshot>) -> Preflight {
    match intent {
        WriteIntent::ApplyBands {
            bands: target,
            mode,
            ..
        } => {
            let mut rows = vec![
                ("bands now".to_string(), bands_list(&current_bands(snap))),
                ("bands after".to_string(), bands_list(target)),
                ("mode".to_string(), mode.clone()),
            ];
            if let Some(s) = snap {
                rows.push((
                    "signal now".to_string(),
                    match s.signal.rsrp_dbm {
                        Some(rsrp) => format!("{rsrp:.0} dBm · {}", s.connection.net_type),
                        None => format!("not reported · {}", s.connection.net_type),
                    },
                ));
                rows.push((
                    "clients".to_string(),
                    format!("{} associated", s.clients.len()),
                ));
            }
            Preflight {
                rows,
                warning: Some("applying bands restarts the radio: 10–30 s offline".to_string()),
            }
        }
        WriteIntent::SetDns { primary, secondary } => {
            let mut rows = vec![
                ("DNS now".to_string(), dns_now(snap)),
                ("DNS after".to_string(), format!("{primary} / {secondary}")),
            ];
            push_dns_support(&mut rows, snap);
            Preflight {
                rows,
                warning: Some(
                    "DNS does not restart the radio; the next poll verifies it".to_string(),
                ),
            }
        }
        WriteIntent::ResetDns => {
            let mut rows = vec![
                ("DNS now".to_string(), dns_now(snap)),
                (
                    "DNS after".to_string(),
                    snap.and_then(|s| s.dns.as_ref())
                        .and_then(|d| d.lan_ip.clone())
                        .map(|lan| format!("router as resolver ({lan})"))
                        .unwrap_or_else(|| "router as resolver".to_string()),
                ),
            ];
            push_dns_support(&mut rows, snap);
            Preflight {
                rows,
                warning: Some("DNS does not restart the radio".to_string()),
            }
        }
    }
}

/// The confirm modal must not look like a write that can work when the firmware
/// says it has no such setting: the router would answer OK and drop the DNS fields.
fn push_dns_support(rows: &mut Vec<(String, String)>, snap: Option<&Snapshot>) {
    let hidden = snap.and_then(|s| s.dns.as_ref()).and_then(|d| d.editable) == Some(false);
    if hidden {
        rows.push((
            "support".to_string(),
            "not on this firmware (dhcpsdns_display=0)".to_string(),
        ));
    }
}

fn dns_now(snap: Option<&Snapshot>) -> String {
    match snap.and_then(|s| s.dns.as_ref()) {
        Some(d) if !d.is_manual() => match &d.lan_ip {
            Some(lan) => format!("router as resolver ({lan})"),
            None => "router as resolver".to_string(),
        },
        Some(d) => format!(
            "{} / {}",
            d.primary.clone().unwrap_or_else(|| "?".into()),
            d.secondary.clone().unwrap_or_else(|| "?".into())
        ),
        None => "not reported".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> WriteIntent {
        WriteIntent::ApplyBands {
            mode: "00".into(),
            bands: vec![1, 3, 5],
            mask: "0".into(),
            option: None,
        }
    }

    #[test]
    fn the_preflight_names_both_sides_of_a_band_change() {
        let p = for_intent(&intent(), None);
        assert!(p
            .rows
            .iter()
            .any(|(k, v)| k == "bands after" && v == "B1,B3,B5"));
        assert!(p.rows.iter().any(|(k, _)| k == "bands now"));
    }

    #[test]
    fn the_preflight_never_hides_the_radio_restart() {
        let p = for_intent(&intent(), None);
        assert!(p.warning.unwrap_or_default().contains("offline"));
    }

    #[test]
    fn the_preflight_names_the_router_as_resolver_instead_of_automatic() {
        let snap = Snapshot {
            dns: Some(crate::model::DnsInfo {
                status: Some("1".into()),
                primary: Some("192.168.8.1".into()),
                secondary: Some("192.168.8.1".into()),
                lan_ip: Some("192.168.8.1".into()),
                editable: Some(false),
            }),
            quota: None,
            ..Snapshot::default()
        };
        let p = for_intent(
            &WriteIntent::SetDns {
                primary: "1.1.1.1".into(),
                secondary: "1.0.0.1".into(),
            },
            Some(&snap),
        );
        assert!(p
            .rows
            .iter()
            .any(|(k, v)| k == "DNS now" && v == "router as resolver (192.168.8.1)"));
        assert!(p
            .rows
            .iter()
            .any(|(k, v)| k == "DNS after" && v == "1.1.1.1 / 1.0.0.1"));

        let reset = for_intent(&WriteIntent::ResetDns, Some(&snap));
        assert!(reset
            .rows
            .iter()
            .any(|(k, v)| k == "DNS after" && v == "router as resolver (192.168.8.1)"));
        assert!(!reset.rows.iter().any(|(_, v)| v.contains("automatic")));
    }

    /// A confirm modal that lists the change without saying the firmware has no such
    /// setting asks the user to approve a write the router will silently drop.
    #[test]
    fn the_confirm_names_a_firmware_without_the_dns_row() {
        let dns = |editable: Option<bool>| Snapshot {
            dns: Some(crate::model::DnsInfo {
                status: Some("1".into()),
                primary: Some("192.168.8.1".into()),
                secondary: Some("192.168.8.1".into()),
                lan_ip: Some("192.168.8.1".into()),
                editable,
            }),
            quota: None,
            ..Snapshot::default()
        };
        let set = WriteIntent::SetDns {
            primary: "1.1.1.1".into(),
            secondary: "1.0.0.1".into(),
        };
        let hidden = dns(Some(false));
        for intent in [set.clone(), WriteIntent::ResetDns] {
            let p = for_intent(&intent, Some(&hidden));
            assert!(
                p.rows
                    .iter()
                    .any(|(k, v)| k == "support" && v.contains("dhcpsdns_display=0")),
                "the hidden row must be named: {intent:?} {:?}",
                p.rows
            );
        }
        let shown = dns(Some(true));
        assert!(!for_intent(&set, Some(&shown))
            .rows
            .iter()
            .any(|(k, _)| k == "support"));
        let unknown = dns(None);
        assert!(!for_intent(&set, Some(&unknown))
            .rows
            .iter()
            .any(|(k, _)| k == "support"));
        assert!(!for_intent(&set, None)
            .rows
            .iter()
            .any(|(k, _)| k == "support"));
    }
}
