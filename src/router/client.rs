//! Huawei web-API client.
//!
//! Blocking by design: the router serialises requests anyway (~85 ms each, a
//! 5-endpoint poll measured 432 ms), so one serialized session costs nothing and
//! removes token-rotation races. Every timeout is set explicitly - ureq defaults
//! all of them to `None` (no limit at all).

use crate::error::{classify, ApiError};
use crate::router::bands::BandPayload;
use crate::router::parse;
use crate::xml::{self, Node};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;
use ureq::{Agent, Error as UreqError};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

/// The whole budget Drop may spend closing a session: one connect and one
/// request. The firmware frees a session by itself after 90 to 150 s idle
/// (measured 2026-11-22), so a missed logout costs delay, never correctness.
const BEST_EFFORT_LOGOUT_MS: u64 = 2_000;

pub struct HuaweiClient {
    agent: Agent,
    base: String,
    timeout_ms: u64,
    /// name -> value. Replaced by name, never appended: a duplicated SessionID
    /// in the Cookie header makes the firmware drop the session (error 125002).
    cookies: BTreeMap<String, String>,
    tokens: Vec<String>,
    authed: bool,
}

fn build_agent(timeout_ms: u64) -> Agent {
    let config = Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(1)))
        .timeout_global(Some(Duration::from_millis(timeout_ms)))
        .timeout_recv_body(Some(Duration::from_secs(2)))
        .http_status_as_error(false)
        // A router never legitimately redirects an API call. Following one
        // carried `__RequestVerificationToken` to whatever host `Location`
        // named, so refuse instead.
        .max_redirects(0)
        .build();
    config.into()
}

/// The `sms/sms-list` body. `Ascending` is mandatory: without it the firmware
/// answers 100005 (ERROR_FORMAT_ERROR) and the inbox reads as refused.
/// `BoxType` 1 is the inbox, as measured on H122-373 firmware 11.0.2.21
/// (2026-11-18); 2 and 3 are Huawei's outbox and draft, unverified here.
pub(crate) fn sms_list_body(page: u32, box_type: u8) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><PageIndex>{page}</PageIndex><ReadCount>{SMS_PAGE}</ReadCount><BoxType>{box_type}</BoxType><SortType>0</SortType><Ascending>0</Ascending><UnreadPreferred>0</UnreadPreferred></request>"
    )
}

/// Rows asked for per page, and therefore the page size the reader reasons
/// with. One number, because the request and the range label must agree.
pub const SMS_PAGE: u32 = 20;

/// Huawei login codes: 108001 username, 108002 password, 108003 session already
/// open, 108006 both, 108007 too many attempts.
pub(crate) fn login_hint(code: u32) -> &'static str {
    match code {
        108001 => "wrong username",
        108002 => "wrong password",
        108003 => "a session is already open; retry in a moment",
        108006 => "wrong username or password",
        108007 => "too many login attempts; wait before retrying",
        _ => "router refused the login",
    }
}

fn map_ureq(e: UreqError) -> ApiError {
    match e {
        UreqError::StatusCode(code) => ApiError::Http(code),
        other => ApiError::Network(other.to_string()),
    }
}

impl HuaweiClient {
    pub fn new(ip: &str, timeout_ms: u64) -> Self {
        HuaweiClient {
            agent: build_agent(timeout_ms),
            base: format!("http://{ip}"),
            timeout_ms,
            cookies: BTreeMap::new(),
            tokens: Vec::new(),
            authed: false,
        }
    }

    pub fn is_authed(&self) -> bool {
        self.authed
    }

