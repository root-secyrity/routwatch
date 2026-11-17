//! Read-only answers to "who else can reach this router".
//!
//! Every check names its source endpoint and what the firmware answered. A
//! refused endpoint is `NotExposed` with the code, never "off": a missing answer
//! is not a safe answer. Nothing here writes: `Audit::collect` must never call a
//! `set_*` method, and `the_audit_never_posts` holds that.

use crate::error::ApiError;
use crate::router::client::HuaweiClient;
use crate::router::{endpoints, parse};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    On,
    Off,
    NotExposed,
    Unknown,
}

impl State {
    /// The word the text table prints. `Debug` would spell `NotExposed` as
    /// `notexposed`, which reads as a typo in the one command a user checks.
    pub fn as_str(self) -> &'static str {
        match self {
            State::On => "on",
            State::Off => "off",
            State::NotExposed => "not exposed",
            State::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub state: State,
    /// The endpoint and the evidence, in one line.
    pub detail: String,
}

/// What the audit says about TR-069 when no path is confirmed on the firmware.
/// A measured fact, so it carries the firmware and the date it was measured.
pub const TR069_DECLARATION: &str =
    "not exposed by this firmware (probe returned 100003) · measured H122-373 WEBUI 11.0.2.21 2026-11-17";

/// An endpoint the firmware refuses or does not serve is `NotExposed`; anything
/// else (busy, malformed, a lost reply) is `Unknown`, because silence and a
/// refusal are different facts. `classify` turns 100002/100003 into
/// `Unsupported` and an unserved path into `Http(4xx)`, so those two arms carry
/// the real refusals; `Router` is kept for codes with no mapping.
pub fn classify_error(name: &'static str, e: &ApiError) -> Check {
    let state = match e {
        ApiError::Unsupported(_) | ApiError::Denied(_) | ApiError::Http(400..=499) => {
            State::NotExposed
        }
        _ => State::Unknown,
    };
    Check {
        name,
        state,
        detail: format!("{e}"),
    }
}

pub fn classify_field(name: &'static str, value: Option<&str>) -> Check {
    match value.map(str::trim) {
        Some("1") => Check {
            name,
            state: State::On,
            detail: format!("{name}=1"),
        },
        Some("0") => Check {
            name,
            state: State::Off,
            detail: format!("{name}=0"),
        },
        Some(other) => Check {
            name,
            state: State::Unknown,
            detail: format!("{name}={other}"),
        },
        None => Check {
            name,
            state: State::Unknown,
            detail: format!("{name}: not reported"),
        },
    }
}

/// The checks that follow from the probe list alone, with no client at all. A
/// firmware that serves none of `endpoints::PRIVACY` still gets a named answer
/// instead of an empty report.
pub fn declaration_checks() -> Vec<Check> {
    if endpoints::PRIVACY.is_empty() {
        return vec![Check {
            name: "tr069",
            state: State::NotExposed,
            detail: TR069_DECLARATION.to_string(),
        }];
    }
    Vec::new()
}

/// The whole audit, serialisable as it stands: `privacy --json` prints this
/// value, so the derive belongs here and not on a wrapper built later.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Audit {
    pub checks: Vec<Check>,
}

impl Audit {
    /// Read-only. Any write path here is a defect: this function must never
    /// call `set_*`.
    pub fn collect(client: &mut HuaweiClient) -> Audit {
        let mut checks = declaration_checks();
        for spec in endpoints::PRIVACY {
            match client.get(spec.path) {
                Ok(node) => checks.push(classify_field("tr069", node.text_of("Tr069Enable"))),
                Err(e) => checks.push(classify_error("tr069", &e)),
            }
        }
        checks.push(dns_check(client));
        checks.push(firewall_check(client));
        checks.push(admin_surface_check(client));
        Audit { checks }
    }

