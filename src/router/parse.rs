//! XML -> typed structs. Every numeric field is Option: a missing or `" "`
//! placeholder value must stay absent, never become 0.

use crate::model::*;
use crate::xml::Node;

fn f32_of(n: &Node, key: &str) -> Option<f32> {
    let v = n.value(key)?;
    let cleaned: String = v
        .trim_start_matches(">=")
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
        .collect();
    cleaned.parse().ok()
}

fn u32_of(n: &Node, key: &str) -> Option<u32> {
    n.value(key)?.trim().parse().ok()
}

fn u64_of(n: &Node, key: &str) -> Option<u64> {
    n.value(key)?.trim().parse().ok()
}

fn s_of(n: &Node, key: &str) -> Option<String> {
    n.value(key).map(|v| v.trim().to_string())
}

/// "2120000kHz" -> 2120 ; "19300" -> 19300 (as-is when no unit)
fn freq_mhz(n: &Node, key: &str) -> Option<u32> {
    let raw = n.value(key)?;
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    let v: u32 = digits.parse().ok()?;
    if raw.to_ascii_lowercase().contains("khz") {
        Some(v / 1000)
    } else {
        Some(v)
    }
}

/// MCS values arrive packed, one number per carrier:
/// "mcsDownCarrier1Code0:15 mcsDownCarrier1Code1:0" -> [15, 0]
/// "mcsUpCarrier1:17"                             -> [17]
/// Values are kept in carrier order: index 0 is the primary carrier, which is
/// what the router's own page shows as "DL MCS".
fn mcs_values(n: &Node, keys: &[&str]) -> Vec<u8> {
    for key in keys {
        let Some(raw) = n.value(key) else { continue };
        let mut out = Vec::new();
        let bytes = raw.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                // A number right after ':' or '=' is a code value; keep the rest
                // of "Carrier1Code1" digits out by requiring the prefix.
                let prev = raw[..start].chars().last();
                if matches!(prev, Some(':') | Some('=')) {
                    if let Ok(v) = raw[start..i].parse::<u8>() {
                        out.push(v);
                    }
                }
            } else {
                i += 1;
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    Vec::new()
}

fn mcs_primary(values: &[u8]) -> Option<u8> {
    values.first().copied()
}

fn mcs_secondary(values: &[u8]) -> Option<u8> {
    values.get(1).copied()
}

fn nr_value(n: &Node, keys: &[&str]) -> Option<f32> {
    for k in keys {
        if let Some(v) = f32_of(n, k) {
            return Some(v);
        }
    }
    None
}

pub fn parse_signal(n: &Node) -> Signal {
    let band = n.value("band").map(|b| {
        if b.starts_with('B') {
            b.to_string()
        } else {
            format!("B{b}")
        }
    });

    let pusch = n.value("txpower").and_then(|t| {
        let idx = t.find("PPusch:")? + "PPusch:".len();
        let rest = &t[idx..];
        let digits: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-')
            .collect();
        digits.parse::<f32>().ok()
    });

    let nr_rsrp = nr_value(n, &["nrrsrp", "NRRSRP", "nr_rsrp"]);
    let nr_rsrq = nr_value(n, &["nrrsrq", "NRRSRQ", "nr_rsrq"]);
    let nr_sinr = nr_value(n, &["nrsinr", "NRSINR", "nr_sinr"]);
    let nr_dl = freq_mhz(n, "nrdlfreq").or_else(|| freq_mhz(n, "NRDLFREQ"));
    let nr_ul = freq_mhz(n, "nrulfreq").or_else(|| freq_mhz(n, "NRULFREQ"));
    let nr_dl_mcs = mcs_primary(&mcs_values(n, &["nrdlmcs", "NR_DL_MCS", "nr_dl_mcs"]));
    let nr_ul_mcs = mcs_primary(&mcs_values(n, &["nrulmcs", "NR_UL_MCS", "nr_ul_mcs"]));
    let nr_band = nr_dl.map(|mhz| match mhz {
        2496..=2690 => "N41".to_string(),
        3300..=3800 => "N78/N77".to_string(),
        3801..=4200 => "N77".to_string(),
        other => format!("n?({other}MHz)"),
    });

    let nr = if nr_rsrp.is_some()
        || nr_rsrq.is_some()
        || nr_sinr.is_some()
        || nr_dl.is_some()
        || nr_band.is_some()
    {
        Some(Nr {
            rsrp_dbm: nr_rsrp,
            rsrq_db: nr_rsrq,
            sinr_db: nr_sinr,
            band: nr_band,
            dl_mhz: nr_dl,
            ul_mhz: nr_ul,
            dl_mcs: nr_dl_mcs,
            ul_mcs: nr_ul_mcs,
        })
    } else {
        None
    };

    let dl_codes = mcs_values(n, &["dl_mcs", "DLMCS", "dlmcs", "nr_dl_mcs"]);
    let ul_codes = mcs_values(n, &["ul_mcs", "ULMCS", "ulmcs", "nr_ul_mcs"]);
    // CQI is 0..=15; the firmware pads the unused carrier with 127, which is not a
    // reading and must not be shown.
    let cqi = u32_of(n, "cqi0")
        .or_else(|| u32_of(n, "cqi1"))
        .filter(|v| *v <= 15)
        .map(|v| v as u8);

    Signal {
        rsrp_dbm: f32_of(n, "rsrp"),
        rsrq_db: f32_of(n, "rsrq"),
        sinr_db: f32_of(n, "sinr"),
        rssi_dbm: f32_of(n, "rssi"),
        band,
        dl_mhz: freq_mhz(n, "dlfrequency"),
        ul_mhz: freq_mhz(n, "ulfrequency"),
        dl_bw: s_of(n, "dlbandwidth"),
        ul_bw: s_of(n, "ulbandwidth"),
        earfcn: s_of(n, "earfcn"),
        pci: u32_of(n, "pci"),
        cell_id: s_of(n, "cell_id"),
        enodeb: s_of(n, "enodeb_id"),
        tac: u32_of(n, "tac"),
        tm: s_of(n, "transmode"),
        dl_mcs: mcs_primary(&dl_codes),
        dl_mcs_secondary: mcs_secondary(&dl_codes),
        ul_mcs: mcs_primary(&ul_codes),
        cqi,
        pusch_dbm: pusch,
        nr,
    }
}

pub fn parse_traffic(n: &Node) -> Traffic {
    Traffic {
        dl_rate_bytes: u64_of(n, "CurrentDownloadRate").unwrap_or(0),
        ul_rate_bytes: u64_of(n, "CurrentUploadRate").unwrap_or(0),
        session_dl_bytes: u64_of(n, "CurrentDownload").unwrap_or(0),
        session_ul_bytes: u64_of(n, "CurrentUpload").unwrap_or(0),
        total_dl_bytes: u64_of(n, "TotalDownload").unwrap_or(0),
        total_ul_bytes: u64_of(n, "TotalUpload").unwrap_or(0),
        session_secs: u64_of(n, "CurrentConnectTime").unwrap_or(0),
        total_secs: u64_of(n, "TotalConnectTime").unwrap_or(0),
    }
}

pub fn parse_connection(n: &Node) -> Connection {
    let status_code = u32_of(n, "ConnectionStatus").unwrap_or(0) as u16;
    let status = match status_code {
        900 => "connecting",
        901 => "connected",
        902 => "disconnected",
        905 => "no_connection",
        906 => "error",
        _ => "unknown",
    };
    let net_type = match n.value("CurrentNetworkType").unwrap_or("") {
        "19" | "101" => "LTE",
        "20" => "LTE_NR",
        "1011" => "LTE_CA",
        "111" => "NR",
        "0" | "" => "none",
        _ => "other",
    };
    Connection {
        status_code,
        status: status.to_string(),
        net_type: net_type.to_string(),
        net_type_ex: s_of(n, "CurrentNetworkTypeEx").unwrap_or_default(),
        endc: n.value("EndcStatus") == Some("1"),
        endc_restricted: n.value("endcRestrictedStatus") == Some("1"),
        signal_bars: u32_of(n, "SignalIcon").unwrap_or(0) as u8,
        nr_bars: u32_of(n, "SignalIconNr").unwrap_or(0) as u8,
        roaming: n.value("RoamingStatus") == Some("1"),
    }
}

pub fn parse_plmn(n: &Node) -> Plmn {
    Plmn {
        name: s_of(n, "FullName").unwrap_or_default(),
        short: s_of(n, "ShortName").unwrap_or_default(),
        numeric: s_of(n, "Numeric").unwrap_or_default(),
    }
}

pub fn parse_device(n: &Node) -> Device {
    Device {
        name: s_of(n, "DeviceName"),
        serial: s_of(n, "SerialNumber"),
        imei: s_of(n, "Imei"),
        hardware: s_of(n, "HardwareVersion"),
        software: s_of(n, "SoftwareVersion"),
        webui: s_of(n, "WebUIVersion"),
        wan_ip: s_of(n, "WanIPAddress"),
        wan_ipv6: s_of(n, "WanIPv6Address"),
        mac: s_of(n, "MacAddress1"),
        uptime_secs: u64_of(n, "uptime"),
        workmode: s_of(n, "workmode"),
    }
}

/// `device/temperature` answers `<temperature>41</temperature>` on firmwares that
/// have the sensor; a missing or absurd value is `None`, never zero.
pub fn parse_temperature(n: &Node) -> Option<f32> {
    f32_of(n, "temperature").filter(|t| *t > -40.0 && *t < 120.0)
}

pub fn parse_net_mode(n: &Node) -> NetMode {
    NetMode {
        network_mode: s_of(n, "NetworkMode"),
        network_band: s_of(n, "NetworkBand"),
        lte_band: s_of(n, "LTEBand"),
        network_option: s_of(n, "networkOption").or_else(|| s_of(n, "NetworkOption")),
    }
}

pub fn parse_clients(n: &Node) -> Vec<Client> {
    let Some(hosts) = n.find("Hosts") else {
        return Vec::new();
    };
    hosts
        .children_named("Host")
        .map(|h| Client {
            name: s_of(h, "HostName")
                .filter(|s| !s.is_empty())
                .or_else(|| s_of(h, "ActualName").filter(|s| !s.is_empty())),
            ip: s_of(h, "IpAddress")
                .map(|v| v.split(';').next().unwrap_or("").to_string())
                .filter(|s| !s.is_empty()),
            mac: s_of(h, "MacAddress"),
            freq: s_of(h, "Frequency"),
            ssid: s_of(h, "AssociatedSsid"),
            associated_secs: u64_of(h, "AssociatedTime").unwrap_or(0),
        })
        .collect()
}

pub fn parse_sms_list(n: &Node) -> Vec<SmsMessage> {
    match n.find("Messages") {
        Some(container) => container
            .children_named("Message")
            .map(parse_message)
            .collect(),
        // some firmwares put Message directly under <response>
        None => n.children_named("Message").map(parse_message).collect(),
    }
}

fn parse_message(m: &Node) -> SmsMessage {
    SmsMessage {
        index: s_of(m, "Index"),
        phone: s_of(m, "Phone"),
        content: s_of(m, "Content"),
        date: s_of(m, "Date"),
        // Smstat 0 = unread, 1 = read
        unread: m.value("Smstat") == Some("0"),
    }
}

pub fn parse_pin(n: &Node) -> PinStatus {
    PinStatus {
        sim_state: s_of(n, "SimState"),
        pin_state: s_of(n, "PinOptState"),
        pin_retries: s_of(n, "SimPinTimes"),
        puk_retries: s_of(n, "SimPukTimes"),
    }
}

pub fn parse_mobile_data(n: &Node) -> bool {
    n.value("dataswitch") == Some("1")
}

pub fn parse_dns(n: &Node) -> DnsInfo {
    DnsInfo {
        status: s_of(n, "DnsStatus"),
        primary: s_of(n, "PrimaryDns"),
        secondary: s_of(n, "SecondaryDns"),
    }
}

pub fn parse_sms_count(n: &Node) -> SmsCount {
    SmsCount {
        unread: u32_of(n, "LocalUnread").unwrap_or(0),
        inbox: u32_of(n, "LocalInbox").unwrap_or(0),
        outbox: u32_of(n, "LocalOutbox").unwrap_or(0),
        draft: u32_of(n, "LocalDraft").unwrap_or(0),
        max: u32_of(n, "LocalMax").unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse;

    #[test]
    fn a_temperature_outside_the_sane_range_is_not_trusted() {
        let ok = parse("<response><temperature>41</temperature></response>").expect("xml");
        assert_eq!(parse_temperature(&ok), Some(41.0));

        let bogus = parse("<response><temperature>999</temperature></response>").expect("xml");
        assert_eq!(parse_temperature(&bogus), None);

        let absent = parse("<response></response>").expect("xml");
        assert_eq!(parse_temperature(&absent), None);
    }

    /// Scrubbed capture: the real device output with names, MACs, IPs
    /// and identifiers replaced. Kept inline so no private data ships.
    const SIGNAL_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<response>
<pci>481</pci>
<sc></sc>
<cell_id>0000000-000</cell_id>
<rsrq>-6.0dB</rsrq>
<rsrp>-95dBm</rsrp>
<rssi>-67dBm</rssi>
<sinr>3dB</sinr>
<rscp></rscp>
<ecio></ecio>
<mode>7</mode>
<ulbandwidth>20MHz</ulbandwidth>
<dlbandwidth>20MHz</dlbandwidth>
<txpower>PPusch:2dBm PPucch:2dBm PSrs:0dBm PPrach:1dBm</txpower>
<tdd></tdd>
<ul_mcs>mcsUpCarrier1:17</ul_mcs>
<dl_mcs>mcsDownCarrier1Code0:15 mcsDownCarrier1Code1:21</dl_mcs>
<earfcn>DL:100 UL:18100</earfcn>
<rrc_status>1</rrc_status>
<rac></rac>
<lac></lac>
<tac>516</tac>
<band>1</band>
<nei_cellid>No1:480No2:482No3:481No4:481</nei_cellid>
<plmn>00101</plmn>
<ims>0</ims>
<wdlfreq></wdlfreq>
<lteulfreq>19300</lteulfreq>
<ltedlfreq>21200</ltedlfreq>
<transmode>TM[4]</transmode>
<enodeb_id>0000000</enodeb_id>
<cqi0>11</cqi0>
<cqi1>127</cqi1>
<ulfrequency>1930000kHz</ulfrequency>
<dlfrequency>2120000kHz</dlfrequency>
<nrulbandwidth></nrulbandwidth>
<nrdlbandwidth></nrdlbandwidth>
<nrulmcs></nrulmcs>
<nrdlmcs></nrdlmcs>
<nrtxpower></nrtxpower>
<nrearfcn></nrearfcn>
<nrulfreq></nrulfreq>
<nrdlfreq></nrdlfreq>
<nrsinr></nrsinr>
<nrrsrp></nrrsrp>
<nrrsrq></nrrsrq>
<nrbler></nrbler>
<nrrank></nrrank>
<nrcqi0></nrcqi0>
<nrcqi1></nrcqi1>
<scc_pci></scc_pci>
<arfcn></arfcn>
<bsic></bsic>
<rxlev></rxlev>
</response>"#;
    /// Scrubbed capture: the real device output with names, MACs, IPs
    /// and identifiers replaced. Kept inline so no private data ships.
    const HOSTS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?><response><Hosts><Host><AssociatedTime>388587</AssociatedTime><HostName>living-room-tv</HostName><AssociatedSsid>TestNet</AssociatedSsid><ID>InternetGatewayDevice.LANDevice.1.Hosts.Host.6.</ID><Frequency>5GHz</Frequency><ActualName>living-room-tv</ActualName><MacAddress>02:00:00:00:00:01</MacAddress><IpAddress>192.168.8.101;fe80::1</IpAddress></Host><Host><AssociatedTime>0</AssociatedTime><HostName>phone-1</HostName><AssociatedSsid>TestNet</AssociatedSsid><ID>InternetGatewayDevice.LANDevice.1.Hosts.Host.30.</ID><Frequency>5GHz</Frequency><ActualName>phone-1</ActualName><MacAddress>02:00:00:00:00:02</MacAddress><IpAddress>192.168.8.102;fe80::2</IpAddress></Host><Host><AssociatedTime>41972</AssociatedTime><HostName></HostName><AssociatedSsid>TestNet</AssociatedSsid><ID>InternetGatewayDevice.LANDevice.1.Hosts.Host.47.</ID><Frequency>2.4GHz</Frequency><ActualName></ActualName><MacAddress>02:00:00:00:00:03</MacAddress><IpAddress>192.168.8.103;fe80::3</IpAddress></Host></Hosts></response>"#;
    /// Scrubbed capture: the real device output with names, MACs, IPs
    /// and identifiers replaced. Kept inline so no private data ships.
    const TRAFFIC_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<response>
<CurrentConnectTime>165315</CurrentConnectTime>
<CurrentUpload>7140630901</CurrentUpload>
<CurrentDownload>105453440768</CurrentDownload>
<CurrentDownloadRate>2525182</CurrentDownloadRate>
<CurrentUploadRate>20672</CurrentUploadRate>
<TotalUpload>1861346477605</TotalUpload>
<TotalDownload>55019061003290</TotalDownload>
<TotalConnectTime>87575369</TotalConnectTime>
<showtraffic>1</showtraffic>
</response>"#;

    fn doc() -> Node {
        parse(
            "<response>\
             <pci>481</pci><cell_id>0000000-000</cell_id><rsrq>-5.0dB</rsrq>\
             <rsrp>-96dBm</rsrp><rssi>>=-51dBm</rssi><sinr>3dB</sinr>\
             <mode>7</mode><ulbandwidth>20MHz</ulbandwidth><dlbandwidth>20MHz</dlbandwidth>\
             <txpower>PPusch:2dBm PPucch:1dBm</txpower>\
             <ul_mcs>mcsUpCarrier1:25</ul_mcs>\
             <dl_mcs>mcsDownCarrier1Code0:15 mcsDownCarrier1Code1:0</dl_mcs>\
             <earfcn>DL:100 UL:18100</earfcn><tac>516</tac><band>1</band>\
             <transmode>TM[4]</transmode><enodeb_id>0000000</enodeb_id>\
             <cqi0>11</cqi0><cqi1>127</cqi1>\
             <ulfrequency>1930000kHz</ulfrequency><dlfrequency>2120000kHz</dlfrequency>\
             <nrrsrp></nrrsrp><nrsinr> </nrsinr>\
             </response>",
        )
        .unwrap()
    }

    /// Parity lock: tests/fixtures/signal_h122.xml is a real capture from the
    /// H122-373, and the expected values below were produced by the ORIGINAL
    /// Electron app's parser on that same file (independent implementation).
    /// If this test fails, routwatch disagrees with the reference client.
    #[test]
    fn matches_reference_parser_on_real_capture() {
        let n = parse(SIGNAL_XML).unwrap();
        let s = parse_signal(&n);
        assert_eq!(s.rsrp_dbm, Some(-95.0));
        assert_eq!(s.rsrq_db, Some(-6.0));
        assert_eq!(s.sinr_db, Some(3.0));
        assert_eq!(s.rssi_dbm, Some(-67.0));
        assert_eq!(s.band.as_deref(), Some("B1"));
        assert_eq!(s.dl_mhz, Some(2120));
        assert_eq!(s.ul_mhz, Some(1930));
        assert_eq!(s.pci, Some(481));
        assert_eq!(s.tac, Some(516));
        assert_eq!(s.earfcn.as_deref(), Some("DL:100 UL:18100"));
        assert_eq!(s.dl_mcs, Some(15));
        assert_eq!(s.ul_mcs, Some(17));
        assert_eq!(s.cqi, Some(11));
        assert_eq!(s.tm.as_deref(), Some("TM[4]"));
        assert_eq!(s.cell_id.as_deref(), Some("0000000-000"));
        assert_eq!(s.enodeb.as_deref(), Some("0000000"));
        assert_eq!(s.dl_bw.as_deref(), Some("20MHz"));
        assert_eq!(s.ul_bw.as_deref(), Some("20MHz"));
        assert!(s.nr.is_none(), "this firmware exposes no NR fields");
    }

    /// Real NR strings captured on 2026-10-21 with ENDC active (mode=auto).
    /// This is why the gauges panel can show 5G when the link is genuinely 5G.
    #[test]
    fn parses_real_nr_fields() {
        let doc = parse(
            "<response><band>1</band><nrsinr>4dB</nrsinr><nrrsrp>-115dBm</nrrsrp><nrrsrq>-12.0dB</nrrsrq>\
             <nrdlmcs>NRmcsDownCarrier1Code0:28 NRmcsDownCarrier1Code1:0</nrdlmcs>\
             <nrulmcs>NRmcsUpCarrier1:12</nrulmcs><nrearfcn>DL:528980 UL:528980</nrearfcn>\
             <nrdlfreq>2644900kHz</nrdlfreq><nrulfreq>2644900kHz</nrulfreq>\
             <nrdlbandwidth>90MHz</nrdlbandwidth></response>",
        )
        .unwrap();
        let s = parse_signal(&doc);
        let nr = s.nr.expect("NR block must be present");
        assert_eq!(nr.rsrp_dbm, Some(-115.0));
        assert_eq!(nr.rsrq_db, Some(-12.0));
        assert_eq!(nr.sinr_db, Some(4.0));
        assert_eq!(nr.dl_mhz, Some(2644));
        assert_eq!(nr.ul_mhz, Some(2644));
        assert_eq!(
            nr.band.as_deref(),
            Some("N41"),
            "2644 MHz lands in band n41"
        );
        assert_eq!(nr.dl_mcs, Some(28), "Code0 is the primary NR carrier");
        assert_eq!(nr.ul_mcs, Some(12));
    }

    #[test]
    fn real_capture_yields_clients() {
        let n = parse(HOSTS_XML).unwrap();
        let clients = parse_clients(&n);
        assert!(!clients.is_empty(), "capture has associated hosts");
        assert!(clients.iter().all(|c| c.mac.is_some()));
        assert!(clients.iter().all(|c| c
            .ip
            .as_deref()
            .map(|ip| !ip.contains(';'))
            .unwrap_or(true)));
    }

    #[test]
    fn sms_list_and_pin_parse() {
        let doc = parse(
            "<response><Count>2</Count><Messages>\
             <Message><Smstat>0</Smstat><Index>40001</Index><Phone>+966500000000</Phone>\
             <Content>hello there</Content><Date>2026-10-20 10:00:00</Date></Message>\
             <Message><Smstat>1</Smstat><Index>40002</Index><Phone>+966511111111</Phone>\
             <Content>read one</Content><Date>2026-10-19 09:00:00</Date></Message>\
             </Messages></response>",
        )
        .unwrap();
        let msgs = parse_sms_list(&doc);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].unread && !msgs[1].unread);
        assert_eq!(msgs[0].phone.as_deref(), Some("+966500000000"));
        assert_eq!(msgs[0].content.as_deref(), Some("hello there"));

        let pin = parse(
            "<response><SimState>257</SimState><PinOptState>258</PinOptState>\
             <SimPinTimes>3</SimPinTimes><SimPukTimes>10</SimPukTimes></response>",
        )
        .unwrap();
        let p = parse_pin(&pin);
        assert_eq!(p.sim_state.as_deref(), Some("257"));
        assert!(!p.locked(), "257 = ready");
        let locked = parse("<response><SimState>258</SimState></response>").unwrap();
        assert!(parse_pin(&locked).locked());
    }

    #[test]
    fn real_capture_traffic_is_numeric() {
        let n = parse(TRAFFIC_XML).unwrap();
        let t = parse_traffic(&n);
        assert!(t.total_dl_bytes > 0);
        assert!(t.session_secs > 0);
    }

    #[test]
    fn signal_fields_parse_like_the_router() {
        let s = parse_signal(&doc());
        assert_eq!(s.rsrp_dbm, Some(-96.0));
        assert_eq!(s.rsrq_db, Some(-5.0));
        assert_eq!(s.sinr_db, Some(3.0));
        assert_eq!(s.rssi_dbm, Some(-51.0));
        assert_eq!(s.band.as_deref(), Some("B1"));
        assert_eq!(s.dl_mhz, Some(2120));
        assert_eq!(s.ul_mhz, Some(1930));
        assert_eq!(
            s.dl_mcs,
            Some(15),
            "primary carrier = Code0, like the router page"
        );
        assert_eq!(s.dl_mcs_secondary, Some(0));
        assert_eq!(s.ul_mcs, Some(25));
        assert_eq!(s.cqi, Some(11));
        assert_eq!(s.pusch_dbm, Some(2.0));
        assert_eq!(s.tac, Some(516));
        assert!(s.nr.is_none(), "empty NR fields must stay absent");
    }
}
