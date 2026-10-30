//! A Markdown report that can be attached to a forum post or a vendor ticket.
//!
//! Two explicit modes, no default: `redacted` (identity hidden) and `full`.
//! There is deliberately no `impl Default for Redaction` - a call site has to
//! say which one it wants, so identity can never be exported by omission.

use crate::model::Snapshot;
use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Redaction {
    pub identity: bool,
}

impl Redaction {
    /// The mode a keyboard shortcut produces without asking.
    pub fn redacted() -> Self {
        Self { identity: false }
    }

    /// Everything, for local archiving or a vendor ticket.
    pub fn full() -> Self {
        Self { identity: true }
    }
}

/// `~/.local/share/routwatch/reports/` (XDG data dir when it exists).
pub fn dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("reports"))
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}

/// A table cell: control characters gone, `|` escaped so a value cannot open a
/// new column.
fn cell(text: &str) -> String {
    clean(text).replace('|', "\\|")
}

/// An identity field: shown only in `full`, hidden otherwise.
fn identity_field(red: Redaction, value: Option<&str>) -> String {
    if red.identity {
        opt(value)
    } else {
        "(hidden)".to_string()
    }
}

fn opt(value: Option<&str>) -> String {
    match value {
        Some(v) if !v.is_empty() => cell(v),
        _ => "(none reported)".to_string(),
    }
}

fn row(out: &mut String, key: &str, value: String) {
    use std::fmt::Write as _;
    let _ = writeln!(out, "| {key} | {} |", cell(&value));
}

/// A present identity field becomes `(hidden)`, an absent one stays absent:
/// "this firmware did not report a serial" is a fact worth keeping.
fn hidden(value: &Option<String>) -> Option<String> {
    match value {
        Some(v) if !v.is_empty() => Some("(hidden)".to_string()),
        _ => None,
    }
}

/// The shareable half of a snapshot.
///
/// Someone who owns a cellular router and wants it supported has nothing to
/// hide behind: they must paste what their firmware reported. This keeps every
/// field a maintainer needs to write a parser - model, firmware, signal shape,
/// band list, which endpoints answered - and replaces the six fields that
/// identify the person, the SIM or their network. No screenshots, no raw XML,
/// no IMEI.
pub fn redacted(snap: &Snapshot) -> Snapshot {
    let mut out = snap.clone();
    out.device.serial = hidden(&out.device.serial);
    out.device.imei = hidden(&out.device.imei);
    out.device.wan_ip = hidden(&out.device.wan_ip);
    out.device.wan_ipv6 = hidden(&out.device.wan_ipv6);
    out.device.mac = hidden(&out.device.mac);
    for (i, client) in out.clients.iter_mut().enumerate() {
        client.name = if client.name.is_some() {
            Some(format!("(client {})", i + 1))
        } else {
            None
        };
        client.mac = hidden(&client.mac);
        client.ssid = hidden(&client.ssid);
    }
    out
}

