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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;

/// What a firmware answers. A profile is the difference between two models, and
/// the only place a new one has to be described.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The reference router: every section fills, host list included.
    H122373,
    /// A firmware without `wlan/host-list`: the panel must say so, not "0".
    NoHostList,
    /// `dhcp/settings` answers the values a DNS test writes, so a refused POST
    /// can still be read back as applied: the exact shape of the false success.
    DnsEcho,
    /// `POST dhcp/settings` is a whole-document write: a body without the other
    /// DHCP fields is refused with 100005 and nothing changes. The DNS bug's shape.
    DhcpStrict,
    /// The same short body is answered `OK` and dropped: the write claims success
    /// and the read-back disagrees.
    DhcpIgnoresShort,
    /// Every POST is answered `OK` and nothing is ever stored.
    DhcpIgnoresAll,
    /// The H122-373 band write: `POST net/net-mode` applies the payload and then
    /// answers `<code>100006</code>`. The device did what was asked; the code is
    /// noise. A tool that trusts the code calls this a failure.
    SwitchWarns,
    /// Same code, same body, but the new mask only shows up after `LATE_MS`:
    /// the radio is mid-restart when the first read-back lands.
    SwitchWarnsLate,
    /// The code without the change: a real refusal dressed as the same warning.
    SwitchRefuses,
}

