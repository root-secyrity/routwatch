use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub router: Router,
    #[serde(default)]
    pub app: App,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Router {
    pub ip: String,
    pub username: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BandPreset {
    /// Derived from the bands themselves (`B1+B3+B20`), never typed by hand.
    pub name: String,
    /// Band numbers, ascending, de-duplicated.
    pub bands: Vec<u8>,
}

impl BandPreset {
    pub fn new(bands: &[u8]) -> BandPreset {
        let mut bands: Vec<u8> = bands.to_vec();
        bands.sort_unstable();
        bands.dedup();
        BandPreset {
            name: if bands.is_empty() {
                "—".into()
            } else {
                bands
                    .iter()
                    .map(|b| format!("B{b}"))
                    .collect::<Vec<_>>()
                    .join("+")
            },
            bands,
        }
    }
}

/// Ready-made merges, kept inside the band list this firmware actually lists.
/// They are the starting point, not a closed set: the user can save their own.
pub fn default_presets() -> Vec<BandPreset> {
    [
        vec![1, 3],
        vec![3, 20],
        vec![1, 3, 8],
        vec![1, 3, 20, 28],
        vec![1, 3, 5, 8, 20, 28],
    ]
    .iter()
    .map(|b| BandPreset::new(b))
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub refresh_ms: u64,
    pub timeout_ms: u64,
    pub theme: String,
    pub view: String,
    pub graph_style: String,
    pub budget_gb: u64,
    /// `true` includes serial/IMEI/WAN/client identities in `e` reports.
    #[serde(default)]
    pub report_identity: bool,
    #[serde(default)]
    pub secure_trust: bool,
    /// `None` = the key was never written, so the built-in merges are used.
    /// `Some([])` = the user deleted every preset and means it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presets: Option<Vec<BandPreset>>,
}

impl Default for App {
    fn default() -> Self {
        App {
            refresh_ms: 2000,
            timeout_ms: 3000,
            theme: "terminal".into(),
            view: "full".into(),
            graph_style: "dots".into(),
            budget_gb: 100,
            report_identity: false,
            secure_trust: false,
            presets: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            router: Router {
                ip: "192.168.8.1".into(),
                username: "admin".into(),
                password: String::new(),
            },
            app: App::default(),
        }
    }
}

pub fn config_path(explicit: Option<PathBuf>) -> Result<PathBuf, ApiError> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    let base =
        dirs::config_dir().ok_or_else(|| ApiError::Config("no XDG_CONFIG_HOME or HOME".into()))?;
    Ok(base.join("routwatch").join("config.toml"))
}

pub fn generate(explicit: Option<PathBuf>) -> Result<PathBuf, ApiError> {
    let path = config_path(explicit)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| ApiError::Config(format!("{dir:?}: {e}")))?;
    }
    if path.exists() {
        return Err(ApiError::Config(format!(
            "{} already exists; refusing to overwrite",
            path.display()
        )));
    }
    let body = toml::to_string_pretty(&Config::default())
        .map_err(|e| ApiError::Config(format!("serialize: {e}")))?;
    let header = "# routwatch config — see `routwatch --help`\n\
                  # Password is optional here: prefer ROUTWATCH_PASSWORD or secret-tool.\n";
    fs::write(&path, format!("{header}{body}"))
        .map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|e| ApiError::Config(format!("chmod 600 {}: {e}", path.display())))?;
    Ok(path)
}

/// Persist settings changed in the UI. Always leaves the file at 0600.
pub fn save(explicit: Option<PathBuf>, cfg: &Config) -> Result<PathBuf, ApiError> {
    let path = config_path(explicit)?;
    let body =
        toml::to_string_pretty(cfg).map_err(|e| ApiError::Config(format!("serialize: {e}")))?;
    let header = "# routwatch config — see `routwatch --help`\n\
                  # Password is optional here: prefer ROUTWATCH_PASSWORD or secret-tool.\n";
    fs::write(&path, format!("{header}{body}"))
        .map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|e| ApiError::Config(format!("chmod 600 {}: {e}", path.display())))?;
    Ok(path)
}

pub fn load(explicit: Option<PathBuf>) -> Result<Config, ApiError> {
    let path = config_path(explicit)?;
    let meta =
        fs::metadata(&path).map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(ApiError::Config(format!(
            "{} is mode {:o}; run `chmod 600 {}` (it may hold a password)",
            path.display(),
            mode,
            path.display()
        )));
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
    let mut cfg: Config =
        toml::from_str(&raw).map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
    // ROUTWATCH_IP overrides the router address: lets a script or a broken-link
    // test point somewhere else without editing the config file.
    if let Ok(ip) = std::env::var("ROUTWATCH_IP") {
        if !ip.trim().is_empty() {
            cfg.router.ip = ip.trim().to_string();
        }
    }
    Ok(cfg)
}

