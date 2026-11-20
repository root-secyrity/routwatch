//! Config snapshots: what the settings read as, without the settings.
//!
//! A snapshot answers one question: did this setting change since last time?
//! Storing the values would answer it and also put the Wi-Fi key, the admin
//! password and the identity fields on disk, which is a different trade. So
//! every value is reduced to a salted fingerprint, and the four fields this
//! firmware carries that are secrets are not fingerprinted at all: their row
//! records that the field answered, never what it answered. A changed Wi-Fi key
//! therefore reads as `set` before and after, and the limitations document says
//! so instead of implying coverage it does not have.
//!
//! The salt is 32 bytes from `/dev/urandom`, written once at 0600 beside the
//! snapshots. Its only job is to stop a fingerprint from being compared against
//! a rainbow table from another machine; it is not a secret store.
//!
//! Changes are only declared when two consecutive reads agree: this firmware
//! answers some documents in ~45 ms and others in ~1500 ms, and a single read
//! that lands in the wrong tail is not a setting that moved. Readings the two
//! reads disagree on are counted as unsettled and never reported.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::router::client::HuaweiClient;
use crate::xml::Node;

/// Bumped only when an older reader would misread a snapshot.
pub const VERSION: u32 = 1;

/// Snapshots kept on disk. Each one is small, but a setting diff is read against
/// the newest only, so older ones are history the owner can look at, not data a
/// command needs.
pub const KEEP: usize = 20;

/// The endpoints a snapshot reads, in one session, serially. Every one of them
/// answers on the H122-373 (measured 2026-11-20); a refusal is recorded rather
/// than skipped, because a path that stopped answering is a visibility change.
pub const ENDPOINTS: [&str; 8] = [
    "device/information",
    "wlan/basic-settings",
    "dhcp/settings",
    "dhcp/feature-switch",
    "security/firewall-switch",
    "security/mac-filter",
    "global/module-switch",
    "net/net-mode",
];

/// Hex characters kept from the digest. Change detection needs collision
/// resistance, not a signature; 128 bits is far past the number of settings.
const FP_CHARS: usize = 32;

/// One read of the settings, reduced to fingerprints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub v: u32,
    pub at_unix: u64,
    /// The firmware build that answered. Two snapshots from different builds are
    /// not compared field by field: the fields themselves may have moved.
    pub firmware: String,
    /// `endpoint#field/path` to fingerprint.
    pub fields: BTreeMap<String, String>,
    /// Endpoints that did not answer, with what the call said.
    pub refused: BTreeMap<String, String>,
    /// `endpoint#field` recorded as answered, with no value and no fingerprint.
    pub secrets: BTreeSet<String>,
}

impl Snapshot {
    pub fn len(&self) -> usize {
        self.fields.len() + self.secrets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0 && self.refused.is_empty()
    }
}

/// How a field moved between two snapshots. `Unchanged` is not a class here: a
/// row that did not move is not reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Added,
    Removed,
    Changed,
    /// The endpoint answered before and refuses now (or the reverse). The field
    /// did not necessarily change; our ability to read it did.
    Visibility,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Added => "added",
            Class::Removed => "removed",
            Class::Changed => "changed",
            Class::Visibility => "visibility-changed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub class: Class,
    pub key: String,
}

/// The outcome of comparing two snapshots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub rows: Vec<Row>,
    /// The two snapshots were taken under different firmware builds. Field
    /// alarms are suppressed: this is a report about the build, not about a
    /// setting that moved.
    pub firmware_changed: bool,
    /// Fields the two reads of the current state disagreed on. They are left
    /// out of the comparison entirely.
    pub unsettled: usize,
}

impl Report {
    pub fn drifted(&self) -> bool {
        !self.rows.is_empty() && !self.firmware_changed
    }
}

// ---------------------------------------------------------------------------
// paths and salt

pub fn dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("config-snapshots"))
}

fn salt_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("config-salt"))
}

