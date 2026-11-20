//! The capability matrix: what this firmware answered, not what we assume.
//!
//! One row per endpoint, with exactly one of four states:
//!
//! - `answered`: the document came back and parsed far enough to be read.
//! - `refused(code)`: the firmware answered, with an error code. A refusal is
//!   information about this firmware, and it is recorded with the code.
//! - `not-served(status)`: the HTTP layer answered, with a status such as 404.
//!   The path is not part of this build.
//! - `untested`: nothing was learned. A call that never got an answer measured
//!   nothing, so a transport failure is untested rather than refused.
//!
//! Two rules keep the matrix honest:
//!
//! - A refusal is not absence, and absence is not "off". Neither state says
//!   anything about the value of a setting, so neither is ever shown as one.
//! - It is a record, not a permission system. A feature whose endpoint is
//!   `untested` still runs: the matrix may add a sentence to an error message,
//!   and it never hides a feature that the code would otherwise attempt.
//!
//! The file is keyed by the firmware version string and carries the date it was
//! measured, so an answer from another build is not read as a fact about this
//! one. Paths, states and codes are stored; no setting value ever is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::router::endpoints::{Cap, ProbeResult};

/// Bumped only when an older reader would misread the file.
pub const VERSION: u32 = 1;

/// The states a single endpoint can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityState {
    Answered,
    Refused,
    NotServed,
    Untested,
}

impl CapabilityState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CapabilityState::Answered => "answered",
            CapabilityState::Refused => "refused",
            CapabilityState::NotServed => "not-served",
            CapabilityState::Untested => "untested",
        }
    }

    /// True when the firmware declined, in either of the two ways it can.
    pub fn is_refusal(&self) -> bool {
        matches!(self, CapabilityState::Refused | CapabilityState::NotServed)
    }
}

/// One endpoint's recorded answer. At most one of `code` and `status` is set:
/// a firmware error carries a code, a transport answer carries an HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub state: CapabilityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl Entry {
    fn answered() -> Self {
        Entry {
            state: CapabilityState::Answered,
            code: None,
            status: None,
        }
    }

    fn untested() -> Self {
        Entry {
            state: CapabilityState::Untested,
            code: None,
            status: None,
        }
    }

    fn of_cap(cap: &Cap) -> Self {
        match cap {
            Cap::Ok => Entry::answered(),
            // A path the firmware does not serve answers 404 at the HTTP layer.
            Cap::Unsupported(404) => Entry {
                state: CapabilityState::NotServed,
                code: None,
                status: Some(404),
            },
            Cap::Unsupported(code) | Cap::Denied(code) => Entry {
                state: CapabilityState::Refused,
                code: Some(*code),
                status: None,
            },
            // The call never reached an answer: nothing was measured.
            Cap::Failed(_) => Entry::untested(),
        }
    }

    /// The one-line sentence a consumer may print. `None` for the states that
    /// carry no news, which includes `untested`: it is not evidence of absence.
    pub fn note(&self) -> Option<String> {
        match self.state {
            CapabilityState::Refused => Some(match self.code {
                Some(c) => format!("this firmware refused the read with {c}"),
                None => "this firmware refused the read".into(),
            }),
            CapabilityState::NotServed => Some(match self.status {
                Some(s) => format!("this firmware does not serve the path (HTTP {s})"),
                None => "this firmware does not serve the path".into(),
            }),
            CapabilityState::Answered | CapabilityState::Untested => None,
        }
    }
}

/// The measured answers for one firmware build.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CapabilityStore {
    pub v: u32,
    pub firmware: String,
    pub measured_at: u64,
    pub entries: BTreeMap<String, Entry>,
}

impl CapabilityStore {
    /// Build a store from one probe pass. Endpoints absent from `results` stay
    /// out of the map, and `state_of` reads them as untested rather than as a
    /// guess.
    pub fn from_probe(firmware: &str, measured_at: u64, results: &[ProbeResult]) -> Self {
        let entries = results
            .iter()
            .map(|r| (r.path.to_string(), Entry::of_cap(&r.cap)))
            .collect();
        CapabilityStore {
            v: VERSION,
            firmware: firmware.to_string(),
            measured_at,
            entries,
        }
    }

    /// The recorded state of a path. Anything the store does not hold, or holds
    /// for another firmware build, is untested.
    pub fn state_of(&self, path: &str) -> CapabilityState {
        self.entries
            .get(path)
            .map(|e| e.state)
            .unwrap_or(CapabilityState::Untested)
    }

    /// The sentence to append to an error, when the matrix has news.
    pub fn note(&self, path: &str) -> Option<String> {
        self.entries.get(path).and_then(|e| e.note())
    }