    /// One line for `doctor --live`, built from the same checks the command
    /// prints so the two cannot disagree.
    pub fn summary(&self) -> String {
        self.checks
            .iter()
            .map(headline)
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// One check as the single-word-plus-fact the summary needs. DNS carries its
/// address, because "dns on" would hide the only thing worth reading.
fn headline(c: &Check) -> String {
    if c.name == "dns" {
        let first = c.detail.split(" · ").next().unwrap_or("");
        let addr = first
            .strip_prefix("dhcp advertises ")
            .unwrap_or(first)
            .to_string();
        return format!("dns {addr}");
    }
    format!("{} {}", c.name, short_state(c))
}

/// The state word the summary uses, with the code beside a refusal. `On`/`Off`
/// read as `on`/`off`; the detail stays in the full table.
fn short_state(c: &Check) -> String {
    match c.state {
        State::NotExposed => {
            let code = c
                .detail
                .split(|ch: char| !ch.is_ascii_digit())
                .find(|s| s.len() >= 3)
                .unwrap_or("");
            if code.is_empty() {
                "not exposed".to_string()
            } else {
                format!("not exposed ({code})")
            }
        }
        State::On => "on".to_string(),
        State::Off => "off".to_string(),
        State::Unknown => "unknown".to_string(),
    }
}

/// DNS is not a switch, so its state means "the two sources agree": the servers
/// the DHCP answer advertises and the server `monitoring/status` reports for the
/// live link are different facts, and a line that called the first one "in use"
/// would be guessing.
fn dns_check(client: &mut HuaweiClient) -> Check {
    let advertised = match client.get("dhcp/settings") {
        Ok(n) => Some(parse::parse_dns(&n)),
        Err(_) => None,
    };
    let link = client
        .get("monitoring/status")
        .ok()
        .and_then(|n| n.text_of("PrimaryDns").map(str::to_string));

    let adv_primary = advertised.as_ref().and_then(|d| d.primary.clone());
    let adv_line = match &adv_primary {
        Some(p) => format!("dhcp advertises {p}"),
        None => "dhcp advertises nothing (no PrimaryDns)".to_string(),
    };
    let link_line = match &link {
        Some(l) => format!("link reports {l}"),
        None => "link reports nothing (no PrimaryDns on monitoring/status)".to_string(),
    };
    let state = match (&adv_primary, &link) {
        (Some(a), Some(l)) if a == l => State::On,
        (Some(_), Some(_)) => State::Unknown,
        _ => State::Unknown,
    };
    Check {
        name: "dns",
        state,
        detail: format!("{adv_line} · {link_line}"),
    }
}

fn firewall_check(client: &mut HuaweiClient) -> Check {
    match client.get("security/firewall-switch") {
        Ok(node) => {
            let mut c = classify_field("firewall", node.text_of("FirewallMainSwitch"));
            c.detail
                .push_str(" · FirewallMainSwitch on security/firewall-switch");
            c
        }
        Err(e) => classify_error("firewall", &e),
    }
}

/// Whether an unauthenticated client on the WLAN reaches the admin pages is not
/// a field this firmware reports: `wlan/basic-settings` carries the radio and
/// `dhcp/settings` the LAN address, and neither answers the question. The check
/// prints what it read and says `unknown` rather than turning silence into a
/// pass.
fn admin_surface_check(client: &mut HuaweiClient) -> Check {
    let dhcp = client.get("dhcp/settings").ok();
    let wlan = client.get("wlan/basic-settings").ok();
    if dhcp.is_none() && wlan.is_none() {
        return Check {
            name: "admin",
            state: State::NotExposed,
            detail: "neither dhcp/settings nor wlan/basic-settings answered".to_string(),
        };
    }
    // `accessipaddress` is empty on the H122-373 while `homeurl` carries the name
    // the DHCP answer gives the router, and an empty string is not an address.
    let lan = dhcp
        .as_ref()
        .and_then(|n| {
            ["accessipaddress", "homeurl"]
                .iter()
                .filter_map(|f| n.text_of(f))
                .map(str::trim)
                .find(|v| !v.is_empty())
        })
        .map(str::to_string)
        .unwrap_or_else(|| "not reported".to_string());
    let radio = match wlan.as_ref().and_then(|n| n.text_of("WifiEnable")) {
        Some("1") => "wifi on".to_string(),
        Some("0") => "wifi off".to_string(),
        Some(other) => format!("wifi={other}"),
        None => "wifi not reported".to_string(),
    };
    Check {
        name: "admin",
        state: State::Unknown,
        detail: format!(
            "lan address {lan} · {radio} · no field reports whether the web UI needs a password over WLAN"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_endpoint_is_not_exposed_with_its_code() {
        // `classify` maps 100003 to Unsupported, not Router (`src/error.rs:65-73`).
        // Matching Router alone would call every real refusal `Unknown`.
        let c = classify_error("tr069", &ApiError::Unsupported(100003));
        assert_eq!(c.state, State::NotExposed);
        assert!(c.detail.contains("100003"));
    }

    #[test]
    fn an_unserved_path_is_not_exposed_even_as_a_404() {
        // A firmware that does not serve the path answers 404, which the client
        // turns into Http(404) (`src/router/client.rs:174`).
        let c = classify_error("tr069", &ApiError::Http(404));
        assert_eq!(c.state, State::NotExposed);
        assert!(c.detail.contains("404"));
    }

    #[test]
    fn a_missing_field_is_unknown_not_off() {
        let c = classify_field("tr069", None);
        assert_eq!(c.state, State::Unknown);
        assert!(c.detail.contains("not reported"));
    }

    #[test]
    fn a_zero_flag_is_off_and_a_one_is_on() {
        assert_eq!(State::NotExposed.as_str(), "not exposed");
        assert_eq!(classify_field("tr069", Some("0")).state, State::Off);
        assert_eq!(classify_field("tr069", Some("1")).state, State::On);
    }

    #[test]
    fn an_empty_probe_list_still_declares_tr069() {
        let checks = declaration_checks();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].name, "tr069");
        assert_eq!(checks[0].state, State::NotExposed);
        assert!(checks[0].detail.contains("100003"), "{}", checks[0].detail);
        assert!(checks[0].detail.contains("11.0.2.21"));
    }

    #[test]
    fn the_audit_never_posts() {
        let mock = crate::mock_cpe::MockCpe::start(crate::mock_cpe::Profile::H122373);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let before = mock.posts();
        let audit = Audit::collect(&mut c);
        assert_eq!(mock.posts(), before, "the audit must not POST");
        assert!(audit.checks.iter().any(|c| c.name == "tr069"));
        assert!(audit.checks.iter().any(|c| c.name == "dns"));
        assert!(audit.checks.iter().any(|c| c.name == "firewall"));
        assert!(audit.checks.iter().any(|c| c.name == "admin"));
    }
}
