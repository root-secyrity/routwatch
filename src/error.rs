use std::fmt;

/// Every failure carries its class, so the UI can say what happened instead of
/// showing an empty value. Derived from the router's own `<error><code>` bodies.
///
/// 100003 means ERROR_NO_RIGHT, and this firmware also answers it for paths it
/// does not serve (measured on WEBUI 11.0.2.1, 2026-11-10), so `classify` keeps
/// it Unsupported and `Display` names the code without claiming a cause. That
/// leaves `Denied` unreachable from `classify`; it stays for callers that know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    Unsupported(u32),
    Denied(u32),
    Invalid(u32),
    Busy(u32),
    Router(u32),
    /// A write the router did not accept, with the real reason (exit 1).
    WriteFailed(String),
    Http(u16),
    Network(String),
    Parse(String),
    Auth(String),
    Config(String),
    /// Bad arguments, an unsupported value, or a local file the command cannot
    /// read as asked: a recording, a snapshot (exit 3).
    Usage(String),
    /// The command already printed its own verdict; only the exit code matters.
    Quiet(i32),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unsupported(code) => match code_name(*code) {
                Some("ERROR_NOT_SUPPORT") => write!(
                    f,
                    "the firmware does not serve this request ({} ERROR_NOT_SUPPORT)",
                    code
                ),
                Some("ERROR_NO_RIGHT") => write!(
                    f,
                    "the firmware refused this request ({} ERROR_NO_RIGHT)",
                    code
                ),
                _ => write!(f, "not supported by this firmware ({})", code),
            },
            ApiError::Denied(code) => write!(f, "permission denied ({})", code),
            ApiError::Invalid(code) => write!(f, "invalid request or session ({})", code),
            ApiError::Busy(code) => write!(f, "router busy ({})", code_with_name(*code)),
            ApiError::Router(code) => write!(f, "unknown router error ({})", code),
            ApiError::WriteFailed(reason) => write!(f, "write failed: {}", reason),
            ApiError::Http(status) => write!(f, "http status {}", status),
            ApiError::Network(reason) => write!(f, "network: {}", reason),
            ApiError::Parse(reason) => write!(f, "parse: {}", reason),
            ApiError::Auth(reason) => write!(f, "authentication failed: {}", reason),
            ApiError::Config(reason) => write!(f, "config: {}", reason),
            ApiError::Usage(text) => f.write_str(text),
            ApiError::Quiet(_) => Ok(()),
        }
    }
}

impl std::error::Error for ApiError {}

/// Map a Huawei `<code>` to a typed error.
pub fn classify(code: u32) -> ApiError {
    match code {
        100003 => ApiError::Unsupported(code),
        100005 => ApiError::Invalid(code), // ERROR_FORMAT_ERROR, not a denial
        125002 | 125003 => ApiError::Invalid(code),
        100002 => ApiError::Unsupported(code), // ERROR_NOT_SUPPORT
        100004 => ApiError::Busy(code),        // ERROR_BUSY
        _ => ApiError::Router(code),
    }
}

/// The firmware codes this client acts on, with the constant Huawei's tables give
/// each one. Only codes with a published name appear here: 125002, 125003 and
/// 100010 are classified but have no name in either table, so none is invented.
pub const CODE_NAMES: &[(u32, &str)] = &[
    // Huawei error table; measured on a B310-22s running the same WebAPI
    // (2022-02-16, huawei-lte-api discussion #190).
    (100002, "ERROR_NOT_SUPPORT"),
    // Huawei error table, and measured on WEBUI 11.0.2.1 (2026-11-10).
    (100003, "ERROR_NO_RIGHT"),
    // Huawei error table only, unverified on this firmware.
    (100004, "ERROR_BUSY"),
    // Huawei error table, and measured on WEBUI 11.0.2.1 sms/sms-list (2026-11-10).
    (100005, "ERROR_FORMAT_ERROR"),
    // Huawei error table, and measured on WEBUI 11.0.2.1 net/net-mode (2026-11-09).
    (100006, "ERROR_PARAMETER_ERROR"),
];

/// The Huawei constant for a firmware code, when one is published.
pub fn code_name(code: u32) -> Option<&'static str> {
    CODE_NAMES
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, name)| *name)
}

/// The code as the firmware numbers it, with its constant when one is published.
fn code_with_name(code: u32) -> String {
    match code_name(code) {
        Some(name) => format!("{code} {name}"),
        None => code.to_string(),
    }
}

