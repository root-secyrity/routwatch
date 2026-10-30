//! One document a stranger can paste in a public issue when they want their
//! router supported.
//!
//! Three things are needed to write a parser for a firmware nobody here has:
//! which endpoints it answers, which fields those answers contain, and what the
//! values look like. This collects all three, and hides the values that would
//! identify the person who sent it - the field *names* are the useful part.

use crate::headless::{Capability, DoctorReport};
use crate::model::Snapshot;
use crate::report;

/// One probed endpoint, as it will be printed: the path, whether the firmware
/// answered, and the field names it used (never their values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointLine {
    pub path: String,
    pub state: String,
    pub fields: Vec<String>,
}

fn row(out: &mut String, cells: &[&str]) {
    use std::fmt::Write as _;
    out.push('|');
    for cell in cells {
        let _ = write!(out, " {} |", cell.replace('|', "\\|"));
    }
    out.push('\n');
}

/// Render the shareable document. The snapshot is redacted here rather than by
/// the caller, so no call site can forget and ship an IMEI.
pub fn render(
    version: &str,
    router: &str,
    when: u64,
    report: &DoctorReport,
    snapshot: &Snapshot,
    endpoints: &[EndpointLine],
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# routwatch share");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Written by `routwatch {version}` for a router that is not supported yet."
    );
    let _ = writeln!(
        out,
        "Serial, IMEI, MACs, WAN IP and client names are `(hidden)`."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "- router: `{router}`");
    let _ = writeln!(out, "- generated_at: {when}");
    let _ = writeln!(out, "- platform: {}", std::env::consts::OS);
    let _ = writeln!(out);

    let _ = writeln!(out, "## What this firmware answers\n");
    row(&mut out, &["capability", "state", "reason"]);
    row(&mut out, &["---", "---", "---"]);
    for cap in &report.capabilities {
        row(&mut out, &[&cap.id, &cap.state, &cap.reason]);
    }
    let refused: Vec<&Capability> = report
        .capabilities
        .iter()
        .filter(|c| c.state != "ready" && c.state != "not_checked")
        .collect();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} of {} capabilities are not ready.",
        refused.len(),
        report.capabilities.len()
    );

    let _ = writeln!(out, "\n## Endpoints and their fields\n");
    row(&mut out, &["endpoint", "answer", "fields (names only)"]);
    row(&mut out, &["---", "---", "---"]);
    for line in endpoints {
        let fields = if line.fields.is_empty() {
            "-".to_string()
        } else {
            line.fields.join(", ")
        };
        row(&mut out, &[&line.path, &line.state, &fields]);
    }

    let _ = writeln!(out, "\n## Snapshot, redacted\n");
    let _ = writeln!(out, "```json");
    let redacted = report::redacted(snapshot);
    match serde_json::to_string_pretty(&redacted) {
        Ok(json) => {
            let _ = writeln!(out, "{json}");
        }
        Err(e) => {
            let _ = writeln!(out, "{{\"error\": \"serialize: {e}\"}}");
        }
    }
    let _ = writeln!(out, "```");
    let _ = writeln!(
        out,
        "\nValues that identify the SIM, the network or the people on it were \
         replaced before writing. Field names were kept on purpose: a parser is \
         written against names, not against somebody's IMEI."
    );
    out
}

/// Write the document at 0600 and never over an existing file, the same rule
/// the report writer follows.
pub fn write(text: &str, path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Client, Device};

    fn report() -> DoctorReport {
        DoctorReport {
            schema_version: 1,
            scope: "live",
            platform: "linux",
            router: Some("192.168.8.1".into()),
            capabilities: vec![
                Capability {
                    id: "auth".into(),
                    state: "ready".into(),
                    reason: "login_ok".into(),
                    detail: "session established".into(),
                    next_check: None,
                },
                Capability {
                    id: "sms/sms-count".into(),
                    state: "failed".into(),
                    reason: "firmware_error_100005".into(),
                    detail: "not supported by this firmware (100005)".into(),
                    next_check: None,
                },
            ],
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            device: Device {
                name: Some("Mock-CPE".into()),
                software: Some("11.0.2.21".into()),
                serial: Some("SER123".into()),
                imei: Some("867530912345678".into()),
                wan_ip: Some("203.0.113.9".into()),
                ..Default::default()
            },
            clients: vec![Client {
                name: Some("Anna's phone".into()),
                mac: Some("AA:BB:CC:DD:EE:FF".into()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn endpoints() -> Vec<EndpointLine> {
        vec![
            EndpointLine {
                path: "device/information".into(),
                state: "ok".into(),
                fields: vec!["DeviceName".into(), "SerialNumber".into()],
            },
            EndpointLine {
                path: "sms/sms-count".into(),
                state: "unsupported".into(),
                fields: vec![],
            },
        ]
    }

    /// The document is meant to be pasted in a public issue: no identity, but
    /// everything a parser writer needs.
    #[test]
    fn the_shared_document_has_no_identity_and_keeps_the_shapes() {
        let text = render(
            "0.2.4",
            "192.168.8.1",
            1_700_000_000,
            &report(),
            &snapshot(),
            &endpoints(),
        );

        for secret in [
            "SER123",
            "867530912345678",
            "203.0.113.9",
            "AA:BB:CC:DD:EE:FF",
            "Anna's phone",
        ] {
            assert!(!text.contains(secret), "{secret} leaked:\n{text}");
        }
        for wanted in [
            "routwatch 0.2.4",
            "auth",                  // capability id
            "firmware_error_100005", // why sms was refused
            "device/information",    // endpoint path
            "SerialNumber",          // field name, the thing a parser needs
            "Mock-CPE",              // model survives: it is not identity
            "11.0.2.21",             // firmware survives
        ] {
            assert!(text.contains(wanted), "{wanted} missing:\n{text}");
        }
        assert!(text.contains("(hidden)"), "{text}");
    }

    /// A transport failure and a refused endpoint must be told apart in the
    /// table, or the reader cannot tell what to retry.
    #[test]
    fn endpoints_state_is_carried_through() {
        let text = render("0.2.4", "r", 0, &report(), &snapshot(), &endpoints());
        assert!(
            text.contains("| device/information | ok | DeviceName, SerialNumber |"),
            "{text}"
        );
        assert!(
            text.contains("| sms/sms-count | unsupported | - |"),
            "{text}"
        );
    }

    /// The writer must not overwrite: two shares in the same second are two
    /// files, and the mode is 0600.
    #[test]
    fn the_shared_file_is_private_and_never_overwrites() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("routwatch-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = base.join("share.md");

        write("first", &path).expect("first write");
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "share file is not private");
        let err = write("second", &path).expect_err("second write must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(&base);
    }
}