pub fn markdown(snap: &Snapshot, when: u64, red: Redaction) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("# routwatch report\n\n");
    let _ = writeln!(
        out,
        "{} · {} · firmware {}",
        cell(&snap.plmn.name),
        cell(&snap.connection.net_type),
        opt(snap.device.software.as_deref())
    );
    let _ = writeln!(out, "\n- generated: {when} (unix seconds)");
    let _ = writeln!(out, "- router: {}", cell(&snap.router));
    let _ = writeln!(
        out,
        "- identity fields: {}",
        if red.identity { "included" } else { "hidden" }
    );

    out.push_str("\n## Signal\n\n| field | value |\n|---|---|\n");
    row(
        &mut out,
        "RSRP",
        snap.signal
            .rsrp_dbm
            .map(|v| format!("{v:.0} dBm"))
            .unwrap_or_else(|| "not reported".into()),
    );
    row(
        &mut out,
        "RSRQ",
        snap.signal
            .rsrq_db
            .map(|v| format!("{v:.1} dB"))
            .unwrap_or_else(|| "not reported".into()),
    );
    row(
        &mut out,
        "SINR",
        snap.signal
            .sinr_db
            .map(|v| format!("{v:.1} dB"))
            .unwrap_or_else(|| "not reported".into()),
    );
    row(&mut out, "band", opt(snap.signal.band.as_deref()));
    row(
        &mut out,
        "network",
        format!(
            "{} · ENDC {}",
            snap.connection.net_type,
            if snap.connection.endc { "on" } else { "off" }
        ),
    );
    if let Some(nr) = &snap.signal.nr {
        row(&mut out, "NR bars", snap.connection.nr_bars.to_string());
        row(&mut out, "NR band", opt(nr.band.as_deref()));
    }

    out.push_str("\n## Device\n\n| field | value |\n|---|---|\n");
    row(&mut out, "model", opt(snap.device.name.as_deref()));
    row(&mut out, "hardware", opt(snap.device.hardware.as_deref()));
    row(&mut out, "firmware", opt(snap.device.software.as_deref()));
    row(
        &mut out,
        "serial",
        identity_field(red, snap.device.serial.as_deref()),
    );
    row(
        &mut out,
        "IMEI",
        identity_field(red, snap.device.imei.as_deref()),
    );
    row(
        &mut out,
        "WAN IP",
        identity_field(red, snap.device.wan_ip.as_deref()),
    );

    out.push_str("\n## Traffic and clients\n\n");
    let _ = writeln!(
        out,
        "- session: {} B down / {} B up",
        snap.traffic.session_dl_bytes, snap.traffic.session_ul_bytes
    );
    let _ = writeln!(out, "- clients: {}", snap.clients.len());
    if red.identity && !snap.clients.is_empty() {
        out.push_str("\n| name | ip | band |\n|---|---|---|\n");
        for c in &snap.clients {
            let _ = writeln!(
                out,
                "| {} | {} | {} |",
                opt(c.name.as_deref()),
                opt(c.ip.as_deref()),
                opt(c.freq.as_deref())
            );
        }
    }

    out.push_str("\n## Raw snapshot\n\n```json\n");
    let mut value = serde_json::to_value(snap).unwrap_or(serde_json::Value::Null);
    if !red.identity {
        for path in [
            "device.serial",
            "device.imei",
            "device.mac",
            "device.wan_ip",
            "device.wan_ipv6",
        ] {
            scrub(&mut value, path);
        }
        if let Some(list) = value.get_mut("clients").and_then(|c| c.as_array_mut()) {
            for client in list {
                for key in ["name", "mac", "ssid", "ip"] {
                    scrub(client, key);
                }
            }
        }
    }
    let _ = writeln!(
        out,
        "{}",
        clean(&serde_json::to_string_pretty(&value).unwrap_or_default())
    );
    out.push_str("```\n");
    out
}

/// Set one dotted path to null; a missing path is not an error.
fn scrub(value: &mut serde_json::Value, path: &str) {
    let mut cur = value;
    let mut parts = path.split('.').peekable();
    while let Some(key) = parts.next() {
        let Some(next) = cur.get_mut(key) else { return };
        if parts.peek().is_none() {
            *next = serde_json::Value::Null;
            return;
        }
        cur = next;
    }
}