/// The per-install salt, created on first use at 0600. A second process that
/// wins the creation race re-reads the file rather than generating another one,
/// so two callers always agree on the fingerprints they write.
fn salt() -> std::io::Result<[u8; 32]> {
    let path = salt_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no data directory: set XDG_DATA_HOME",
        )
    })?;
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(out) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(out);
        }
    }
    let mut fresh = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut fresh)?;
    match crate::output::write_private(&path, &fresh, false) {
        Ok(()) => Ok(fresh),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = std::fs::read(&path)?;
            <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: not 32 bytes", path.display()),
                )
            })
        }
        Err(e) => Err(e),
    }
}

/// Salt plus value, digest kept short. Not reversible and not portable: the salt
/// never leaves the machine it was generated on.
fn fingerprint(salt: &[u8; 32], value: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt);
    h.update([0u8]);
    h.update(value.as_bytes());
    let digest = h.finalize();
    digest
        .iter()
        .take(FP_CHARS / 2)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Field names this firmware uses for secrets. Presence is recorded, the value
/// is not: a fingerprint of a short Wi-Fi key is a guessable fingerprint.
fn is_secret(field: &str) -> bool {
    let f = field.to_ascii_lowercase();
    [
        "password",
        "wpa",
        "psk",
        "wep",
        "key",
        "acsurl",
        "token",
        "secret",
        "credential",
    ]
    .iter()
    .any(|needle| f.contains(needle))
}

// ---------------------------------------------------------------------------
// reading

/// Read the settings once. Read-only: the only POSTs on this path are the login
/// and the logout that `connect` performs.
pub fn collect(client: &mut HuaweiClient, at_unix: u64) -> Result<Snapshot, ApiError> {
    let salt = salt().map_err(|e| ApiError::Config(format!("config salt: {e}")))?;
    let mut snap = Snapshot {
        v: VERSION,
        at_unix,
        firmware: "unknown".to_string(),
        fields: BTreeMap::new(),
        refused: BTreeMap::new(),
        secrets: BTreeSet::new(),
    };
    for endpoint in ENDPOINTS {
        match client.get(endpoint) {
            Ok(node) => {
                if endpoint == "device/information" {
                    if let Some(build) = node.text_of("SoftwareVersion") {
                        if !build.trim().is_empty() {
                            snap.firmware = build.trim().to_string();
                        }
                    }
                }
                // The root is the envelope (`<response>`); the fields are its
                // children, so the key names a setting and not the envelope.
                let mut leaves = Vec::new();
                for child in &node.children {
                    walk("", child, &mut leaves);
                }
                let mut seen: BTreeMap<String, usize> = BTreeMap::new();
                for (field, value) in leaves {
                    // Repeated names are list rows: two bands, two clients.
                    let n = seen.entry(field.clone()).or_insert(0);
                    *n += 1;
                    let field = if *n == 1 {
                        field
                    } else {
                        format!("{field}#{n}")
                    };
                    let key = format!("{endpoint}#{field}");
                    if is_secret(&field) {
                        snap.secrets.insert(key);
                    } else {
                        snap.fields.insert(key, fingerprint(&salt, &value));
                    }
                }
            }
            Err(e) => {
                snap.refused.insert(endpoint.to_string(), e.to_string());
            }
        }
    }
    Ok(snap)
}

/// Every leaf under `node`, as `parent/child` paths. Repeated names (list rows)
/// are numbered so two clients do not collapse onto one key.
fn walk(prefix: &str, node: &Node, out: &mut Vec<(String, String)>) {
    let here = if prefix.is_empty() {
        node.name.clone()
    } else {
        format!("{prefix}/{}", node.name)
    };
    if node.children.is_empty() {
        let text = node.text.trim();
        if !text.is_empty() {
            out.push((here, text.to_string()));
        }
        return;
    }
    for child in &node.children {
        walk(&here, child, out);
    }
}

// ---------------------------------------------------------------------------
// settling two reads