/// How long `SwitchWarnsLate` takes to publish the new mask. Shorter than the
/// tool's own grace period, so a correct tool sees the change on its second read.
const LATE_MS: u64 = 600;

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
            Profile::H122373 | Profile::DnsEcho => routes.push(("/api/wlan/host-list", HOST_LIST)),
            Profile::DhcpStrict | Profile::DhcpIgnoresShort | Profile::DhcpIgnoresAll => {
                routes.push(("/api/wlan/host-list", HOST_LIST))
            }
            // A 404 is what a firmware that does not implement the endpoint
            // actually returns; the tool must read it as "not exposed".
            Profile::NoHostList => {}
            Profile::SwitchWarns | Profile::SwitchWarnsLate | Profile::SwitchRefuses => {}
        }
        if self == Profile::DnsEcho {
            for (path, body) in routes.iter_mut() {
                if *path == "/api/dhcp/settings" {
                    *body = DHCP_ECHO;
                }
            }
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
/// What a router that already carries the test's servers answers.
const DHCP_ECHO: &str = "<response><DnsStatus>0</DnsStatus><PrimaryDns>9.9.9.9</PrimaryDns><SecondaryDns>8.8.8.8</SecondaryDns></response>";
const SMS_REFUSED: &str = "<error><code>100005</code><message>No support</message></error>";
const HOST_LIST: &str = r#"<response><Hosts>
<Host><HostName>living-room-tv</HostName><IpAddress>192.168.8.101</IpAddress><MacAddress>02:00:00:00:00:01</MacAddress><WifiFrequency>5</WifiFrequency></Host>
<Host><HostName>phone-1</HostName><IpAddress>192.168.8.102</IpAddress><MacAddress>02:00:00:00:00:02</MacAddress><WifiFrequency>5</WifiFrequency></Host>
</Hosts></response>"#;

/// The band state a switch profile keeps between requests. The real firmware is
/// stateful in exactly this way, which is why a POST's answer and a later GET
/// can tell different stories.
#[derive(Debug, Clone)]
struct SwitchState {
    mode: String,
    band: String,
    lte: String,
    option: Option<String>,
    applied_at: Option<std::time::Instant>,
}

impl Default for SwitchState {
    fn default() -> Self {
        SwitchState {
            mode: "00".into(),
            band: "3FFFFFFF".into(),
            lte: "800C5".into(),
            option: None,
            applied_at: None,
        }
    }
}

impl SwitchState {
    fn body(&self) -> String {
        let opt = self
            .option
            .as_ref()
            .map(|o| format!("<NetworkOption>{o}</NetworkOption>"))
            .unwrap_or_default();
        format!(
            "<response><NetworkMode>{}</NetworkMode><NetworkBand>{}</NetworkBand><LTEBand>{}</LTEBand>{opt}</response>",
            self.mode, self.band, self.lte
        )
    }

    /// Read the four fields out of a real payload. Deliberately dumb string
    /// work: the point is to echo what the client sent, not to validate it.
    fn apply(&mut self, body: &str) {
        let tag = |name: &str| -> Option<String> {
            let open = format!("<{name}>");
            let close = format!("</{name}>");
            let start = body.find(&open)? + open.len();
            let end = body[start..].find(&close)? + start;
            Some(body[start..end].trim().to_string())
        };
        if let Some(m) = tag("NetworkMode") {
            self.mode = m;
        }
        if let Some(b) = tag("NetworkBand") {
            self.band = b;
        }
        if let Some(l) = tag("LTEBand") {
            self.lte = l;
        }
        self.option = tag("NetworkOption");
        self.applied_at = Some(std::time::Instant::now());
    }
}

/// The `dhcp/settings` document a DHCP profile keeps. Field names and order copy
/// what a live H122-373 answers, because the firmware validates names, not values.
#[derive(Debug, Clone)]
struct DhcpState {
    dns_status: String,
    primary: String,
    secondary: String,
}

impl Default for DhcpState {
    fn default() -> Self {
        DhcpState {
            dns_status: "1".into(),
            primary: "192.168.8.1".into(),
            secondary: "192.168.8.1".into(),
        }
    }
}

impl DhcpState {
    fn body(&self) -> String {
        format!(
            "<response><DnsStatus>{}</DnsStatus><DhcpStartIPAddress>192.168.8.100</DhcpStartIPAddress>\
             <DhcpIPAddress>192.168.8.1</DhcpIPAddress><accessipaddress></accessipaddress>\
             <homeurl>homerouter.cpe</homeurl><DhcpStatus>1</DhcpStatus>\
             <DhcpLanNetmask>255.255.255.0</DhcpLanNetmask><SecondaryDns>{}</SecondaryDns>\
             <PrimaryDns>{}</PrimaryDns><DhcpEndIPAddress>192.168.8.200</DhcpEndIPAddress>\
             <DhcpLeaseTime>86400</DhcpLeaseTime></response>",
            self.dns_status, self.secondary, self.primary
        )
    }

    /// Every field the real firmware insists on. A body missing one is not a write.
    fn complete(body: &str) -> bool {
        [
            "DhcpIPAddress",
            "DhcpLanNetmask",
            "DhcpStatus",
            "DhcpStartIPAddress",
            "DhcpEndIPAddress",
            "DhcpLeaseTime",
            "DnsStatus",
            "PrimaryDns",
            "SecondaryDns",
        ]
        .iter()
        .all(|f| body.contains(&format!("<{f}>")))
    }

    fn apply(&mut self, body: &str) {
        let tag = |name: &str| -> Option<String> {
            let open = format!("<{name}>");
            let close = format!("</{name}>");
            let start = body.find(&open)? + open.len();
            let end = body[start..].find(&close)? + start;
            Some(body[start..end].trim().to_string())
        };
        if let Some(v) = tag("DnsStatus") {
            self.dns_status = v;
        }
        if let Some(v) = tag("PrimaryDns") {
            self.primary = v;
        }
        if let Some(v) = tag("SecondaryDns") {
            self.secondary = v;
        }
    }
}

/// A running fake router. Dropping it stops the listener.
pub struct MockCpe {
    addr: String,
    stop: Arc<AtomicBool>,
    logins: Arc<AtomicUsize>,
    logouts: Arc<AtomicUsize>,
    signals: Arc<AtomicUsize>,
    dhcp_posts: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
}

impl MockCpe {
    /// Bind an ephemeral port and answer `profile` until dropped.
    pub fn start(profile: Profile) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock cpe");
        let addr = listener.local_addr().expect("mock addr").to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let logins = Arc::new(AtomicUsize::new(0));
        let logouts = Arc::new(AtomicUsize::new(0));
        let signals = Arc::new(AtomicUsize::new(0));
        let (li, lo) = (Arc::clone(&logins), Arc::clone(&logouts));
        let sig = Arc::clone(&signals);
        let state = Arc::new(Mutex::new(SwitchState::default()));
        let st = Arc::clone(&state);
        let dhcp_posts = Arc::new(AtomicUsize::new(0));
        let dp = Arc::clone(&dhcp_posts);
        let dhcp = Arc::new(Mutex::new(DhcpState::default()));
        let dh = Arc::clone(&dhcp);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let counters = Counters {
                    logins: &li,
                    logouts: &lo,
                    signals: &sig,
                    dhcp_posts: &dp,
                };
                let _ = serve(stream, profile, &counters, &st, &dh);
            }
        });
        MockCpe {
            addr,
            stop,
            logins,
            logouts,
            signals,
            dhcp_posts,
            thread: Some(thread),
        }
    }

    /// `host:port`, the exact string a config's `ip` would carry.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Successful `POST /api/user/login` calls seen so far.
    pub fn logins(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }

    /// `POST /api/user/logout` calls seen so far.
    pub fn logouts(&self) -> usize {
        self.logouts.load(Ordering::SeqCst)
    }

    /// `GET /api/device/signal` calls seen so far: one per poll cycle, which is
    /// how a test sees the worker's actual interval.
    pub fn signals(&self) -> usize {
        self.signals.load(Ordering::SeqCst)
    }

    /// `POST /api/dhcp/settings` calls seen so far: how a test proves nothing was sent.
    pub fn dhcp_posts(&self) -> usize {
        self.dhcp_posts.load(Ordering::SeqCst)
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

/// Every counter the fake router keeps. One handle keeps `serve` under
/// `clippy::too_many_arguments`, which is a hard error here.
struct Counters<'a> {
    logins: &'a AtomicUsize,
    logouts: &'a AtomicUsize,
    signals: &'a AtomicUsize,
    dhcp_posts: &'a AtomicUsize,
}