    /// Counts in the order answered, refused, not-served, untested.
    pub fn counts(&self) -> [usize; 4] {
        let mut out = [0usize; 4];
        for e in self.entries.values() {
            let i = match e.state {
                CapabilityState::Answered => 0,
                CapabilityState::Refused => 1,
                CapabilityState::NotServed => 2,
                CapabilityState::Untested => 3,
            };
            out[i] += 1;
        }
        out
    }
}

pub fn capabilities_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("capabilities.json"))
}

/// The sentence to append to a refusal, read from the recorded matrix when one
/// exists. Best-effort by design: no matrix, an unreadable matrix or an
/// `untested` row all return `None`, so the caller's own message stands alone
/// and no attempt is ever blocked. This is what keeps the matrix a record
/// rather than a permission system.
pub fn note_for(endpoint: &str) -> Option<String> {
    let store = load(&capabilities_path()?).ok().flatten()?;
    store.note(endpoint)
}

/// Write the matrix 0600 through the project's only private writer. `force` is
/// set because re-measuring is the command's purpose.
pub fn save(path: &Path, store: &CapabilityStore) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::output::write_private(path, &body, true)
}

/// `None` when nothing was measured yet. A file that will not parse is an
/// error the caller prints, not an empty matrix: silence would read as "this
/// firmware answers nothing".
pub fn load(path: &Path) -> Result<Option<CapabilityStore>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let store: CapabilityStore =
        serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    if store.v > VERSION {
        return Err(format!(
            "{}: written by a newer routwatch (v{})",
            path.display(),
            store.v
        ));
    }
    Ok(Some(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn results() -> Vec<ProbeResult> {
        vec![
            ProbeResult {
                group: "signal",
                path: "device/signal",
                cap: Cap::Ok,
            },
            ProbeResult {
                group: "security",
                path: "security/mac-filter",
                cap: Cap::Unsupported(100003),
            },
            ProbeResult {
                group: "sms",
                path: "sms/sms-list",
                cap: Cap::Unsupported(100005),
            },
            ProbeResult {
                group: "clients",
                path: "wlan/host-list",
                cap: Cap::Unsupported(404),
            },
            ProbeResult {
                group: "device",
                path: "device/temperature",
                cap: Cap::Failed("connection reset".into()),
            },
        ]
    }

    fn scratch(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rw-cap-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("capabilities.json")
    }

    #[test]
    fn the_four_states_follow_the_answer() {
        let store = CapabilityStore::from_probe("11.0.2.21", 1_700_000_000, &results());
        assert_eq!(store.state_of("device/signal"), CapabilityState::Answered);
        assert_eq!(
            store.state_of("security/mac-filter"),
            CapabilityState::Refused
        );
        assert_eq!(
            store.entries["security/mac-filter"].code,
            Some(100003),
            "a refusal keeps the code the firmware gave"
        );
        assert_eq!(store.state_of("wlan/host-list"), CapabilityState::NotServed);
        assert_eq!(store.entries["wlan/host-list"].status, Some(404));
        assert_eq!(store.state_of("sms/sms-list"), CapabilityState::Refused);
    }

    #[test]
    fn a_call_that_never_answered_is_untested_not_refused() {
        let store = CapabilityStore::from_probe("11.0.2.21", 1, &results());
        assert_eq!(
            store.state_of("device/temperature"),
            CapabilityState::Untested
        );
        assert_eq!(store.note("device/temperature"), None);
    }

    #[test]
    fn a_path_nobody_measured_is_untested() {
        let store = CapabilityStore::from_probe("11.0.2.21", 1, &results());
        assert_eq!(store.state_of("ntwk/tr069"), CapabilityState::Untested);
        assert_eq!(store.counts(), [1, 2, 1, 1]);
    }

    #[test]
    fn an_untested_path_never_hides_a_feature() {
        let store = CapabilityStore::from_probe("11.0.2.21", 1, &results());
        for path in ["ntwk/tr069", "device/temperature", "device/signal"] {
            assert!(
                store.note(path).is_none(),
                "{path} carries no refusal, so it must not be reported as one"
            );
        }
        assert!(store
            .note("security/mac-filter")
            .unwrap()
            .contains("100003"));
    }

    #[test]
    fn the_matrix_round_trips_through_a_0600_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("round");
        let store = CapabilityStore::from_probe("11.0.2.21", 42, &results());
        save(&path, &store).expect("save");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the matrix is private");
        let back = load(&path).expect("load").expect("some");
        assert_eq!(back, store);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_newer_matrix_is_an_error_not_an_empty_one() {
        let path = scratch("newer");
        std::fs::write(
            &path,
            r#"{"v":99,"firmware":"x","measured_at":1,"entries":{}}"#,
        )
        .expect("write");
        assert!(load(&path).unwrap_err().contains("newer routwatch"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
