//! What the client list actually reports.
//!
//! The client tier reads `wlan/host-list` every 30 s and carries six fields:
//! name, IP, MAC, frequency band, SSID and the seconds the host has been
//! associated. It carries no per-client traffic and no per-client signal, and
//! this firmware serves no per-client detail path at all (`PROBE` has none, and
//! the WebUI's host list is one document).
//!
//! So an inspector built on this data must be honest about its columns: a value
//! the firmware did not send is printed as `not reported`, never as `0`, `-1` or
//! a guess. The listed absences are the columns a reader would expect and will
//! not find.

use crate::model::Client;

/// Columns a client inspector would want and this firmware does not send. Each
/// one is named rather than silently missing, so a blank cell cannot be read as
/// "zero traffic".
pub const NOT_REPORTED: &[&str] = &[
    "per-client traffic",
    "per-client signal",
    "per-client uptime",
];

/// The reported fields of one client, in reading order. Every field of `Client`
/// appears exactly once; `every_field_of_a_client_is_reported` holds that.
pub fn rows(c: &Client) -> Vec<(&'static str, String)> {
    vec![
        ("name", word(&c.name)),
        ("ip", word(&c.ip)),
        ("mac", word(&c.mac)),
        ("band", word(&c.freq)),
        ("ssid", word(&c.ssid)),
        ("connected", crate::ui::widgets::dur(c.associated_secs)),
    ]
}

/// An absent value says so. An empty string would read as a value that is empty,
/// which is a different claim about the network.
pub fn word(v: &Option<String>) -> String {
    match v {
        Some(s) if !s.trim().is_empty() => s.clone(),
        _ => "not reported".to_string(),
    }
}

/// How old the client list is, in words the header already uses.
pub fn age_word(age: std::time::Duration) -> String {
    let secs = age.as_secs();
    if secs < 10 {
        format!("{:.1}s", age.as_secs_f32())
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_of_a_client_is_reported() {
        let c = Client {
            name: Some("phone".into()),
            ip: Some("192.168.8.20".into()),
            mac: Some("02:00:00:00:00:01".into()),
            freq: Some("5GHz".into()),
            ssid: Some("Test Net".into()),
            associated_secs: 90,
        };
        let labels: Vec<&str> = rows(&c).into_iter().map(|(l, _)| l).collect();
        assert_eq!(
            labels,
            vec!["name", "ip", "mac", "band", "ssid", "connected"]
        );
        assert!(rows(&c).iter().all(|(_, v)| !v.is_empty()));
    }

    #[test]
    fn an_absent_value_says_not_reported() {
        let c = Client::default();
        for (label, value) in rows(&c) {
            if label == "connected" {
                assert_eq!(value, "0s");
                continue;
            }
            assert_eq!(value, "not reported", "{label} invented a value");
        }
    }

    #[test]
    fn a_blank_value_is_not_a_value() {
        assert_eq!(word(&Some("   ".into())), "not reported");
        assert_eq!(word(&None), "not reported");
        assert_eq!(word(&Some("5GHz".into())), "5GHz");
    }

    #[test]
    fn the_unreported_columns_are_named() {
        assert_eq!(NOT_REPORTED.len(), 3);
        let labels: Vec<&str> = rows(&Client::default())
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        for absent in NOT_REPORTED {
            assert!(!labels.contains(absent), "{absent} must not look reported");
        }
    }
}