fn serve(
    stream: TcpStream,
    profile: Profile,
    counters: &Counters,
    state: &Mutex<SwitchState>,
    dhcp: &Mutex<DhcpState>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/").to_string();

    if path.starts_with("/api/device/signal") {
        counters.signals.fetch_add(1, Ordering::SeqCst);
    }

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
    let mut payload = String::new();
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
        payload = String::from_utf8_lossy(&body).to_string();
    }

    // The three switch profiles keep their band state across requests: the POST
    // answer and the later GET have to be able to disagree, which is the whole
    // point of the case.
    let is_switch = matches!(
        profile,
        Profile::SwitchWarns | Profile::SwitchWarnsLate | Profile::SwitchRefuses
    );
    let is_dhcp = matches!(
        profile,
        Profile::DhcpStrict | Profile::DhcpIgnoresShort | Profile::DhcpIgnoresAll
    );

    let (status, body) = match (method, path.as_str()) {
        ("GET", "/api/dhcp/settings") if is_dhcp => {
            (200, dhcp.lock().map(|d| d.body()).unwrap_or_default())
        }
        ("POST", "/api/dhcp/settings") if is_dhcp => {
            counters.dhcp_posts.fetch_add(1, Ordering::SeqCst);
            if Profile::DhcpIgnoresAll == profile {
                (200, "<response>OK</response>".to_string())
            } else if !DhcpState::complete(&payload) {
                match profile {
                    // The real firmware's format error: the write did not happen.
                    Profile::DhcpStrict => (
                        200,
                        "<error><code>100005</code><message>format error</message></error>"
                            .to_string(),
                    ),
                    _ => (200, "<response>OK</response>".to_string()),
                }
            } else {
                if let Ok(mut d) = dhcp.lock() {
                    d.apply(&payload);
                }
                (200, "<response>OK</response>".to_string())
            }
        }
        ("POST", "/api/user/login") => {
            counters.logins.fetch_add(1, Ordering::SeqCst);
            (200, "<response>OK</response>".to_string())
        }
        ("POST", "/api/user/logout") => {
            counters.logouts.fetch_add(1, Ordering::SeqCst);
            (200, "<response>OK</response>".to_string())
        }
        ("POST", "/api/net/net-mode") if is_switch => {
            // SwitchRefuses answers the same code and changes nothing.
            if profile != Profile::SwitchRefuses {
                if let Ok(mut s) = state.lock() {
                    s.apply(&payload);
                }
            }
            (
                200,
                "<error><code>100006</code><message>Unknown error</message></error>".to_string(),
            )
        }
        ("GET", "/api/net/net-mode") if is_switch => {
            let late_pending = profile == Profile::SwitchWarnsLate
                && state
                    .lock()
                    .ok()
                    .and_then(|s| s.applied_at)
                    .is_some_and(|t| t.elapsed().as_millis() < LATE_MS as u128);
            let body = if late_pending {
                NET_MODE.to_string()
            } else {
                state
                    .lock()
                    .map(|s| s.body())
                    .unwrap_or_else(|_| NET_MODE.to_string())
            };
            (200, body)
        }
        ("GET", p) => match profile.routes().into_iter().find(|(route, _)| *route == p) {
            Some((_, body)) => (200, body.to_string()),
            None => (
                404,
                "<error><code>125002</code><message>not found</message></error>".to_string(),
            ),
        },
        _ => (
            404,
            "<error><code>125002</code><message>not found</message></error>".to_string(),
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

    /// The fake firmware has to be as strict as the real one, or the DNS tests
    /// below prove nothing: a three-field body is a format error, not a write.
    #[test]
    fn a_strict_dhcp_profile_refuses_a_three_field_document() {
        let mock = MockCpe::start(Profile::DhcpStrict);
        let mut c = HuaweiClient::new(mock.addr(), 2000);
        c.login("admin", "demo").expect("mock login");
        let short = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><DnsStatus>1</DnsStatus><PrimaryDns>1.1.1.1</PrimaryDns><SecondaryDns>1.0.0.1</SecondaryDns></request>";
        let err = c.post("dhcp/settings", short).expect_err("must refuse");
        assert_eq!(err, crate::error::ApiError::Invalid(100005));
        assert_eq!(mock.dhcp_posts(), 1);
        let doc = c.get("dhcp/settings").expect("read back");
        assert_eq!(
            doc.value("PrimaryDns"),
            Some("192.168.8.1"),
            "nothing may change"
        );
    }
}