pub fn write(snap: &Snapshot, dir: &Path, when: u64, red: Redaction) -> io::Result<PathBuf> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let body = markdown(snap, when, red);
    for attempt in 0..100u32 {
        let name = if attempt == 0 {
            format!("routwatch-{when}.md")
        } else {
            format!("routwatch-{when}-{attempt}.md")
        };
        let path = dir.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(body.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many reports in one second",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Client, Device, DnsInfo, Signal, Snapshot};

    fn sample() -> Snapshot {
        let mut s = Snapshot {
            router: "192.168.8.1".into(),
            signal: Signal {
                rsrp_dbm: Some(-96.0),
                sinr_db: Some(12.0),
                band: Some("B3".into()),
                ..Default::default()
            },
            device: Device {
                serial: Some("SER123".into()),
                imei: Some("867530912345678".into()),
                wan_ip: Some("203.0.113.9".into()),
                ..Default::default()
            },
            clients: vec![Client {
                name: Some("laptop".into()),
                mac: Some("AA:BB:CC:DD:EE:FF".into()),
                ssid: Some("HomeNet".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        s.dns = Some(DnsInfo {
            status: Some("1".into()),
            primary: None,
            secondary: None,
        });
        s
    }

    /// The point of `snapshot --json --redact`: a person who owns a router nobody
    /// has ever tested can paste the whole thing in a public issue. Every field a
    /// maintainer needs to write a parser survives; the six that identify the
    /// person, the SIM or the network do not.
    #[test]
    fn the_shareable_snapshot_keeps_the_shape_and_drops_the_identity() {
        let shared = redacted(&sample());
        let json = serde_json::to_string(&shared).expect("serialize");

        for secret in [
            "SER123",
            "867530912345678",
            "203.0.113.9",
            "AA:BB:CC:DD:EE:FF",
            "HomeNet",
            "laptop",
        ] {
            assert!(
                !json.contains(secret),
                "{secret} leaked into the shareable JSON"
            );
        }
        assert!(
            json.contains("(hidden)"),
            "nothing was hidden at all: {json}"
        );
        assert!(
            json.contains("\"rsrp_dbm\":-96.0") && json.contains("\"band\":\"B3\""),
            "the signal shape was lost: {json}"
        );
        assert_eq!(
            shared.clients.len(),
            1,
            "the client count is a capability, not an identity"
        );
        assert_eq!(shared.clients[0].name.as_deref(), Some("(client 1)"));
        assert!(
            shared.device.serial.is_some(),
            "a present field stays present"
        );
    }

    /// Absent identity stays absent: "this firmware reports no serial" is a fact.
    #[test]
    fn redaction_does_not_invent_fields_the_router_never_sent() {
        let bare = redacted(&Snapshot::default());
        assert!(bare.device.serial.is_none());
        assert!(bare.device.imei.is_none());
        assert!(bare.clients.is_empty());
    }

    /// Without the flag nothing changes - the local view still shows what the
    /// router said, otherwise the two paths would drift.
    #[test]
    fn an_unredacted_snapshot_still_carries_what_the_router_said() {
        let json = serde_json::to_string(&sample()).expect("serialize");
        assert!(json.contains("SER123"));
        assert!(json.contains("867530912345678"));
    }

    #[test]
    fn the_report_hides_identity_but_keeps_what_a_helper_needs() {
        let text = markdown(&sample(), 1_700_000_000, Redaction::redacted());
        for secret in [
            "SER123",
            "867530912345678",
            "203.0.113.9",
            "AA:BB:CC:DD:EE:FF",
            "HomeNet",
            "laptop",
        ] {
            assert!(!text.contains(secret), "leaked {secret}");
        }
        for useful in ["-96", "B3", "192.168.8.1", "12"] {
            assert!(text.contains(useful), "missing {useful}");
        }
    }

    #[test]
    fn the_report_keeps_identity_when_asked() {
        let text = markdown(&sample(), 1_700_000_000, Redaction::full());
        assert!(text.contains("867530912345678"));
    }

    #[test]
    fn the_report_survives_pipes_and_control_characters() {
        let mut s = sample();
        s.plmn.name = "A|B\u{7}C".into();
        let text = markdown(&s, 1, Redaction::redacted());
        assert!(
            text.contains("A\\|B"),
            "the data pipe was not escaped in a table cell: {text}"
        );
        assert!(
            text.contains("| field | value |"),
            "the table structure was damaged: {text}"
        );
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "control character survived"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_report_file_is_private_and_never_overwrites() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("routwatch-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let first = write(&sample(), &base, 42, Redaction::redacted()).expect("first write");
        let second = write(&sample(), &base, 42, Redaction::redacted()).expect("second write");
        assert_ne!(first, second, "the second report overwrote the first");
        let mode = std::fs::metadata(&first).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "report file is not private");
        let dir_mode = std::fs::metadata(&base).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "report directory is not private");
        let _ = std::fs::remove_dir_all(&base);
    }
}
