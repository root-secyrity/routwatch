//! `routwatch antenna worth` end to end: the real binary, a saved signal log and
//! a saved speed ledger, and no router. Every row here is synthetic.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_routwatch"))
}

/// A scratch data home that removes itself, plus a private config beside it.
struct Scratch {
    dir: PathBuf,
    config: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "rw-aw-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("routwatch")).expect("data dir");
        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            "[router]\nip = \"192.168.8.1\"\nusername = \"admin\"\n\n[app]\nrefresh_ms = 2000\ntimeout_ms = 250\ntheme = \"terminal\"\nview = \"full\"\ngraph_style = \"dots\"\nbudget_gb = 100\n",
        )
        .expect("config");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        Scratch { dir, config }
    }

    fn log(&self) -> PathBuf {
        self.dir.join("routwatch").join("signal-history.jsonl")
    }

    fn speed(&self) -> PathBuf {
        self.dir.join("routwatch").join("speed-history.jsonl")
    }

    /// Rows spread over the last `minutes`, one a minute, ending an hour ago.
    fn write_rows(&self, minutes: usize, rsrp: f32, rsrq: f32, sinr: f32) {
        let now = now();
        let mut text = String::new();
        for i in 0..minutes {
            let at = now - 3600 - 60 * (minutes as u64 - i as u64);
            text.push_str(&format!(
                "{{\"v\":2,\"at_unix\":{at},\"net_type\":\"LTE\",\"band\":\"B1\",\"nr_band\":null,\"rsrp_dbm\":{rsrp},\"rsrq_db\":{rsrq},\"sinr_db\":{sinr},\"pci\":481,\"endc\":true,\"earfcn_dl\":1650,\"earfcn_ul\":18100,\"tm\":\"TM[4]\"}}\n"
            ));
        }
        std::fs::write(self.log(), text).expect("signal log");
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = bin();
        cmd.args(args);
        cmd.env("XDG_DATA_HOME", &self.dir);
        cmd.env("XDG_CONFIG_HOME", &self.dir);
        cmd.env("ROUTWATCH_PASSWORD", "test-placeholder");
        cmd.env("TZ", "UTC");
        cmd.output().expect("spawn routwatch")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cfg_arg(s: &Scratch) -> String {
    s.config.to_str().unwrap().to_string()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn a_poor_sinr_over_a_strong_rsrp_is_read_as_interference_limited() {
    let s = Scratch::new("interf");
    s.write_rows(90, -92.0, -15.0, 3.0);
    let cfg = cfg_arg(&s);
    let out = s.run(&["--config", &cfg, "antenna", "worth", "--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(v["cells"][0]["class"], "interference-limited", "{v}");
    assert_eq!(v["cells"][0]["rows"], 90);
    assert_eq!(v["cells"][0]["earfcn_dl"], 1650);
    assert_eq!(v["speed_rows"], 0);
}

#[test]
fn a_weak_rsrp_is_signal_limited_even_with_a_poor_sinr() {
    let s = Scratch::new("weak");
    s.write_rows(90, -108.0, -18.0, 2.0);
    let cfg = cfg_arg(&s);
    let out = s.run(&["--config", &cfg, "antenna", "worth", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(v["cells"][0]["class"], "signal-limited", "{v}");
}

#[test]
fn a_good_link_reads_as_not_signal_limited_and_names_no_lever_to_buy() {
    let s = Scratch::new("good");
    s.write_rows(90, -78.0, -9.0, 22.0);
    let cfg = cfg_arg(&s);
    let out = s.run(&["--config", &cfg, "antenna", "worth"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("not-signal-limited"), "{text}");
    assert!(text.contains("lever: nothing on the radio side"), "{text}");
    for word in ["buy", "purchase", "order"] {
        assert!(!text.to_lowercase().contains(word), "{word} in {text}");
    }
}

#[test]
fn an_empty_log_is_said_so_and_exits_zero() {
    let s = Scratch::new("empty");
    let cfg = cfg_arg(&s);
    let out = s.run(&["--config", &cfg, "antenna", "worth", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(v["cells"].as_array().unwrap().len(), 0);
    assert!(
        v["missing"][0].as_str().unwrap().contains("only v2 rows"),
        "{v}"
    );
}

#[test]
fn a_speed_row_in_the_window_is_reported_beside_the_class_not_into_it() {
    let s = Scratch::new("speed");
    s.write_rows(90, -92.0, -15.0, 3.0);
    let now = now();
    let mut e = routwatch::history::Entry::blank();
    e.started_unix = now - 1200;
    e.ended_unix = now - 1180;
    e.elapsed_ms = 20_000;
    e.download_bps = 42_000_000.0;
    e.upload_bps = 8_000_000.0;
    e.ttfb_ms = 40.0;
    e.down_secs = 10.0;
    e.up_secs = 4.0;
    e.bytes_down = 52_000_000;
    e.bytes_up = 4_000_000;
    e.band = Some("B1".into());
    e.band_at_start = Some("B1".into());
    let line = serde_json::to_string(&e).expect("row");
    std::fs::write(s.speed(), format!("{line}\n")).expect("speed ledger");
    let cfg = cfg_arg(&s);
    let out = s.run(&["--config", &cfg, "antenna", "worth", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(v["speed_rows"], 1, "{v}");
    assert_eq!(v["cells"][0]["class"], "interference-limited", "{v}");
    assert!(v["download_median_mbps"].as_f64().unwrap() > 41.0, "{v}");
}

#[test]
fn the_command_is_in_help_and_in_the_completion_list() {
    let s = Scratch::new("help");
    let out = s.run(&["--help"]);
    assert!(stdout(&out).contains("antenna"), "{}", stdout(&out));
    let out = s.run(&["--completion", "fish"]);
    assert!(stdout(&out).contains("antenna"), "{}", stdout(&out));
    let out = s.run(&["antenna", "--help"]);
    assert!(stdout(&out).contains("worth"), "{}", stdout(&out));
}
