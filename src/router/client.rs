//! Huawei web-API client.
//!
//! Blocking by design: the router serialises requests anyway (~85 ms each, a
//! 5-endpoint poll measured 432 ms), so one serialized session costs nothing and
//! removes token-rotation races. Every timeout is set explicitly - ureq defaults
//! all of them to `None` (no limit at all).

use crate::error::{classify, ApiError};
use crate::router::bands::BandPayload;
use crate::xml::{self, Node};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;
use ureq::{Agent, Error as UreqError};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

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
        .build();
    config.into()
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
    /// place. `raw("/")` used to build `/api//`, which a strict server answers
    /// with 404 - and the callers that pass "" or "/" only ever meant `/api/`.
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
            esc(user),
            encoded,
            ptype
        );

        let resp = self.raw_post("/api/user/login", &xml_body)?;
        let node = xml::parse(&resp)?;
        if node.name == "error" {
            let code = node
                .value("code")
                .and_then(|c| c.parse::<u32>().ok())
                .unwrap_or(0);
            let hint = match code {
                108001 | 108003 | 108006 => "wrong username or password",
                108002 => "too many login attempts; wait before retrying",
                _ => "router refused the login",
            };
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

    /// GET an `api/...` endpoint, mapping `<error><code>` to a typed error.
    pub fn get(&mut self, endpoint: &str) -> Result<Node, ApiError> {
        let raw = self.raw_get(&format!("/api/{endpoint}"))?;
        unwrap_document(&raw)
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

    /// Write DNS settings. `status` is "0" (manual servers) or "1" (automatic).
    pub fn set_dns(
        &mut self,
        status: &str,
        primary: &str,
        secondary: &str,
    ) -> Result<Node, ApiError> {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><DnsStatus>{status}</DnsStatus><PrimaryDns>{}</PrimaryDns><SecondaryDns>{}</SecondaryDns></request>",
            esc(primary),
            esc(secondary)
        );
        self.post("dhcp/settings", &body)
    }

    /// SMS inbox. The firmware wants POST here; GET answers 125002.
    pub fn sms_list(&mut self, page: u32) -> Result<Node, ApiError> {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><PageIndex>{page}</PageIndex><ReadCount>20</ReadCount><BoxType>1</BoxType><SortType>0</SortType><UnreadPreferred>0</UnreadPreferred></request>"
        );
        self.post("sms/sms-list", &body)
    }

    pub fn pin_status(&mut self) -> Result<Node, ApiError> {
        self.get("pin/status")
    }

    pub fn mobile_data_switch(&mut self) -> Result<Node, ApiError> {
        self.get("dialup/mobile-dataswitch")
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

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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

    /// `raw("/")` used to build `GET /api//`, which a strict server answers 404 -
    /// `doctor --live` then declared a reachable router unreachable and stopped
    /// before it probed a single capability.
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
}
