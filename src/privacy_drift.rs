//! The privacy baseline and its drift.
//!
//! A baseline is what the router answered when the owner last said "this is how
//! it should be". Drift is the news that an answer changed, and it is expensive
//! to get wrong: this firmware refuses whole documents without warning, and a
//! refusal can look exactly like a setting that changed if you compare naively.
//!
//! Three rules make a drift trustworthy:
//!
//! - A refusal is not a change. `unknown` and `not exposed` are the firmware
//!   declining to answer; neither says anything about the setting, so neither is
//!   compared. `unknown` is also never folded into `off`.
//! - Values are compared, not the presence of a field. A check that is missing
//!   from this read is no information, not a drift.
//! - Two reads must agree. One read of a flaky document is a rumour; the drift is
//!   reported on the second read that carries the same new value, and reported
//!   once.
//!
//! The baseline file is private (0600) through the same writer config uses, so a
//! baseline cannot land world-readable beside a config that is not.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::privacy::{Audit, State};

/// Bumped only when an older reader would misread the file.
pub const VERSION: u32 = 1;

/// What the owner accepted. `checks` maps a check name to the state word, which
/// is the value that is compared; the human detail is not compared because it
/// carries a DNS address that legitimately changes by lease.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Baseline {
    pub v: u32,
    pub saved_at: u64,
    pub checks: BTreeMap<String, String>,
}

impl Baseline {
    pub fn from_audit(audit: &Audit, at_unix: u64) -> Self {
        Baseline {
            v: VERSION,
            saved_at: at_unix,
            checks: audit
                .checks
                .iter()
                .map(|c| (c.name.to_string(), c.state.as_str().to_string()))
                .collect(),
        }
    }
}

pub fn path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("routwatch").join("privacy-baseline.json"))
}

/// Write the baseline 0600. `output::write_private` is the project's only 0600
/// writer, and `force` is set because re-baselining is the command's purpose.
pub fn save(path: &Path, base: &Baseline) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(base)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::output::write_private(path, &body, true)
}

/// `None` when there is no baseline yet. A baseline that will not parse is an
/// error the caller prints rather than silently treating as "no drift".
pub fn load(path: &Path) -> Result<Option<Baseline>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let base: Baseline =
        serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
    if base.v > VERSION {
        return Err(format!(
            "{}: written by a newer routwatch (v{})",
            path.display(),
            base.v
        ));
    }
    Ok(Some(base))
}

/// One settled change: the name, what the baseline said, and what two reads
/// agreed on.
#[derive(Debug, Clone, PartialEq)]
pub struct Drift {
    pub name: String,
    pub from: String,
    pub to: String,
}

impl Drift {
    pub fn line(&self) -> String {
        format!(
            "{}: {} -> {} (two reads agreed)",
            self.name, self.from, self.to
        )
    }
}

/// The two-agreement filter. It is stateful on purpose: the first read that
/// disagrees with the baseline arms a candidate, the second settles it.
#[derive(Debug, Default)]
pub struct Agree {
    pending: BTreeMap<String, String>,
    reported: BTreeMap<String, String>,
}

impl Agree {
    /// The drifts this read settles, oldest name first. `base` is not modified:
    /// a baseline is only replaced by the owner's own command.
    pub fn observe(&mut self, base: &Baseline, now: &Audit) -> Vec<Drift> {
        let mut found = Vec::new();
        for check in &now.checks {
            let name = check.name;
            let Some(before) = base.checks.get(name) else {
                // Not in the baseline: nothing was accepted to drift from.
                continue;
            };
            if !is_answer(&check.state) {
                // A refusal leaves the candidate as it was: it neither settles
                // nor clears a change the next read may confirm.
                continue;
            }
            let value = check.state.as_str().to_string();
            if &value == before {
                self.pending.remove(name);
                self.reported.remove(name);
                continue;
            }
            if self.pending.get(name) == Some(&value) && self.reported.get(name) != Some(&value) {
                found.push(Drift {
                    name: name.to_string(),
                    from: before.clone(),
                    to: value.clone(),
                });
                self.reported.insert(name.to_string(), value.clone());
            }
            self.pending.insert(name.to_string(), value);
        }
        found
    }
}

/// A state that answers the question. `Unknown` is a refused document and
/// `NotExposed` is an endpoint the firmware does not serve; neither is a value of
/// the setting, so neither can drift.
fn is_answer(state: &State) -> bool {
    matches!(state, State::On | State::Off)
}

/// The check names in a baseline, for a one-line report of what is watched.
pub fn watched(base: &Baseline) -> Vec<String> {
    base.checks.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privacy::Check;

    fn audit(pairs: &[(&'static str, State)]) -> Audit {
        Audit {
            checks: pairs
                .iter()
                .map(|(n, s)| Check {
                    name: n,
                    state: *s,
                    detail: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_refusal_never_settles_a_drift() {
        let base = Baseline::from_audit(&audit(&[("tr069", State::On)]), 1);
        let mut agree = Agree::default();
        assert!(agree
            .observe(&base, &audit(&[("tr069", State::Unknown)]))
            .is_empty());
        assert!(agree
            .observe(&base, &audit(&[("tr069", State::NotExposed)]))
            .is_empty());
        assert!(agree
            .observe(&base, &audit(&[("tr069", State::Unknown)]))
            .is_empty());
    }
}
