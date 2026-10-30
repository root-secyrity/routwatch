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
        100005 => ApiError::Denied(code),
        125002 | 125003 => ApiError::Invalid(code),
        100002 => ApiError::Busy(code),
        _ => ApiError::Router(code),
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
