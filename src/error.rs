use thiserror::Error;

/// Every failure carries its class, so the UI can say what happened instead of
/// showing an empty value. Derived from the router's own `<error><code>` bodies.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ApiError {
    #[error("not supported by this firmware ({0})")]
    Unsupported(u32),
    #[error("permission denied ({0})")]
    Denied(u32),
    #[error("invalid request or session ({0})")]
    Invalid(u32),
    #[error("router busy ({0})")]
    Busy(u32),
    #[error("unknown router error ({0})")]
    Router(u32),
    /// A write the router did not accept, with the real reason (exit 1).
    #[error("write failed: {0}")]
    WriteFailed(String),
    #[error("http status {0}")]
    Http(u16),
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("config: {0}")]
    Config(String),
    /// Bad arguments or an unsupported value on the command line (exit 3).
    #[error("{0}")]
    Usage(String),
    /// The command already printed its own verdict; only the exit code matters.
    #[error("")]
    Quiet(i32),
}

/// Map a Huawei `<code>` to a typed error.
pub fn classify(code: u32) -> ApiError {
    match code {
        100003 => ApiError::Unsupported(code),
        100005 => ApiError::Invalid(code), // ERROR_FORMAT_ERROR, not a denial
        125002 | 125003 => ApiError::Invalid(code),
        100002 => ApiError::Busy(code),
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