    fn cookie_header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            None
        } else {
            Some(
                self.cookies
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// `/api/<endpoint>`, with the slash between the two owned in exactly one
    /// place: `raw("/")` must build `/api/`, never `/api//`, which a strict
    /// server answers with 404. Callers that pass "" or "/" mean `/api/`.
    fn api_path(endpoint: &str) -> String {
        let trimmed = endpoint.trim_matches('/');
        if trimmed.is_empty() {
            "/api/".to_string()
        } else {
            format!("/api/{trimmed}")
        }
    }

    /// The web UI itself. Reachability asks for this, not for an `/api/` path:
    /// every Huawei firmware serves its login page, and none of them promises
    /// anything at `/api/`.
    pub fn root(&mut self) -> Result<String, ApiError> {
        self.raw_get("/")
    }

    /// Keep the session cookie and the rotating verification tokens in sync.
    fn absorb_headers(&mut self, headers: &ureq::http::HeaderMap) {
        for v in headers.get_all("set-cookie").iter() {
            if let Ok(s) = v.to_str() {
                let kv = s.split(';').next().unwrap_or("").trim();
                if let Some((name, value)) = kv.split_once('=') {
                    let name = name.trim();
                    if !name.is_empty() {
                        self.cookies
                            .insert(name.to_string(), value.trim().to_string());
                    }
                }
            }
        }

        let header = |name: &str| -> Option<String> {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        if let Some(one) = header("__requestverificationtokenone") {
            self.tokens = vec![one];
            if let Some(two) = header("__requestverificationtokentwo") {
                self.tokens.push(two);
            }
        } else if let Some(single) = header("__requestverificationtoken") {
            self.tokens = vec![single];
        }
    }

    fn raw_get(&mut self, path: &str) -> Result<String, ApiError> {
        let mut req = self.agent.get(&self.url(path)).header("User-Agent", UA);
        if let Some(c) = self.cookie_header() {
            req = req.header("Cookie", &c);
        }
        if self.tokens.len() == 1 {
            req = req.header("__RequestVerificationToken", &self.tokens[0]);
        }
        let resp = req.call().map_err(map_ureq)?;
        let status = resp.status().as_u16();
        self.absorb_headers(resp.headers());
        let body = resp
            .into_body()
            .read_to_string()
            .map_err(|e| ApiError::Network(format!("read body: {e}")))?;
        if status >= 400 {
            return Err(ApiError::Http(status));
        }
        Ok(body)
    }

    fn raw_post(&mut self, path: &str, body: &str) -> Result<String, ApiError> {
        let token = if self.tokens.len() > 1 {
            Some(self.tokens.remove(0))
        } else {
            self.tokens.first().cloned()
        };
        let mut req = self
            .agent
            .post(&self.url(path))
            .header("User-Agent", UA)
            .header("Content-Type", "application/xml");
        if let Some(c) = self.cookie_header() {
            req = req.header("Cookie", &c);
        }
        if let Some(t) = token {
            req = req.header("__RequestVerificationToken", &t);
        }
        let resp = req.send(body).map_err(map_ureq)?;
        let status = resp.status().as_u16();
        self.absorb_headers(resp.headers());
        let text = resp
            .into_body()
            .read_to_string()
            .map_err(|e| ApiError::Network(format!("read body: {e}")))?;
        if status >= 400 {
            return Err(ApiError::Http(status));
        }
        Ok(text)
    }

    pub fn login(&mut self, user: &str, pass: &str) -> Result<(), ApiError> {
        // Fresh agent, fresh session: never reuse a poisoned cookie/token pair.
        self.agent = build_agent(self.timeout_ms);
        self.cookies.clear();
        self.tokens.clear();
        self.authed = false;

        if let Ok(html) = self.raw_get("/") {
            if let Some(t) = extract_csrf(&html) {
                self.tokens = vec![t];
            }
        }

        let mut ptype = 4u32;
        if let Ok(body) = self.raw_get("/api/user/state-login") {
            if let Ok(n) = xml::parse(&body) {
                if let Some(v) = n.value("password_type").and_then(|v| v.parse().ok()) {
                    ptype = v;
                }
            }
        }

        if self.tokens.is_empty() {
            // A transport failure here must surface as a network error, not as a
            // missing token: swallowing it made an unreachable router look like
            // an auth problem (and exit 3 instead of 2).
            match self.raw_get("/api/webserver/SesTokInfo") {
                Ok(body) => {
                    if let Ok(n) = xml::parse(&body) {
                        if let Some(t) = n.value("TokInfo") {
                            self.tokens = vec![t.to_string()];
                        }
                        if let Some(s) = n.value("SesInfo") {
                            if let Some((name, value)) = s.split_once('=') {
                                self.cookies
                                    .insert(name.trim().to_string(), value.trim().to_string());
                            }
                        }
                    }
                }
                Err(e) => {
                    return Err(ApiError::Network(format!(
                        "no session token from {} ({e}) - is this a Huawei web UI?",
                        self.base
                    )))
                }
            }
        }

        let token = self.tokens.first().cloned().ok_or_else(|| {
            ApiError::Network(format!(
                "{} answered without a CSRF token - not a Huawei web UI",
                self.base
            ))
        })?;

        let encoded = encode_password(user, pass, ptype, &token);
        let xml_body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><Username>{}</Username><Password>{}</Password><password_type>{}</password_type></request>",
            xml::esc(user),
            encoded,            ptype
        );

        let resp = self.raw_post("/api/user/login", &xml_body)?;
        let node = xml::parse(&resp)?;
        if node.name == "error" {
            let code = node
                .value("code")
                .and_then(|c| c.parse::<u32>().ok())
                .unwrap_or(0);
            let hint = login_hint(code);
            return Err(ApiError::Auth(format!("{hint} (code {code})")));
        }
        if node.text.trim() != "OK" {
            return Err(ApiError::Auth(format!(
                "router rejected login (password_type={ptype}): {}",
                truncate(node.text.trim(), 120)
            )));
        }
        self.authed = true;
        Ok(())
    }

    pub fn logout(&mut self) {
        if self.authed {
            let _ = self.raw_post(
                "/api/user/logout",
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><Logout>1</Logout></request>",
            );
        }
        self.authed = false;
    }

    /// Drop's logout: a fresh short-deadline agent, one POST, no retry, no output.
    ///
    /// `logout` keeps the configured budget because a caller that asked for it can
    /// report the failure. An exit has nobody to report to, and a `--json` run
    /// must not gain a stderr line on the way out.
    fn best_effort_logout(&mut self) {
        self.agent = build_agent(BEST_EFFORT_LOGOUT_MS);
        let _ = self.raw_post(
            "/api/user/logout",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><Logout>1</Logout></request>",
        );
        self.authed = false;
    }

    /// GET an `api/...` endpoint, mapping `<error><code>` to a typed error.
    pub fn get(&mut self, endpoint: &str) -> Result<Node, ApiError> {
        let raw = self.raw_get(&format!("/api/{endpoint}"))?;
        unwrap_document(&raw)
    }

    /// The DHCP document with its exact source text, for the whole-document write.
    pub fn get_dhcp(&mut self) -> Result<crate::model::DhcpSettings, ApiError> {
        let raw = self.raw("dhcp/settings")?;
        let node = unwrap_document(&raw)?;
        Ok(parse::parse_dhcp(&node).with_raw(&xml::inner_of_document(&raw)))
    }

    /// Whether this firmware exposes the LAN DHCP DNS setting at all.
    ///
    /// `dhcp/feature-switch` carries `dhcpsdns_display`, the exact flag the web UI's
    /// `isDnsDisplay()` checks before it draws the DNS checkbox and the two server
    /// fields. When it is not `1` the page is hidden and `POST dhcp/settings` applies
    /// the DHCP range and lease while silently dropping `DnsStatus`, `PrimaryDns` and
    /// `SecondaryDns`. `None` means the endpoint or the flag is missing, which is an
    /// older firmware: the write is attempted as before.
    pub fn dhcp_dns_editable(&mut self) -> Option<bool> {
        let node = self.get("dhcp/feature-switch").ok()?;
        node.value("dhcpsdns_display").map(|v| v.trim() == "1")
    }

    /// Write the band/network-mode configuration. The caller must verify the
    /// result by reading `net/net-mode` back (see `write::apply_bands`).
    pub fn post(&mut self, endpoint: &str, body: &str) -> Result<Node, ApiError> {
        let raw = self.raw_post(&format!("/api/{endpoint}"), body)?;
        unwrap_document(&raw)
    }

    /// Apply a band/network-mode payload.
    pub fn set_net_mode(&mut self, payload: &BandPayload) -> Result<Node, ApiError> {
        self.post("net/net-mode", &payload.xml())
    }

    /// SMS inbox. The firmware wants POST here; GET answers 125002.
    pub fn sms_list(&mut self, page: u32) -> Result<Node, ApiError> {
        self.sms_list_in(page, 1)
    }

    /// One page of one box. `box` is the firmware's `BoxType`: 1 inbox,
    /// 2 outbox, 3 draft.
    pub fn sms_list_in(&mut self, page: u32, box_type: u8) -> Result<Node, ApiError> {
        self.post("sms/sms-list", &sms_list_body(page, box_type))
    }

    /// Delete one message by index. Huawei takes a single index per request,
    /// which is why a bulk delete is a loop. Measured on the H122-373
    /// (2026-11-19): the index disappears from the next list.
    pub fn sms_delete(&mut self, index: &str) -> Result<Node, ApiError> {
        self.post(
            "sms/delete-sms",
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><Index>{index}</Index></request>"
            ),
        )
    }

    /// Mark one message read. Never called from the reader: opening a message
    /// must not write, so this waits for its own key and the write gate.
    pub fn sms_set_read(&mut self, index: &str) -> Result<Node, ApiError> {
        self.post(
            "sms/set-read",
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><Index>{index}</Index></request>"
            ),
        )
    }

    pub fn pin_status(&mut self) -> Result<Node, ApiError> {
        self.get("pin/status")
    }

    pub fn mobile_data_switch(&mut self) -> Result<Node, ApiError> {
        self.get("dialup/mobile-dataswitch")
    }

    /// The dial-up document as the firmware sends it. Field names measured on
    /// H122-373 (WEBUI 11.0.2.21, 2026-11-17): it carries `ConnectMode` and the
    /// dial policy, not the connect state. That state is `<ConnectionStatus>` in
    /// `monitoring/status`.
    pub fn dialup_connection(&mut self) -> Result<Node, ApiError> {
        self.get("dialup/connection")
    }

    /// The exact XML the data switch sends. The node name is what the firmware
    /// answers on H122-373 (WEBUI 11.0.2.21, 2026-11-17): GET
    /// `dialup/mobile-dataswitch` returns `<dataswitch>1</dataswitch>`, so the
    /// request carries the same name.
    pub fn dataswitch_payload(up: bool) -> &'static str {
        if up {
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><dataswitch>1</dataswitch></request>"
        } else {
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><dataswitch>0</dataswitch></request>"
        }
    }

    /// DESTRUCTIVE: restarts the router's radio/stack. The caller must have taken a
    /// typed confirmation; nothing else in the app calls this directly.
    pub fn restart(&mut self) -> Result<Node, ApiError> {
        self.post("device/control", Self::restart_payload())
    }

    /// The exact XML the restart sends - used by --dry-run and tests.
    pub fn restart_payload() -> &'static str {
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><Control>1</Control></request>"
    }

    /// Raw text, for fixtures and `snapshot --raw`.
    pub fn raw(&mut self, endpoint: &str) -> Result<String, ApiError> {
        self.raw_get(&Self::api_path(endpoint))
    }
}

