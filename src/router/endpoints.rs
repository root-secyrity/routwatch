//! The endpoint map, and the capability probe that keeps the UI honest.
//!
//! Verified against a Huawei H122-373 (WEBUI 11.0.2.21). A firmware either
//! answers, or says 100003 (unsupported) / 100005 (denied) - the caller reports
//! that class instead of showing an empty value.

use crate::error::ApiError;
use crate::router::client::HuaweiClient;

pub struct Spec {
    pub group: &'static str,
    pub path: &'static str,
}

pub const PROBE: &[Spec] = &[
    Spec {
        group: "signal",
        path: "device/signal",
    },
    Spec {
        group: "device",
        path: "device/information",
    },
    Spec {
        group: "device",
        path: "device/basic_information",
    },
    Spec {
        group: "device",
        path: "device/temperature",
    },
    Spec {
        group: "status",
        path: "monitoring/status",
    },
    Spec {
        group: "traffic",
        path: "monitoring/traffic-statistics",
    },
    Spec {
        group: "traffic",
        path: "monitoring/check-notifications",
    },
    Spec {
        group: "network",
        path: "net/current-plmn",
    },
    Spec {
        group: "network",
        path: "net/net-mode",
    },
    Spec {
        group: "network",
        path: "net/net-mode-list",
    },
    Spec {
        group: "network",
        path: "net/cell-info",
    },
    Spec {
        group: "clients",
        path: "wlan/host-list",
    },
    Spec {
        group: "clients",
        path: "wlan/basic-settings",
    },
    Spec {
        group: "network",
        path: "dhcp/settings",
    },
    Spec {
        group: "network",
        path: "dialup/connection",
    },
    Spec {
        group: "network",
        path: "dialup/mobile-dataswitch",
    },
    Spec {
        group: "sim",
        path: "pin/status",
    },
    Spec {
        group: "sim",
        path: "pin/simlock",
    },
    Spec {
        group: "security",
        path: "security/firewall-switch",
    },
    Spec {
        group: "security",
        path: "security/mac-filter",
    },
    Spec {
        group: "device",
        path: "global/module-switch",
    },
    Spec {
        group: "device",
        path: "online-update/status",
    },
    Spec {
        group: "device",
        path: "language/current-language",
    },
    Spec {
        group: "device",
        path: "vpn/feature-switch",
    },
    Spec {
        group: "sms",
        path: "sms/sms-count",
    },
    Spec {
        group: "sms",
        path: "sms/config",
    },
    // Known-blocked on this firmware - probed so the UI can say why.
    Spec {
        group: "sms",
        path: "sms/sms-list",
    },
    Spec {
        group: "quota",
        path: "monitoring/month-statistics",
    },
    Spec {
        group: "quota",
        path: "monitoring/start-date",
    },
    Spec {
        group: "device",
        path: "device/antenna_status",
    },
    Spec {
        group: "device",
        path: "ota/status",
    },
    Spec {
        group: "device",
        path: "sntp/get-settings",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cap {
    Ok,
    Unsupported(u32),
    Denied(u32),
    Failed(String),
}

impl Cap {
    pub fn state(&self) -> &'static str {
        match self {
            Cap::Ok => "ready",
            Cap::Unsupported(_) => "unsupported",
            Cap::Denied(_) => "denied",
            Cap::Failed(_) => "failed",
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Cap::Ok => "answered".into(),
            Cap::Unsupported(c) => format!("firmware_error_{c}"),
            Cap::Denied(c) => format!("firmware_error_{c}"),
            Cap::Failed(_) => "call_failed".into(),
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            Cap::Ok => None,
            Cap::Unsupported(_) => Some("this firmware does not implement the endpoint".into()),
            Cap::Denied(_) => Some("reachable but refused for this session/user".into()),
            Cap::Failed(e) => Some(e.clone()),
        }
    }

    pub fn from_err(e: &ApiError) -> Cap {
        match e {
            ApiError::Unsupported(c) => Cap::Unsupported(*c),
            ApiError::Denied(c) => Cap::Denied(*c),
            other => Cap::Failed(other.to_string()),
        }
    }
}

pub struct ProbeResult {
    pub group: &'static str,
    pub path: &'static str,
    pub cap: Cap,
}

pub fn probe_all(c: &mut HuaweiClient) -> Vec<ProbeResult> {
    PROBE
        .iter()
        .map(|spec| {
            let cap = match c.get(spec.path) {
                Ok(_) => Cap::Ok,
                Err(e) => Cap::from_err(&e),
            };
            ProbeResult {
                group: spec.group,
                path: spec.path,
                cap,
            }
        })
        .collect()
}