/// Load the config, or the built-in defaults when the file simply does not
/// exist.
///
/// Only the commands that never touch the router use this (`preview`,
/// `replay`): a layout review on a fresh machine — or on a CI runner — must not
/// require `--generate-config` first. A file that exists but is unreadable,
/// world-readable or malformed is still an error, so the 0600 guarantee is not
/// weakened.
pub fn load_or_default(explicit: Option<PathBuf>) -> Result<Config, ApiError> {
    let path = config_path(explicit)?;
    match fs::metadata(&path) {
        Ok(_) => load(Some(path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(ApiError::Config(format!("{}: {e}", path.display()))),
    }
}

/// Password ladder, in the documented order. Empty means "not provided here".
pub fn resolve_password(cfg_password: &str) -> Result<String, ApiError> {
    if let Ok(v) = std::env::var("ROUTWATCH_PASSWORD") {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if let Some(v) = secret_lookup() {
        return Ok(v);
    }
    if !cfg_password.is_empty() {
        return Ok(cfg_password.to_string());
    }
    Err(ApiError::Config(
        "no credentials (checked --password-stdin, ROUTWATCH_PASSWORD, secret-tool, config.toml)"
            .into(),
    ))
}

/// libsecret lookup; a missing binary is not an error, just a skipped rung.
pub fn secret_lookup() -> Option<String> {
    let out = std::process::Command::new("secret-tool")
        .args(["lookup", "service", "routwatch"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const MINIMAL: &str = r#"
[router]
ip = "192.168.8.1"
username = "admin"

[app]
refresh_ms = 2000
timeout_ms = 3000
theme = "terminal"
view = "full"
graph_style = "dots"
budget_gb = 100
"#;

    #[test]
    fn presets_default_when_the_key_is_absent_but_empty_is_respected() {
        let cfg: Config = toml::from_str(MINIMAL).unwrap();
        assert!(
            cfg.app.presets.is_none(),
            "an old config must fall back to the built-in merges"
        );

        // An explicitly empty list means the user deleted them all; it must not
        // silently grow the defaults back on load.
        let with_empty = format!("{MINIMAL}\npresets = []\n");
        let cfg: Config = toml::from_str(&with_empty).unwrap();
        assert_eq!(cfg.app.presets, Some(Vec::new()));
    }

    #[test]
    fn presets_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("routwatch-presets-{}", std::process::id()));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();

        let mut cfg = Config::default();
        cfg.app.presets = Some(vec![BandPreset::new(&[20, 3, 1])]);
        save(Some(path.clone()), &cfg).unwrap();

        let loaded = load(Some(path.clone())).unwrap();
        let presets = loaded.app.presets.expect("presets survive a save/load");
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "B1+B3+B20", "name is derived and sorted");
        assert_eq!(presets[0].bands, vec![1, 3, 20]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_falls_back_to_defaults_but_a_bad_one_does_not() {
        let dir = std::env::temp_dir().join(format!("routwatch-default-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let absent = dir.join("config.toml");

        // No file at all: defaults, so preview/replay work on a fresh machine.
        let cfg = load_or_default(Some(absent.clone())).expect("defaults without a file");
        assert_eq!(cfg.router.ip, "192.168.8.1");
        assert_eq!(cfg.app.theme, "terminal");
        assert!(cfg.app.presets.is_none());

        // A file that exists but is world-readable is still refused.
        std::fs::write(&absent, "[router]\nip = \"10.0.0.1\"\nusername = \"u\"\n").unwrap();
        std::fs::set_permissions(&absent, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            load_or_default(Some(absent.clone())).is_err(),
            "0600 must still be enforced when the file exists"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_then_load_round_trips_and_stays_0600() {
        let dir = std::env::temp_dir().join(format!("routwatch-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();

        let mut cfg = Config::default();
        cfg.app.theme = "dracula".into();
        cfg.app.view = "dense".into();
        cfg.app.graph_style = "braille".into();
        cfg.app.refresh_ms = 1500;
        cfg.app.budget_gb = 250;

        let written = save(Some(path.clone()), &cfg).unwrap();
        assert_eq!(written, path);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "settings file must stay private");

        let loaded = load(Some(path.clone())).unwrap();
        assert_eq!(loaded.app.theme, "dracula");
        assert_eq!(loaded.app.view, "dense");
        assert_eq!(loaded.app.graph_style, "braille");
        assert_eq!(loaded.app.refresh_ms, 1500);
        assert_eq!(loaded.app.budget_gb, 250);

        // a world-readable file is refused, not silently accepted
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(Some(path.clone())).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
