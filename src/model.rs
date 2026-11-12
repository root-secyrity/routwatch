use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Nr {
    pub rsrp_dbm: Option<f32>,
    pub rsrq_db: Option<f32>,
    pub sinr_db: Option<f32>,
    pub band: Option<String>,
    pub dl_mhz: Option<u32>,
    pub ul_mhz: Option<u32>,
    pub dl_mcs: Option<u8>,
    pub ul_mcs: Option<u8>,
    /// `nrearfcn`, kept as the firmware writes it: `DL:528980 UL:528980`.
    pub earfcn: Option<String>,
    /// `nrdlbandwidth` / `nrulbandwidth`, kept as text: `100MHz`.
    pub dl_bw: Option<String>,
    pub ul_bw: Option<String>,
    /// `nrrank`: MIMO layers, 1..=8. Anything else is not a reading.
    pub rank: Option<u8>,
    /// `nrbler` as a percentage, 0..=100.
    pub bler_pct: Option<f32>,
    /// `nrcqi0`/`nrcqi1`, 0..=15; 127 is the firmware's padding, not a reading.
    pub cqi: Option<u8>,
    /// `nrtxpower` in dBm when the firmware sends a bare number.
    pub txpower_dbm: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Signal {
    pub rsrp_dbm: Option<f32>,
    pub rsrq_db: Option<f32>,
    pub sinr_db: Option<f32>,
    pub rssi_dbm: Option<f32>,
    pub band: Option<String>,
    pub dl_mhz: Option<u32>,
    pub ul_mhz: Option<u32>,
    pub dl_bw: Option<String>,
    pub ul_bw: Option<String>,
    pub earfcn: Option<String>,
    pub pci: Option<u32>,
    pub cell_id: Option<String>,
    pub enodeb: Option<String>,
    pub tac: Option<u32>,
    pub tm: Option<String>,
    pub dl_mcs: Option<u8>,
    /// Second DL carrier when carrier aggregation is active (mcsDownCarrier1Code1).
    pub dl_mcs_secondary: Option<u8>,
    pub ul_mcs: Option<u8>,
    pub cqi: Option<u8>,
    pub pusch_dbm: Option<f32>,
    /// None means this firmware exposes no NR fields - never render zeros instead.
    pub nr: Option<Nr>,
}

/// The router's own monthly accounting, when the firmware exposes it.
/// `month_dl_bytes` and `month_ul_bytes` are the counters behind
/// `monitoring/month_statistics`; `start_day` comes from
/// `monitoring/start_date` and defaults to 1 when that endpoint is refused.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Quota {
    pub month_dl_bytes: u64,
    pub month_ul_bytes: u64,
    /// Billing cycle start day of month, 1..=28.
    pub start_day: u8,
    pub data_limit_bytes: Option<u64>,
    /// `MonthLastClearTime` exactly as the firmware writes it.
    pub last_clear: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Traffic {
    pub dl_rate_bytes: u64,
    pub ul_rate_bytes: u64,
    pub session_dl_bytes: u64,
    pub session_ul_bytes: u64,
    pub total_dl_bytes: u64,
    pub total_ul_bytes: u64,
    pub session_secs: u64,
    pub total_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Connection {
    pub status_code: u16,
    pub status: String,
    pub net_type: String,
    pub net_type_ex: String,
    pub endc: bool,
    pub endc_restricted: bool,
    pub signal_bars: u8,
    pub nr_bars: u8,
    pub roaming: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Plmn {
    pub name: String,
    pub short: String,
    pub numeric: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Device {
    pub name: Option<String>,
    pub serial: Option<String>,
    pub imei: Option<String>,
    pub hardware: Option<String>,
    pub software: Option<String>,
    pub webui: Option<String>,
    pub wan_ip: Option<String>,
    pub wan_ipv6: Option<String>,
    pub mac: Option<String>,
    pub uptime_secs: Option<u64>,
    pub workmode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Client {
    pub name: Option<String>,
    pub ip: Option<String>,
    pub mac: Option<String>,
    pub freq: Option<String>,
    pub ssid: Option<String>,
    pub associated_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetMode {
    pub network_mode: Option<String>,
    pub network_band: Option<String>,
    pub lte_band: Option<String>,
    pub network_option: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmsMessage {
    pub index: Option<String>,
    pub phone: Option<String>,
    pub content: Option<String>,
    pub date: Option<String>,
    pub unread: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PinStatus {
    pub sim_state: Option<String>,
    pub pin_state: Option<String>,
    pub pin_retries: Option<String>,
    pub puk_retries: Option<String>,
}

impl PinStatus {
    /// 258/259/260 mean the SIM wants a PIN/PUK on Huawei firmware; 257 is ready.
    pub fn locked(&self) -> bool {
        matches!(
            self.sim_state.as_deref(),
            Some("258") | Some("259") | Some("260")
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceExtras {
    pub pin: Option<PinStatus>,
    pub mobile_data: Option<bool>,
    pub sms_inbox: Vec<SmsMessage>,
    /// Why the inbox could not be read (e.g. "permission denied (100005)").
    pub sms_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DnsInfo {
    /// `DnsStatus` exactly as the device reports it. The DHCP page uses `0` for two
    /// typed servers and `1` when the router answers itself (`js/dhcp.js`), but it is
    /// not trustworthy across firmwares, so `is_manual()` decides from the addresses.
    pub status: Option<String>,
    pub primary: Option<String>,
    pub secondary: Option<String>,
    /// The router's own LAN address (`DhcpIPAddress`). Advertising it for both
    /// servers is what the DHCP panel calls automatic: the router resolves.
    pub lan_ip: Option<String>,
    /// Whether this firmware exposes the LAN DHCP DNS setting at all, read from
    /// `dhcp/feature-switch`. `None` means the device did not say (no such endpoint),
    /// which is an older firmware and is treated as writable, as before.
    pub editable: Option<bool>,
}

impl DnsInfo {
    /// True when the advertised servers are not the router itself.
    ///
    /// Decided by the addresses because `DnsStatus` is a flag, not a mode: the live box
    /// reports `1` while serving `192.168.8.1 / 192.168.8.1`. A firmware that hides its
    /// LAN IP falls back to the old reading *or* "the two servers differ" - custom servers
    /// almost never share an address, while the router's own pair always does. That
    /// covers `fixtures/sample-snapshot-5g.json` (status `1`, two custom servers, no
    /// `DhcpIPAddress`).
    pub fn is_manual(&self) -> bool {
        match (self.primary.as_deref(), self.secondary.as_deref()) {
            (Some(p), Some(s)) => match self.lan_ip.as_deref() {
                Some(lan) => !(p == lan && s == lan),
                None => self.status.as_deref() == Some("0") || p != s,
            },
            _ => self.status.as_deref() == Some("0"),
        }
    }
}

/// The `dhcp/settings` document as the firmware holds it.
///
/// `POST dhcp/settings` is a whole-document write: the firmware validates field
/// names, so a body carrying only the DNS trio is refused (100005) or ignored.
/// Reading it first and echoing it back is the only safe way to change one part.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DhcpSettings {
    pub dhcp_ip: Option<String>,
    pub netmask: Option<String>,
    pub dhcp_status: Option<String>,
    pub start_ip: Option<String>,
    pub end_ip: Option<String>,
    pub lease_time: Option<String>,
    pub dns_status: Option<String>,
    pub primary: Option<String>,
    pub secondary: Option<String>,
    pub homeurl: Option<String>,
    pub access_ip: Option<String>,
    /// The exact XML between `<response>` and `</response>`, as received. Empty when the
    /// document was built by hand (tests, mocks).
    pub raw_inner: String,
}

impl DhcpSettings {
    /// Attach the source text so the write can echo it byte for byte.
    pub fn with_raw(mut self, inner: &str) -> Self {
        self.raw_inner = inner.to_string();
        self
    }

    /// The whole document with the two DNS servers replaced.
    ///
    /// The raw text is echoed and only the two element bodies change, so an element this
    /// code does not model (the mock's `Domains`, or a field another firmware adds)
    /// survives. `DnsStatus` is copied from the device, never invented. When no raw text
    /// is available the eleven known fields are serialised instead.
    pub fn body_with_dns(&self, primary: &str, secondary: &str) -> String {
        let inner = if self.raw_inner.trim().is_empty() {
            self.known_fields_inner()
        } else {
            self.raw_inner.clone()
        };
        let inner = set_element(&inner, "PrimaryDns", primary);
        let inner = set_element(&inner, "SecondaryDns", secondary);
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><request>{inner}</request>")
    }

    /// Fallback body for a document with no source text: the eleven fields this code
    /// models, in the order the firmware reports them.
    fn known_fields_inner(&self) -> String {
        fn el(name: &str, value: Option<&String>) -> String {
            format!(
                "<{name}>{}</{name}>",
                crate::xml::esc(value.map(String::as_str).unwrap_or(""))
            )
        }
        let mut inner = String::new();
        inner.push_str(&el("DhcpIPAddress", self.dhcp_ip.as_ref()));
        inner.push_str(&el("DhcpLanNetmask", self.netmask.as_ref()));
        inner.push_str(&el("DhcpStatus", self.dhcp_status.as_ref()));
        inner.push_str(&el("DhcpStartIPAddress", self.start_ip.as_ref()));
        inner.push_str(&el("DhcpEndIPAddress", self.end_ip.as_ref()));
        inner.push_str(&el("DhcpLeaseTime", self.lease_time.as_ref()));
        inner.push_str(&el("DnsStatus", self.dns_status.as_ref()));
        inner.push_str(&el("PrimaryDns", self.primary.as_ref()));
        inner.push_str(&el("SecondaryDns", self.secondary.as_ref()));
        inner.push_str(&el("homeurl", self.homeurl.as_ref()));
        inner.push_str(&el("accessipaddress", self.access_ip.as_ref()));
        inner
    }
}

/// Replace one element's body, or append the element when the document lacks it.
///
/// Flat documents only, which is what this endpoint returns: the first `<name>` and the
/// first `</name>` are the pair to replace.
fn set_element(inner: &str, name: &str, value: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let escaped = crate::xml::esc(value);
    if let (Some(a), Some(b)) = (inner.find(&open), inner.find(&close)) {
        if b > a {
            let start = a + open.len();
            let mut out = String::with_capacity(inner.len() + escaped.len());
            out.push_str(&inner[..start]);
            out.push_str(&escaped);
            out.push_str(&inner[b..]);
            return out;
        }
    }
    let mut out = inner.to_string();
    out.push_str(&format!("{open}{escaped}{close}"));
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmsCount {
    pub unread: u32,
    pub inbox: u32,
    pub outbox: u32,
    pub draft: u32,
    pub max: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub generated_at: u64,
    pub router: String,
    pub signal: Signal,
    pub traffic: Traffic,
    pub connection: Connection,
    pub plmn: Plmn,
    pub device: Device,
    pub net_mode: NetMode,
    pub clients: Vec<Client>,
    pub sms: Option<SmsCount>,
    #[serde(default)]
    pub dns: Option<DnsInfo>,
    /// Router-reported temperature in °C. `None` means this firmware has no
    /// `device/temperature` endpoint - never render it as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// LTE bands this firmware reports as supported (read from net-mode-list).
    #[serde(default)]
    pub supported_bands: Vec<u32>,
    /// The device's own "all bands" mask - never a hardcoded constant.
    #[serde(default)]
    pub all_bands_mask: Option<String>,
    /// The network-level all-bands mask from `BandList`. On H122-373 this is the
    /// 32-bit `3FFFFFFF` the firmware itself echoes in `NetworkBand`, while
    /// `all_bands_mask` holds the 64-bit value from `LTEBandList`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_bands_mask: Option<String>,
    /// Ids of reads this firmware refused (`unsupported`, `denied`, or a plain
    /// 404). A panel that reads one says so; an empty list is omitted from
    /// `--json`, so nothing changes for a firmware that answers everything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = "<response><DnsStatus>1</DnsStatus><DhcpStartIPAddress>192.168.8.100</DhcpStartIPAddress><DhcpIPAddress>192.168.8.1</DhcpIPAddress><accessipaddress></accessipaddress><homeurl>homerouter.cpe</homeurl><DhcpStatus>1</DhcpStatus><DhcpLanNetmask>255.255.255.0</DhcpLanNetmask><SecondaryDns>192.168.8.1</SecondaryDns><PrimaryDns>192.168.8.1</PrimaryDns><DhcpEndIPAddress>192.168.8.200</DhcpEndIPAddress><DhcpLeaseTime>86400</DhcpLeaseTime></response>";

    #[test]
    fn a_dns_document_is_read_whole_and_written_whole() {
        let node = crate::xml::parse(LIVE).unwrap();
        let doc =
            crate::router::parse::parse_dhcp(&node).with_raw(&crate::xml::inner_of_document(LIVE));
        assert_eq!(doc.dhcp_ip.as_deref(), Some("192.168.8.1"));
        assert_eq!(doc.lease_time.as_deref(), Some("86400"));
        let body = doc.body_with_dns("1.1.1.1", "1.0.0.1");
        for field in [
            "DhcpIPAddress",
            "DhcpLanNetmask",
            "DhcpStatus",
            "DhcpStartIPAddress",
            "DhcpEndIPAddress",
            "DhcpLeaseTime",
            "DnsStatus",
            "PrimaryDns",
            "SecondaryDns",
            "homeurl",
            "accessipaddress",
        ] {
            assert!(
                body.contains(&format!("<{field}>")),
                "{field} missing: {body}"
            );
        }
        assert!(body.contains("<PrimaryDns>1.1.1.1</PrimaryDns>"), "{body}");
        assert!(
            body.contains("<DnsStatus>1</DnsStatus>"),
            "DnsStatus must be copied: {body}"
        );
        assert!(
            body.contains("<DhcpLeaseTime>86400</DhcpLeaseTime>"),
            "every element keeps the device's own bytes: {body}"
        );
    }

    /// An element this code does not model must survive the write, not be dropped by
    /// re-serialisation: that is the class of bug the whole-document fix exists for.
    #[test]
    fn an_unknown_element_survives_the_write() {
        let text = "<response><DnsStatus>1</DnsStatus><DhcpIPAddress>192.168.8.1</DhcpIPAddress><Domains>homerouter.cpe</Domains><PrimaryDns>192.168.8.1</PrimaryDns><SecondaryDns>192.168.8.1</SecondaryDns></response>";
        let node = crate::xml::parse(text).unwrap();
        let doc =
            crate::router::parse::parse_dhcp(&node).with_raw(&crate::xml::inner_of_document(text));
        let body = doc.body_with_dns("1.1.1.1", "1.0.0.1");
        assert!(body.contains("<Domains>homerouter.cpe</Domains>"), "{body}");
        assert!(body.contains("<PrimaryDns>1.1.1.1</PrimaryDns>"), "{body}");
        assert!(
            !body.contains("<PrimaryDns>192.168.8.1</PrimaryDns>"),
            "{body}"
        );
    }

    /// The live box answers DnsStatus=1 while serving the router itself: status is
    /// a flag, not a mode. Manual means "not the router's own address".
    #[test]
    fn manual_is_decided_by_the_addresses_not_by_dns_status() {
        let router_itself = DnsInfo {
            status: Some("1".into()),
            primary: Some("192.168.8.1".into()),
            secondary: Some("192.168.8.1".into()),
            lan_ip: Some("192.168.8.1".into()),
            editable: None,
        };
        assert!(!router_itself.is_manual());
        let custom = DnsInfo {
            primary: Some("1.1.1.1".into()),
            secondary: Some("1.0.0.1".into()),
            ..router_itself.clone()
        };
        assert!(custom.is_manual());
        let unknown_lan = DnsInfo {
            lan_ip: None,
            status: Some("0".into()),
            primary: Some("1.1.1.1".into()),
            secondary: Some("1.0.0.1".into()),
            editable: None,
        };
        assert!(
            unknown_lan.is_manual(),
            "old firmware keeps the old reading"
        );
    }
}
