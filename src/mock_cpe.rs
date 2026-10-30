//! A fake Huawei CPE that lives inside the test binary.
//!
//! `fixtures/` starts *after* the parser, so the whole transport half of the
//! tool - the session handshake, cookie and token juggling, HTTP status mapping,
//! firmware error codes - had no test at all. This module puts a real TCP
//! listener behind `127.0.0.1:0` and answers the endpoints a firmware exposes,
//! per profile, so the client and `headless::collect` are exercised end to end
//! without a router and without a new dependency.
//!
//! It also caught two defects nothing else could: `doctor` asking `GET /api//`
//! (a path no firmware promises) and a 404 on `wlan/host-list` being drawn as
//! "0 clients". Both are now held by tests in this file.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// What a firmware answers. A profile is the difference between two models, and
/// the only place a new one has to be described.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The reference router: every section fills, host list included.
    H122373,
    /// A firmware without `wlan/host-list`: the panel must say so, not "0".
    NoHostList,
}

impl Profile {
    fn routes(self) -> Vec<(&'static str, &'static str)> {
        let mut routes = vec![
            ("/", ROOT_HTML),
            ("/api/user/state-login", STATE_LOGIN),
            ("/api/webserver/SesTokInfo", SES_TOK),
            ("/api/device/signal", SIGNAL),
            ("/api/monitoring/status", STATUS),
            ("/api/monitoring/traffic-statistics", TRAFFIC),
            ("/api/net/current-plmn", PLMN),
            ("/api/device/information", DEVICE),
            ("/api/net/net-mode", NET_MODE),
            ("/api/net/net-mode-list", NET_MODE_LIST),
            ("/api/dhcp/settings", DHCP),
            ("/api/sms/sms-count", SMS_REFUSED),
        ];
        match self {
            Profile::H122373 => routes.push(("/api/wlan/host-list", HOST_LIST)),
            // A 404 is what a firmware that does not implement the endpoint
            // actually returns; the tool must read it as "not exposed".
            Profile::NoHostList => {}
        }
        routes
    }
}

const ROOT_HTML: &str =
    r#"<html><head><meta name="csrf_token" content="mocktoken"></head><body>login</body></html>"#;
const STATE_LOGIN: &str = "<response><State>0</State><password_type>4</password_type></response>";
const SES_TOK: &str =
    "<response><SesInfo>SessionID=mock</SesInfo><TokInfo>mocktoken</TokInfo></response>";
const SIGNAL: &str = r#"<response>
<pci>481</pci><cell_id>0000000-000</cell_id><rsrq>-6.0dB</rsrq><rsrp>-95dBm</rsrp>
<rssi>-67dBm</rssi><sinr>3dB</sinr><mode>7</mode><ulbandwidth>20MHz</ulbandwidth>
<dlbandwidth>20MHz</dlbandwidth><earfcn>DL:100 UL:18100</earfcn><band>1</band>
<plmn>00101</plmn><transmode>TM[4]</transmode><tdd></tdd>
<nrrsrp></nrrsrp><nrsinr></nrsinr><nrdlmcs></nrdlmcs>
</response>"#;
const STATUS: &str = r#"<response>
<ConnectionStatus>901</ConnectionStatus><SignalIcon>4</SignalIcon><SignalIconNr>0</SignalIconNr>
<CurrentNetworkType>19</CurrentNetworkType><RoamingStatus>0</RoamingStatus>
<TotalWifiClients>3</TotalWifiClients><CurrentWifiUser>3</CurrentWifiUser>
<WanIPAddress>203.0.113.77</WanIPAddress><PrimaryDns>203.0.113.1</PrimaryDns>
<SecondaryDns>203.0.113.2</SecondaryDns><CurrentPlmnName>Mock Net</CurrentPlmnName>
<CurrentPlmnCode>00101</CurrentPlmnCode><SimState>1</SimState><Uptime>96600</Uptime>
</response>"#;
const TRAFFIC: &str = r#"<response>
<CurrentConnectTime>4000</CurrentConnectTime><CurrentUpload>3800000</CurrentUpload>
<CurrentDownload>29000000</CurrentDownload><TotalUpload>1690000000000</TotalUpload>
<TotalDownload>49990000000000</TotalDownload>
</response>"#;
const PLMN: &str = "<response><State>0</State><FullName>Mock Net</FullName><Numeric>00101</Numeric><ShortName>MOCK</ShortName><Rat>7</Rat></response>";
const DEVICE: &str = r#"<response>
<DeviceName>Mock-CPE</DeviceName><DeviceType>Mock-CPE</DeviceType>
<SoftwareVersion>11.0.2.21</SoftwareVersion><HardwareVersion>WL1MOCKM</HardwareVersion>
<MacAddress1>02:00:00:00:0A:01</MacAddress1><SerialNumber>MOCKSERIAL0001</SerialNumber>
<Imei>000000000000000</Imei>
</response>"#;
const NET_MODE: &str =
    "<response><NetworkMode>19</NetworkMode><NetworkBand>3FFFFFFF</NetworkBand><LTEBand>800C5</LTEBand></response>";