impl ApiError {
    /// Stable machine-readable class for --json output.
    pub fn class(&self) -> &'static str {
        match self {
            ApiError::Unsupported(_) => "unsupported",
            ApiError::Denied(_) => "denied",
            ApiError::Invalid(_) => "invalid",
            ApiError::Busy(_) => "busy",
            ApiError::Router(_) => "router_error",
            ApiError::WriteFailed(_) => "write",
            ApiError::Http(_) => "http",
            ApiError::Network(_) => "network",
            ApiError::Parse(_) => "parse",
            ApiError::Auth(_) => "auth",
            ApiError::Config(_) => "config",
            ApiError::Usage(_) => "usage",
            ApiError::Quiet(_) => "quiet",
        }
    }

    /// Documented exit codes: 0 ok, 1 router, 2 network, 3 usage/config.
    pub fn exit_code(&self) -> i32 {
        match self {
            ApiError::Network(_) => 2,
            ApiError::Config(_) => 3,
            ApiError::Auth(_) => 3,
            ApiError::Usage(_) => 3,
            ApiError::Quiet(code) => *code,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_codes_keep_their_meaning() {
        assert_eq!(classify(100003), ApiError::Unsupported(100003));
        assert_eq!(
            classify(100005),
            ApiError::Invalid(100005),
            "100005 is ERROR_FORMAT_ERROR, not a denial"
        );
        assert_eq!(
            classify(100002),
            ApiError::Unsupported(100002),
            "100002 is ERROR_NOT_SUPPORT, not a busy router"
        );
        assert_eq!(
            classify(100004),
            ApiError::Busy(100004),
            "100004 is ERROR_BUSY"
        );
        // 100003 means NO_RIGHT, but this firmware also answers it for paths it
        // does not serve (measured 2026-11-10), so no single class is honest and
        // it stays Unsupported until a measurement separates the two.
        assert_eq!(classify(100003), ApiError::Unsupported(100003));
    }

    #[test]
    fn display_names_the_code_beside_its_meaning() {
        assert_eq!(
            ApiError::Unsupported(100002).to_string(),
            "the firmware does not serve this request (100002 ERROR_NOT_SUPPORT)"
        );
        assert_eq!(
            ApiError::Unsupported(100003).to_string(),
            "the firmware refused this request (100003 ERROR_NO_RIGHT)"
        );
        assert_eq!(
            ApiError::Busy(100004).to_string(),
            "router busy (100004 ERROR_BUSY)"
        );
    }

    #[test]
    fn display_stays_numeric_for_codes_without_a_name() {
        assert_eq!(
            ApiError::Router(999999).to_string(),
            "unknown router error (999999)"
        );
        assert_eq!(
            ApiError::Unsupported(404).to_string(),
            "not supported by this firmware (404)"
        );
        assert_eq!(
            ApiError::Invalid(125002).to_string(),
            "invalid request or session (125002)"
        );
    }

    #[test]
    fn display_keeps_the_text_of_every_other_variant() {
        assert_eq!(
            ApiError::Denied(100003).to_string(),
            "permission denied (100003)"
        );
        assert_eq!(
            ApiError::WriteFailed("no read-back".into()).to_string(),
            "write failed: no read-back"
        );
        assert_eq!(ApiError::Http(500).to_string(), "http status 500");
        assert_eq!(
            ApiError::Network("timeout".into()).to_string(),
            "network: timeout"
        );
        assert_eq!(
            ApiError::Parse("bad xml".into()).to_string(),
            "parse: bad xml"
        );
        assert_eq!(
            ApiError::Auth("wrong password".into()).to_string(),
            "authentication failed: wrong password"
        );
        assert_eq!(
            ApiError::Config("mode 0644".into()).to_string(),
            "config: mode 0644"
        );
        assert_eq!(
            ApiError::Usage("no such flag".into()).to_string(),
            "no such flag"
        );
        assert_eq!(ApiError::Quiet(0).to_string(), "");
    }

    #[test]
    fn named_codes_carry_their_huawei_constant() {
        assert_eq!(code_name(100002), Some("ERROR_NOT_SUPPORT"));
        assert_eq!(code_name(100003), Some("ERROR_NO_RIGHT"));
        assert_eq!(code_name(100004), Some("ERROR_BUSY"));
        assert_eq!(code_name(100005), Some("ERROR_FORMAT_ERROR"));
        assert_eq!(code_name(100006), Some("ERROR_PARAMETER_ERROR"));
    }

    #[test]
    fn unknown_and_unnamed_codes_have_no_name() {
        assert_eq!(code_name(999999), None);
        // No name for these in either published table, so none is invented.
        assert_eq!(code_name(125002), None);
        assert_eq!(code_name(125003), None);
        assert_eq!(code_name(100010), None);
    }

    #[test]
    fn the_name_table_has_no_duplicate_keys() {
        let mut codes: Vec<u32> = CODE_NAMES.iter().map(|(code, _)| *code).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total);

        let mut names: Vec<&str> = CODE_NAMES.iter().map(|(_, name)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
    }
}