impl Drop for HuaweiClient {
    /// Closes the firmware session on the exit paths no caller wrote by hand.
    ///
    /// The firmware keeps one session per login and refuses the next login with
    /// `108003` while it is open: on H122-373, WEBUI 11.0.2.21, measured
    /// 2026-11-22, the tenth consecutive `status --json` run was refused. Every
    /// `?` and every early return used to skip the handwritten `logout()`, so a
    /// one-shot command leaked a session per run. Drop cannot report, print or
    /// wait long, so it sends one request with a short deadline and ignores the
    /// answer; docs/limits.md states that a dead network makes it a silent no-op.
    fn drop(&mut self) {
        if self.authed {
            self.best_effort_logout();
        }
    }
}

fn unwrap_document(raw: &str) -> Result<Node, ApiError> {
    let node = xml::parse(raw)?;
    if node.name == "error" {
        let code = node
            .value("code")
            .and_then(|c| c.parse::<u32>().ok())
            .unwrap_or(0);
        return Err(classify(code));
    }
    Ok(node)
}

pub fn encode_password(user: &str, pass: &str, ptype: u32, token: &str) -> String {
    if ptype == 4 {
        // exactly what the firmware expects:
        // base64( sha256hex( user + base64(sha256hex(pass)) + token ) )
        let inner = hex_sha256(pass.as_bytes());
        let b64 = STANDARD.encode(inner.as_bytes());
        let outer = hex_sha256(format!("{user}{b64}{token}").as_bytes());
        STANDARD.encode(outer.as_bytes())
    } else {
        STANDARD.encode(pass.as_bytes())
    }
}