const NET_MODE_LIST: &str =
    "<response><BandList><Band>1</Band><Band>3</Band><Band>5</Band></BandList></response>";
const DHCP: &str = "<response><PrimaryDns>203.0.113.1</PrimaryDns><SecondaryDns>203.0.113.2</SecondaryDns><Domains>mock.lan</Domains></response>";
const SMS_REFUSED: &str = "<error><code>100005</code><message>No support</message></error>";
const HOST_LIST: &str = r#"<response><Hosts>
<Host><HostName>living-room-tv</HostName><IpAddress>192.168.8.101</IpAddress><MacAddress>02:00:00:00:00:01</MacAddress><WifiFrequency>5</WifiFrequency></Host>
<Host><HostName>phone-1</HostName><IpAddress>192.168.8.102</IpAddress><MacAddress>02:00:00:00:00:02</MacAddress><WifiFrequency>5</WifiFrequency></Host>
</Hosts></response>"#;

/// A running fake router. Dropping it stops the listener.
pub struct MockCpe {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockCpe {
    /// Bind an ephemeral port and answer `profile` until dropped.
    pub fn start(profile: Profile) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock cpe");
        let addr = listener.local_addr().expect("mock addr").to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let _ = serve(stream, profile);
            }
        });
        MockCpe {
            addr,
            stop,
            thread: Some(thread),
        }
    }

    /// `host:port`, the exact string a config's `ip` would carry.
    pub fn addr(&self) -> &str {
        &self.addr
    }
}

impl Drop for MockCpe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the accept loop: one more connection is cheaper than a
        // non-blocking dance, and the flag makes the loop exit after it.
        let _ = TcpStream::connect(&self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(stream: TcpStream, profile: Profile) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/").to_string();

    // Drain headers so the client is not left writing into a closed socket.
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
    }

    let (status, body) = match (method, path.as_str()) {
        ("POST", "/api/user/login") => (200, "<response>OK</response>"),
        ("POST", "/api/user/logout") => (200, "<response>OK</response>"),
        ("GET", p) => match profile.routes().into_iter().find(|(route, _)| *route == p) {
            Some((_, body)) => (200, body),
            None => (
                404,
                "<error><code>125002</code><message>not found</message></error>",
            ),
        },
        _ => (
            404,
            "<error><code>125002</code><message>not found</message></error>",
        ),
    };

    let mut stream = stream;
    let response = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        if status == 200 { "OK" } else { "Not Found" },
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless;
    use crate::router::HuaweiClient;

    fn signed_in(profile: Profile) -> (MockCpe, HuaweiClient) {
        let mock = MockCpe::start(profile);
        let mut client = HuaweiClient::new(mock.addr(), 2000);
        client.login("admin", "demo").expect("mock login");
        (mock, client)
    }

    /// A model that answers everything: the whole snapshot fills, and nothing is
    /// reported as unsupported.
    #[test]
    fn a_full_profile_produces_a_complete_snapshot() {
        let (mock, mut client) = signed_in(Profile::H122373);
        let snap = headless::collect(&mut client, mock.addr()).expect("collect");

        assert_eq!(snap.signal.rsrp_dbm, Some(-95.0));
        assert_eq!(snap.connection.net_type, "LTE");
        assert_eq!(snap.clients.len(), 2, "host list was not parsed");
        assert_eq!(snap.device.software.as_deref(), Some("11.0.2.21"));
        assert!(snap.unsupported.is_empty(), "{:?}", snap.unsupported);
    }

    /// The bug the mock found: no `wlan/host-list` used to mean `clients: []`
    /// with no explanation, which the panel drew as "0 clients".
    #[test]
    fn a_hidden_client_list_is_recorded_not_zeroed() {
        let (mock, mut client) = signed_in(Profile::NoHostList);
        let snap = headless::collect(&mut client, mock.addr()).expect("collect");

        assert!(snap.clients.is_empty());
        assert_eq!(snap.unsupported, vec!["clients".to_string()]);
        // Every other read still worked: one gap is not a broken scan.
        assert_eq!(snap.signal.rsrp_dbm, Some(-95.0));
    }

    /// The other bug: reachability asked `GET /api//`, which a strict server
    /// answers 404, and `doctor` then stopped before probing anything.
    #[test]
    fn doctor_reaches_the_mock_and_keeps_probing() {
        let mock = MockCpe::start(Profile::NoHostList);
        let mut cfg = crate::config::Config::default();
        cfg.router.ip = mock.addr().to_string();

        let report = headless::doctor(&cfg, "/tmp/mock/config.toml", true, Some("demo"));

        let reach = report
            .capabilities
            .iter()
            .find(|c| c.id == "reachability")
            .expect("reachability capability");
        assert_eq!(reach.state, "ready", "{:?}", reach.detail);
        assert_eq!(reach.reason, "http_answered");

        // Probe results exist only after auth succeeded, so their presence is
        // proof that the doubled slash no longer stops the walk.
        assert!(
            report.capabilities.len() > 10,
            "doctor stopped early: {} capabilities",
            report.capabilities.len()
        );
        assert!(
            report
                .capabilities
                .iter()
                .any(|c| c.id == "auth" && c.state == "ready"),
            "auth never ran"
        );
    }
}