/// The part of the current state two consecutive reads agree on. Everything they
/// disagree about is dropped and counted, because a value that answered two ways
/// a second apart is not a value this tool can call a setting.
pub fn settle(first: &Snapshot, second: &Snapshot) -> (Snapshot, BTreeSet<String>) {
    let mut out = Snapshot {
        v: VERSION,
        at_unix: second.at_unix,
        firmware: second.firmware.clone(),
        fields: BTreeMap::new(),
        refused: BTreeMap::new(),
        secrets: BTreeSet::new(),
    };
    // A key either read the same twice or it is not a reading this tool will
    // report; there is no third answer, and the caller needs the names so a
    // dropped key is never mistaken for one that the firmware stopped serving.
    let mut unsettled: BTreeSet<String> = BTreeSet::new();
    for (key, fp) in &second.fields {
        match first.fields.get(key) {
            Some(prev) if prev == fp => {
                out.fields.insert(key.clone(), fp.clone());
            }
            _ => {
                unsettled.insert(key.clone());
            }
        }
    }
    for key in &second.secrets {
        if first.secrets.contains(key) {
            out.secrets.insert(key.clone());
        } else {
            unsettled.insert(key.clone());
        }
    }
    for endpoint in second.refused.keys() {
        if first.refused.contains_key(endpoint) {
            out.refused
                .insert(endpoint.clone(), second.refused[endpoint].clone());
        } else {
            unsettled.insert(endpoint.clone());
        }
    }
    (out, unsettled)
}

// ---------------------------------------------------------------------------
// comparing

pub fn diff(before: &Snapshot, now: &Snapshot) -> Report {
    diff_skipping(before, now, &BTreeSet::new())
}

/// The same comparison, with the readings two consecutive reads could not agree
/// on left out entirely: they are neither removed nor changed, they are unread.
pub fn diff_skipping(before: &Snapshot, now: &Snapshot, skip: &BTreeSet<String>) -> Report {
    let mut report = Report {
        firmware_changed: before.firmware != now.firmware,
        ..Report::default()
    };
    if report.firmware_changed {
        return report;
    }
    let refused_before: BTreeSet<&str> = before.refused.keys().map(String::as_str).collect();
    let refused_now: BTreeSet<&str> = now.refused.keys().map(String::as_str).collect();

    // A whole endpoint that started or stopped answering is one visibility row,
    // not one row per field it used to carry.
    for endpoint in &refused_now {
        if !refused_before.contains(endpoint) {
            report.rows.push(Row {
                class: Class::Visibility,
                key: (*endpoint).to_string(),
            });
        }
    }
    for endpoint in &refused_before {
        if !refused_now.contains(endpoint) {
            report.rows.push(Row {
                class: Class::Visibility,
                key: (*endpoint).to_string(),
            });
        }
    }
    let hidden = |key: &str| {
        let endpoint = key.split('#').next().unwrap_or("");
        refused_now.contains(endpoint)
    };

    for (key, fp) in &now.fields {
        if skip.contains(key) {
            continue;
        }
        match before.fields.get(key) {
            Some(prev) if prev == fp => {}
            Some(_) => report.rows.push(Row {
                class: Class::Changed,
                key: key.clone(),
            }),
            None => {
                if !hidden(key) && !refused_before.contains(key.split('#').next().unwrap_or("")) {
                    report.rows.push(Row {
                        class: Class::Added,
                        key: key.clone(),
                    });
                }
            }
        }
    }
    for key in before.fields.keys() {
        if skip.contains(key) || now.fields.contains_key(key) || hidden(key) {
            continue;
        }
        if refused_before.contains(key.split('#').next().unwrap_or("")) {
            continue;
        }
        report.rows.push(Row {
            class: Class::Removed,
            key: key.clone(),
        });
    }
    for key in &now.secrets {
        if !skip.contains(key) && !before.secrets.contains(key) && !hidden(key) {
            report.rows.push(Row {
                class: Class::Added,
                key: format!("{key} (presence only)"),
            });
        }
    }
    for key in &before.secrets {
        if !skip.contains(key) && !now.secrets.contains(key) && !hidden(key) {
            report.rows.push(Row {
                class: Class::Removed,
                key: format!("{key} (presence only)"),
            });
        }
    }
    report.rows.sort_by(|a, b| a.key.cmp(&b.key));
    report
}

// ---------------------------------------------------------------------------
// storage

fn file_name(at_unix: u64) -> String {
    format!("config-{at_unix}.json")
}

