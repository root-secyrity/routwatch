use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Nr {
    pub rsrp_dbm: Option<f32>,
    pub rsrq_db: Option<f32>,
    pub sinr_db: Option<f32>,
    pub band: Option<String>,
    pub dl_mhz: Option<u32>,
    pub ul_mhz: Option<u32>,
    pub dl_mcs: Option<u8>,
    pub ul_mcs: Option<u8>,
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
    /// None means this firmware exposes no NR fields — never render zeros instead.
    pub nr: Option<Nr>,
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
    /// "0" = manual servers, "1" = automatic (from the DHCP server).
    pub status: Option<String>,
    pub primary: Option<String>,
    pub secondary: Option<String>,
}

impl DnsInfo {
    pub fn is_manual(&self) -> bool {
        self.status.as_deref() == Some("0")
    }
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
    /// `device/temperature` endpoint — never render it as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// LTE bands this firmware reports as supported (read from net-mode-list).
    #[serde(default)]
    pub supported_bands: Vec<u32>,
    /// The device's own "all bands" mask — never a hardcoded constant.
    #[serde(default)]
    pub all_bands_mask: Option<String>,
}