fn hex_sha256(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn extract_csrf(html: &str) -> Option<String> {
    let idx = html.find("csrf_token")?;
    let rest = &html[idx..];
    let start = rest.find("content=\"")? + "content=\"".len();
    let tail = &rest[start..];
    let end = tail.find('"')?;
    let value = &tail[..end];
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csrf_token_is_extracted_from_login_page() {
        let html = r#"<html><head><meta name="csrf_token" content="abc123=="></head></html>"#;
        assert_eq!(extract_csrf(html).as_deref(), Some("abc123=="));
        assert_eq!(extract_csrf("<html></html>"), None);
    }

    /// `raw("/")` must build `/api/`: a doubled slash is answered 404, and
    /// `doctor --live` would declare a reachable router unreachable before it
    /// probed a single capability.
    #[test]
    fn api_paths_never_double_the_slash() {
        assert_eq!(HuaweiClient::api_path(""), "/api/");
        assert_eq!(HuaweiClient::api_path("/"), "/api/");
        assert_eq!(
            HuaweiClient::api_path("device/signal"),
            "/api/device/signal"
        );
        assert_eq!(
            HuaweiClient::api_path("/device/signal/"),
            "/api/device/signal"
        );
        for endpoint in ["", "/", "monitoring/status", "/net/net-mode"] {
            assert!(
                !HuaweiClient::api_path(endpoint).contains("//"),
                "{endpoint:?} still doubles the slash"
            );
        }
    }

    /// Reachability asks the web UI root, not an `/api/` path: no firmware
    /// promises anything under `/api/` itself, every one serves a login page.
    #[test]
    fn reachability_asks_the_web_ui_root() {
        let client = HuaweiClient::new("10.0.0.1:8080", 1000);
        assert_eq!(client.url("/"), "http://10.0.0.1:8080/");
    }

    #[test]
    fn password_encoding_matches_the_reference_client() {
        // base64(sha256hex(user + base64(sha256hex(pass)) + token)) with type 4
        let out = encode_password("admin", "pw", 4, "TOK");
        let inner = hex_sha256(b"pw");
        let b64 = STANDARD.encode(inner.as_bytes());
        let outer = hex_sha256(format!("admin{b64}TOK").as_bytes());
        assert_eq!(out, STANDARD.encode(outer.as_bytes()));
        assert_eq!(
            encode_password("admin", "pw", 0, "TOK"),
            STANDARD.encode(b"pw")
        );
    }

    #[test]
    fn sms_list_body_carries_the_mandatory_ascending_field() {
        let body = sms_list_body(2, 1);
        assert!(body.contains("<PageIndex>2</PageIndex>"), "{body}");
        assert!(
            body.contains("<Ascending>0</Ascending>"),
            "the firmware answers 100005 without it: {body}"
        );
        assert!(body.contains("<BoxType>1</BoxType>"), "{body}");
    }

    #[test]
    fn the_page_request_and_the_range_label_use_one_page_size() {
        let body = sms_list_body(1, 1);
        assert!(
            body.contains(&format!("<ReadCount>{SMS_PAGE}</ReadCount>")),
            "{body}"
        );
        assert_eq!(
            crate::app::SMS_PAGE,
            SMS_PAGE,
            "the reader's range label must count the rows it asked for"
        );
    }

    #[test]
    fn login_codes_match_the_huawei_reference() {
        assert_eq!(login_hint(108001), "wrong username");
        assert_eq!(login_hint(108002), "wrong password");
        assert!(login_hint(108003).contains("already open"));
        assert_eq!(login_hint(108006), "wrong username or password");
        assert!(login_hint(108007).contains("too many"));
    }

    // -- session exit ------------------------------------------------------
    //
    // A leaked session costs the next command: the firmware refuses a second
    // login with 108003 while one is open (H122-373, WEBUI 11.0.2.21, measured
    // 2026-11-22). These tests hold every way a client can leave.

    use crate::mock_cpe::{MockCpe, Profile};

    fn authed(profile: Profile) -> (MockCpe, HuaweiClient) {
        let mock = MockCpe::start(profile);
        let mut c = HuaweiClient::new(mock.addr(), 3_000);
        c.login("admin", "demo").expect("mock login");
        (mock, c)
    }

    #[test]
    fn a_failed_read_still_logs_out() {
        let (mock, mut c) = authed(Profile::CodesRefused);
        assert!(
            c.get("monitoring/status").is_err(),
            "the profile refuses that read with 100003"
        );
        drop(c);
        assert_eq!(mock.logouts(), 1, "a refused read left the session open");
    }

    #[test]
    fn an_early_return_logs_out() {
        fn probe(c: &mut HuaweiClient) -> Result<(), ApiError> {
            c.get("monitoring/status")?;
            Ok(())
        }
        let (mock, mut c) = authed(Profile::CodesRefused);
        assert!(probe(&mut c).is_err(), "the `?` must leave early");
        drop(c);
        assert_eq!(mock.logouts(), 1, "the `?` skipped the logout");
    }

    #[test]
    fn a_manual_logout_is_not_doubled_by_drop() {
        let (mock, mut c) = authed(Profile::H122373);
        c.logout();
        drop(c);
        assert_eq!(mock.logouts(), 1, "Drop repeated a logout already done");
    }

    #[test]
    fn a_client_that_never_logged_in_never_logs_out() {
        let mock = MockCpe::start(Profile::H122373);
        let c = HuaweiClient::new(mock.addr(), 1_000);
        assert!(!c.is_authed());
        drop(c);
        assert_eq!(mock.logouts(), 0, "an anonymous client sent a logout");
    }

    #[test]
    fn a_dropped_client_logs_out_even_on_the_error_path() {
        let mock = MockCpe::start(Profile::H122373);
        let addr = mock.addr().to_string();
        let mut c = HuaweiClient::new(&addr, 1_000);
        c.login("admin", "demo").expect("mock login");
        drop(mock); // the router vanishes mid-life
        assert!(c.get("device/signal").is_err(), "a dead router must error");
        let started = std::time::Instant::now();
        drop(c);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "the exit waited {:?} for a router that is gone",
            started.elapsed()
        );
    }

    /// Structural: the guard only works while nothing leaves the process without
    /// unwinding. `process::exit` and `panic = "abort"` both skip Drop, so both
    /// are pinned here: the two exits in main run before a client exists or
    /// after run() already dropped it, and no profile aborts.
    #[test]
    fn no_process_exit_bypasses_the_guard() {
        fn body_range(src: &str, opener: &str) -> (usize, usize) {
            let lines: Vec<&str> = src.lines().collect();
            let start = lines
                .iter()
                .position(|l| l.starts_with(opener))
                .unwrap_or_else(|| panic!("no {opener}"));
            let mut depth = 0i32;
            let mut opened = false;
            for (i, line) in lines.iter().enumerate().skip(start) {
                for ch in line.chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            opened = true;
                        }
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                if opened && depth == 0 {
                    return (start + 1, i + 1);
                }
            }
            (start + 1, lines.len())
        }

        let main_rs = include_str!("../main.rs");
        let exits: Vec<usize> = main_rs
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains("std::process::exit("))
            .map(|(i, _)| i + 1)
            .collect();
        assert_eq!(
            exits.len(),
            2,
            "a new exit site has to be reasoned about: {exits:?}"
        );
        let (run_from, run_to) = body_range(main_rs, "fn run(");
        for line in exits {
            assert!(
                !(run_from <= line && line <= run_to),
                "the exit at main.rs:{line} sits inside run(), where a live client is dropped first"
            );
        }
        // Built at runtime so this test's own source cannot match the needle.
        let needle = concat!("std::process", "::exit(");
        let router_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/router");
        for entry in std::fs::read_dir(router_dir).expect("src/router") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|e| e == "rs") {
                let body = std::fs::read_to_string(&path).expect("read");
                // Test code may name the call it forbids; only shipped code counts.
                let shipped = body.split("#[cfg(test)]").next().unwrap_or("");
                assert!(
                    !shipped.contains(needle),
                    "{} leaves the process without unwinding, so Drop is skipped",
                    path.display()
                );
            }
        }
        let cargo = include_str!("../../Cargo.toml");
        assert!(
            !cargo.contains("panic = \"abort\""),
            "panic = abort would skip Drop and leak every session"
        );
    }
}