pub fn save(dir: &Path, snap: &Snapshot) -> std::io::Result<PathBuf> {
    let path = dir.join(file_name(snap.at_unix));
    let bytes = serde_json::to_vec_pretty(snap)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::output::write_private(&path, &bytes, true)?;
    if let Err(e) = prune(dir, KEEP) {
        // A snapshot that was written and not pruned is still a snapshot; the
        // caller prints this rather than losing the file it just made.
        eprintln!("warning: could not prune {}: {e}", dir.display());
    }
    Ok(path)
}

pub fn load(path: &Path) -> std::io::Result<Snapshot> {
    let bytes = std::fs::read(path)?;
    let snap: Snapshot = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if snap.v > VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "snapshot written by a newer routwatch",
        ));
    }
    Ok(snap)
}

/// Snapshot paths, oldest first. Names carry the read time, so sorting by name
/// is sorting by age.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("config-") && n.ends_with(".json"))
        })
        .collect();
    out.sort();
    out
}

pub fn latest(dir: &Path) -> Option<PathBuf> {
    list(dir).pop()
}

fn prune(dir: &Path, keep: usize) -> std::io::Result<()> {
    let all = list(dir);
    if all.len() <= keep {
        return Ok(());
    }
    for path in &all[..all.len() - keep] {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(firmware: &str) -> Snapshot {
        Snapshot {
            v: VERSION,
            at_unix: 10,
            firmware: firmware.to_string(),
            fields: BTreeMap::new(),
            refused: BTreeMap::new(),
            secrets: BTreeSet::new(),
        }
    }

    #[test]
    fn a_fingerprint_does_not_carry_the_value() {
        let salt = [7u8; 32];
        let fp = fingerprint(&salt, "SuperSecretWifi");
        assert_eq!(fp.len(), FP_CHARS);
        assert!(!fp.contains("Super"));
        assert_eq!(fp, fingerprint(&salt, "SuperSecretWifi"));
        assert_ne!(fp, fingerprint(&[8u8; 32], "SuperSecretWifi"));
    }

    #[test]
    fn the_four_secret_fields_this_firmware_carries_are_recognised() {
        for field in ["WifiKey", "WpaPsk", "AdminPassword", "Tr069ACSURL"] {
            assert!(is_secret(field), "{field} must be presence-only");
        }
        for field in [
            "Ssid",
            "DhcpIPAddress",
            "FirewallMainSwitch",
            "SoftwareVersion",
        ] {
            assert!(!is_secret(field), "{field} is a setting, not a secret");
        }
    }

    #[test]
    fn a_new_firmware_suppresses_field_alarms() {
        let mut before = snap("11.0.2.21");
        before
            .fields
            .insert("dhcp/settings#DhcpIPAddress".into(), "aa".into());
        let mut now = snap("11.0.2.22");
        now.fields
            .insert("dhcp/settings#DhcpIPAddress".into(), "bb".into());
        let r = diff(&before, &now);
        assert!(r.firmware_changed);
        assert!(r.rows.is_empty(), "no field alarm across a build change");
        assert!(!r.drifted());
    }

    #[test]
    fn an_endpoint_that_stops_answering_is_a_visibility_change() {
        let mut before = snap("11.0.2.21");
        before
            .fields
            .insert("security/mac-filter#MacFilterMode".into(), "aa".into());
        let mut now = snap("11.0.2.21");
        now.refused
            .insert("security/mac-filter".into(), "refused 100003".into());
        let r = diff(&before, &now);
        assert_eq!(
            r.rows,
            vec![Row {
                class: Class::Visibility,
                key: "security/mac-filter".into()
            }],
            "a refused endpoint must not read as a removed field"
        );
    }

    #[test]
    fn a_reading_the_two_reads_disagree_on_is_not_a_setting() {
        let mut first = snap("11.0.2.21");
        first.fields.insert("a#b".into(), "aa".into());
        let mut second = snap("11.0.2.21");
        second.fields.insert("a#b".into(), "bb".into());
        let (settled, unsettled) = settle(&first, &second);
        assert_eq!(unsettled.len(), 1);
        assert!(unsettled.contains("a#b"));
        assert!(settled.fields.is_empty());
    }
}
