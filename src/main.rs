use clap::{Parser, Subcommand};
use routwatch::app::{App, InputKind, PresetSaved, Tab, View};
use routwatch::config;
use routwatch::error::ApiError;
use routwatch::headless;
use routwatch::model::Snapshot;
use routwatch::router::{bands, parse, HuaweiClient};
use routwatch::theme::{GraphStyle, Theme};
use routwatch::ui;
use routwatch::worker::{self, Cmd as WorkerCmd, Ev};
use routwatch::write::{self, WriteIntent, WriteState};
use routwatch::{completion, record};
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

#[derive(Parser)]
#[command(
    name = "routwatch",
    version,
    about = "Huawei LTE/5G CPE manager in the terminal",
    long_about = "A terminal manager for Huawei CPEs: live signal, traffic, clients, bands and DNS. \
                  Writes (bands, DNS) are confirmed and verified by reading the router back; \
                  reads are headless too (`snapshot`, `doctor`, `ping`)."
)]
struct Cli {
    /// Path to config.toml (default: $XDG_CONFIG_HOME/routwatch/config.toml)
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Write a default config file (mode 0600) and exit
    #[arg(long)]
    generate_config: bool,

    /// Read the password from stdin instead of the credential ladder
    #[arg(long, global = true)]
    password_stdin: bool,

    /// Colour theme: terminal (default), dark, ocean, solarized, dracula,
    /// nord, sky, paper, or auto (follow the running desktop's theme).
    /// Aliases: system/ansi = terminal, light/minimal = paper
    #[arg(long, global = true)]
    theme: Option<String>,

    /// View: full, lite, dense
    #[arg(long, global = true)]
    view: Option<String>,

    /// Graph style: dots, bars, braille
    #[arg(long, global = true)]
    graph: Option<String>,

    /// Include serial, IMEI, WAN IP and client identities in `e` reports
    #[arg(long, global = true)]
    report_identity: bool,

    /// Start on a tab: dashboard, bands, traffic, network, clients, sms, device
    #[arg(long, global = true)]
    tab: Option<String>,

    /// Print a shell completion script and exit (bash|zsh|fish)
    #[arg(long, global = true, value_name = "SHELL")]
    completion: Option<String>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print one live sample and exit
    Snapshot {
        /// Machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Dump raw XML responses instead of parsing
        #[arg(long)]
        raw: bool,
    },
    /// Health check in netwatch's doctor --json shape
    Doctor {
        #[arg(long)]
        json: bool,
        /// Contact the router (reachability, auth, endpoint capabilities)
        #[arg(long)]
        live: bool,
    },
    /// Bands: read them, or apply a selection
    Bands {
        #[arg(long)]
        list: bool,
        #[arg(long)]
        json: bool,
        /// Comma-separated LTE bands to apply (e.g. 1,3,5)
        #[arg(long, value_name = "LIST")]
        set: Option<String>,
        /// Network mode for --set: auto, 4g, 5g
        #[arg(long, default_value = "auto")]
        mode: String,
        /// NR option when --mode 5g: nsa, sa, both
        #[arg(long)]
        option: Option<String>,
        /// Print the XML that would be sent, change nothing
        #[arg(long)]
        dry_run: bool,
        /// Permit the write from a non-interactive command
        #[arg(long)]
        allow_write: bool,
        /// Send even when the router already reports this exact mask
        /// (used once per firmware to prove the write path; resets the radio)
        #[arg(long)]
        force: bool,
    },
    /// DNS servers: read, set or reset (verified write path)
    Dns {
        /// PRIMARY,SECONDARY (IPv4)
        #[arg(long, value_name = "PAIR")]
        set: Option<String>,
        /// Hand DNS back to the DHCP server
        #[arg(long)]
        reset: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        allow_write: bool,
        /// Send even when the device already holds these values
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// SMS counters plus one inbox attempt (reports its blocker honestly)
    Sms {
        #[arg(long)]
        inbox: bool,
        #[arg(long)]
        json: bool,
    },
    /// Device details including SIM state and the mobile-data switch
    Device {
        #[arg(long)]
        json: bool,
    },
    /// Restart the router — destructive, needs --allow-write; --dry-run prints only
    Restart {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        allow_write: bool,
    },
    /// Ping a host from this machine (local ping, not the router's)
    Ping {
        host: String,
        #[arg(long, default_value_t = 3)]
        count: u32,
        #[arg(long)]
        json: bool,
    },
    /// Dump every probed endpoint (raw XML by default, --json for a full document)
    Dump {
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// One JSON document: parsed snapshot + every raw response
        #[arg(long)]
        json: bool,
    },
    /// One sample for scripts (--json) or a status bar (--waybar)
    Status {
        /// Flat one-line JSON for scripts
        #[arg(long)]
        json: bool,
        /// Waybar custom-module payload: text, tooltip, class, percentage
        #[arg(long)]
        waybar: bool,
    },
    /// Sample the router into a recording file (.rwr, one frame per line)
    Record {
        /// Destination file (default: $XDG_DATA_HOME/routwatch/recordings/<date>.rwr)
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Drop frames older than this on every write (30d, 12h, 90m, 45s)
        #[arg(long, default_value = "30d")]
        keep: String,
        /// Seconds between samples
        #[arg(long, default_value_t = 2)]
        interval: u64,
        /// Stop after N samples (default: run until Ctrl-C)
        #[arg(long)]
        count: Option<u64>,
    },
    /// Replay a recording: an interactive TUI, or --json for the frames
    Replay {
        file: PathBuf,
        #[arg(long)]
        json: bool,
        /// Playback multiplier (4 = four samples per second of wall time)
        #[arg(long, default_value_t = 4)]
        speed: u64,
    },
    /// Render one frame to stdout (no TTY needed) — review the layout anywhere
    Preview {
        /// WIDTHxHEIGHT, e.g. 100x30
        #[arg(long, default_value = "100x30")]
        size: String,
        /// Render from a saved snapshot JSON instead of contacting the router
        #[arg(long, value_name = "FILE")]
        fixture: Option<PathBuf>,
        /// Layout review: open the apply-confirm modal (sends nothing)
        #[arg(long, hide = true)]
        demo_confirm: bool,
        /// Layout review: open the settings overlay
        #[arg(long, hide = true)]
        demo_settings: bool,
        /// Emit real ANSI colours instead of plain symbols — for README
        /// screenshots and for checking a theme without a terminal
        #[arg(long)]
        ansi: bool,
    },
}

fn main() {
    // Die quietly on a closed pipe (`routwatch snapshot | head`) instead of
    // panicking with "failed printing to stdout".
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(()) => 0,
        Err(e) => {
            // Quiet errors already told the user what happened.
            if !e.to_string().is_empty() {
                eprintln!("routwatch: {e}");
            }
            e.exit_code()
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli) -> Result<(), ApiError> {
    if let Some(shell) = cli.completion.clone() {
        return completion::print(&shell);
    }

    if cli.generate_config {
        let path = config::generate(cli.config.clone())?;
        println!("config written: {}", path.display());
        return Ok(());
    }

    let cfg_path = config::config_path(cli.config.clone())?;
    // The family validates this flag (`Invalid theme. Use: …`) rather than
    // silently dropping to `dark`, so a typo is visible instead of mysterious.
    if let Some(name) = &cli.theme {
        if let Err(e) = routwatch::theme::validate(name) {
            return Err(ApiError::Config(e));
        }
    }
    // Offline commands must work without a config file; everything that talks
    // to the router keeps the strict loader (and its 0600 check).
    let offline = matches!(
        cli.cmd,
        Some(Cmd::Preview { .. }) | Some(Cmd::Replay { .. })
    );
    let cfg = if offline {
        config::load_or_default(cli.config.clone())?
    } else {
        config::load(cli.config.clone())?
    };

    // Captured before the match moves `cli.cmd`: replay needs to build an App.
    let ui_opts = UiOpts {
        theme: cli.theme.clone(),
        view: cli.view.clone(),
        graph: cli.graph.clone(),
        tab: cli.tab.clone(),
        config_path: cli.config.clone(),
    };

    match cli.cmd {
        None => {
            let app = build_app(&cli, &cfg);
            let password = read_password(&cfg, cli.password_stdin)?;
            run_tui(app, cfg, password)
        }
        Some(Cmd::Doctor { json, live }) => {
            let report = headless::doctor(&cfg, &cfg_path.display().to_string(), live);
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                println!("scope: {}", report.scope);
                for c in &report.capabilities {
                    println!("  {:<34} {:<12} {}", c.id, c.state, c.detail);
                }
            }
            // A failed check must be visible to scripts in the exit code too.
            if let Some(failed) = report.capabilities.iter().find(|c| c.state == "failed") {
                let detail = failed.detail.trim_start_matches("network: ").to_string();
                return Err(match failed.id.as_str() {
                    "reachability" => ApiError::Network(detail),
                    "auth" => ApiError::Auth(detail),
                    _ => ApiError::Config(detail),
                });
            }
            Ok(())
        }
        Some(Cmd::Status { json, waybar }) => {
            let snap = match connect(&cfg, cli.password_stdin)
                .and_then(|mut c| headless::collect(&mut c, &cfg.router.ip))
            {
                Ok(s) => Some(s),
                Err(e) => {
                    if waybar {
                        // A bar must always get a document, even when the link is down.
                        let payload = headless::status_waybar(None, Some(&e.to_string()));
                        println!("{}", serde_json::to_string(&payload).unwrap());
                    }
                    return Err(e);
                }
            };
            let snap = snap.expect("checked above");
            if waybar {
                let payload = headless::status_waybar(Some(&snap), None);
                println!("{}", serde_json::to_string(&payload).unwrap());
            } else if json {
                println!(
                    "{}",
                    serde_json::to_string(&headless::status_json(&snap)).unwrap()
                );
            } else {
                println!(
                    "{}\t{}\t{:.0} dBm",
                    snap.plmn.name,
                    snap.connection.net_type,
                    snap.signal.rsrp_dbm.unwrap_or(0.0)
                );
            }
            Ok(())
        }
        Some(Cmd::Record {
            out,
            keep,
            interval,
            count,
        }) => {
            let keep_secs = record::parse_duration(&keep).map_err(ApiError::Config)?;
            let path = out.unwrap_or_else(|| {
                record::default_dir().join(format!("{}.rwr", record::now_unix()))
            });
            let mut client = connect(&cfg, cli.password_stdin)?;
            let mut written = 0u64;
            loop {
                match headless::collect(&mut client, &cfg.router.ip) {
                    Ok(snap) => {
                        record::append(&path, &record::RecordFrame::capture(snap))
                            .map_err(|e| ApiError::Config(format!("recording: {e}")))?;
                        written += 1;
                        eprintln!("recorded {written} → {}", path.display());
                    }
                    Err(e) => eprintln!("routwatch: sample failed ({e}) — continuing"),
                }
                if let Some(limit) = count {
                    if written >= limit {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(interval.max(1)));
            }
            let kept = record::prune(&path, keep_secs, record::now_unix())
                .map_err(|e| ApiError::Config(format!("recording: {e}")))?;
            eprintln!("kept {kept} frames in {}", path.display());
            Ok(())
        }
        Some(Cmd::Replay { file, json, speed }) => {
            let frames = record::load(&file)
                .map_err(|e| ApiError::Config(format!("{}: {e}", file.display())))?;
            if frames.is_empty() {
                return Err(ApiError::Config(format!(
                    "{} holds no frames",
                    file.display()
                )));
            }
            if json {
                println!("{}", serde_json::to_string(&frames).unwrap());
                return Ok(());
            }
            let mut app = ui_opts.build(&cfg);
            run_replay(&mut app, frames, speed.max(1))
        }
        Some(Cmd::Snapshot { json, raw }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            if raw {
                for spec in routwatch::router::endpoints::PROBE.iter().take(8) {
                    match client.raw(spec.path) {
                        Ok(body) => println!("\n##### {} #####\n{}", spec.path, body.trim()),
                        Err(e) => println!("\n##### {} #####\n<error: {e}>", spec.path),
                    }
                }
                client.logout();
                return Ok(());
            }
            let snap = headless::collect(&mut client, &cfg.router.ip)?;
            client.logout();
            if json {
                println!("{}", serde_json::to_string_pretty(&snap).unwrap());
            } else {
                print_human(&snap);
            }
            Ok(())
        }
        Some(Cmd::Bands {
            list,
            json,
            set,
            mode,
            option,
            dry_run,
            allow_write,
            force,
        }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            if let Some(list_arg) = set {
                let mode_code = bands::mode_code(&mode).ok_or_else(|| {
                    ApiError::Config(format!("unknown mode {mode:?}; use auto, 4g or 5g"))
                })?;
                let selection = bands::parse_list(&list_arg).map_err(ApiError::Config)?;
                let all_mask = client
                    .get("net/net-mode-list")
                    .ok()
                    .and_then(|n| bands::from_net_mode_list(&n).1)
                    .map(bands::to_hex);
                let option_code = match &option {
                    Some(o) => Some(
                        bands::option_code(o)
                            .ok_or_else(|| {
                                ApiError::Config(format!(
                                    "unknown NR option {o:?}; use nsa, sa or both"
                                ))
                            })?
                            .to_string(),
                    ),
                    None => None,
                };
                let payload = bands::resolve(
                    mode_code,
                    &selection,
                    all_mask.as_deref(),
                    option_code.as_deref(),
                );
                let (current, current_mask) = write::read_mask(&mut client)?;
                println!(
                    "current: mode={} mask 0x{} -> {}",
                    current.network_mode.clone().unwrap_or_else(|| "—".into()),
                    current_mask.clone().unwrap_or_else(|| "—".into()),
                    current_mask
                        .as_deref()
                        .and_then(bands::from_hex)
                        .map(bands::bands_from_mask)
                        .unwrap_or_default()
                        .iter()
                        .map(|b| format!("B{b}"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                println!(
                    "target:  mode={} mask 0x{} ({})",
                    payload.network_mode,
                    payload.lte_band,
                    selection
                        .iter()
                        .map(|b| format!("B{b}"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                println!("payload: {}", payload.xml());
                if dry_run {
                    println!("dry-run: nothing was sent");
                    client.logout();
                    return Ok(());
                }
                if !allow_write {
                    client.logout();
                    return Err(ApiError::Config(
                        "refusing to write: pass --allow-write (or run the TUI and confirm with y)"
                            .into(),
                    ));
                }
                let intent = WriteIntent::ApplyBands {
                    mode: payload.network_mode.clone(),
                    bands: selection.clone(),
                    mask: payload.lte_band.clone(),
                    option: payload.network_option.clone(),
                };
                let mut progress = |st: WriteState| eprintln!("  {}", st.label());
                let settled =
                    write::apply_bands(&mut client, &payload, intent, force, &mut progress);
                client.logout();
                match settled {
                    WriteState::Success { .. } => {
                        println!("result: {}", settled.label());
                        Ok(())
                    }
                    other => {
                        println!("result: {}", other.label());
                        Err(ApiError::Router(0))
                    }
                }
            } else {
                let node = client.get("net/net-mode-list")?;
                let (defs, all) = bands::from_net_mode_list(&node);
                let mode = parse::parse_net_mode(&client.get("net/net-mode")?);
                client.logout();

                if json {
                    let payload = serde_json::json!({
                        "supported": defs.iter().map(|d| serde_json::json!({
                            "band": d.band, "label": d.label, "mask": format!("{:X}", d.value)
                        })).collect::<Vec<_>>(),
                        "all_bands_mask": all.map(bands::to_hex),
                        "current": mode,
                        "current_bands": mode.lte_band.as_deref()
                            .and_then(bands::from_hex)
                            .map(bands::bands_from_mask),
                    });
                    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
                } else if list {
                    println!(
                        "device supports {} LTE bands; all-bands mask {}",
                        defs.len(),
                        all.map(bands::to_hex).unwrap_or_else(|| "unknown".into())
                    );
                    for d in &defs {
                        println!("  {:<5} mask 0x{:X}", d.label, d.value);
                    }
                    if let Some(hex) = &mode.lte_band {
                        let current = bands::from_hex(hex)
                            .map(bands::bands_from_mask)
                            .unwrap_or_default();
                        // Read-back check M3 relies on: the device mask must equal the
                        // mask rebuilt from the band list we derived from it.
                        let rebuilt = bands::mask_from_bands(&current);
                        let verdict = match bands::from_hex(hex) {
                            Some(device) if device == rebuilt => "mask verified".to_string(),
                            Some(device) => format!(
                                "mask mismatch: device 0x{:X} rebuilt 0x{:X}",
                                device, rebuilt
                            ),
                            None => "mask unreadable".to_string(),
                        };
                        println!(
                            "current: mode={} mask 0x{} -> {} ({verdict})",
                            mode.network_mode.unwrap_or_default(),
                            hex,
                            current
                                .iter()
                                .map(|b| format!("B{b}"))
                                .collect::<Vec<_>>()
                                .join(",")
                        );
                    }
                } else {
                    println!("use --list or --json");
                }
                Ok(())
            }
        }
        Some(Cmd::Dns {
            set,
            reset,
            dry_run,
            allow_write,
            force,
            json,
        }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            let current = client
                .get("dhcp/settings")
                .ok()
                .map(|n| parse::parse_dns(&n));
            if json {
                println!("{}", serde_json::to_string_pretty(&current).unwrap());
                client.logout();
                return Ok(());
            }
            let (primary, secondary) = match &set {
                Some(raw) => write::parse_dns_pair(raw).map_err(ApiError::Config)?,
                None => (String::new(), String::new()),
            };
            if !reset && set.is_none() {
                if let Some(d) = &current {
                    println!(
                        "dns: status={} primary={} secondary={}",
                        d.status.clone().unwrap_or_else(|| "—".into()),
                        d.primary.clone().unwrap_or_else(|| "—".into()),
                        d.secondary.clone().unwrap_or_else(|| "—".into())
                    );
                }
                println!("use --set PRIMARY,SECONDARY or --reset");
                client.logout();
                return Ok(());
            }
            let (status, p, sec) = if reset {
                ("1", "", "")
            } else {
                ("0", primary.as_str(), secondary.as_str())
            };
            println!(
                "current: status={} primary={} secondary={}",
                current
                    .as_ref()
                    .and_then(|d| d.status.clone())
                    .unwrap_or_else(|| "—".into()),
                current
                    .as_ref()
                    .and_then(|d| d.primary.clone())
                    .unwrap_or_else(|| "—".into()),
                current
                    .as_ref()
                    .and_then(|d| d.secondary.clone())
                    .unwrap_or_else(|| "—".into())
            );
            println!(
                "target:  status={status} primary={} secondary={}",
                if p.is_empty() { "—" } else { p },
                if sec.is_empty() { "—" } else { sec }
            );
            println!(
                "payload: <?xml version=\"1.0\" encoding=\"UTF-8\"?><request><DnsStatus>{status}</DnsStatus><PrimaryDns>{p}</PrimaryDns><SecondaryDns>{sec}</SecondaryDns></request>"
            );
            if dry_run {
                println!("dry-run: nothing was sent");
                client.logout();
                return Ok(());
            }
            if !allow_write {
                client.logout();
                return Err(ApiError::Config(
                    "refusing to write: pass --allow-write (or run the TUI and confirm with y)"
                        .into(),
                ));
            }
            let intent = if reset {
                WriteIntent::ResetDns
            } else {
                WriteIntent::SetDns {
                    primary: primary.clone(),
                    secondary: secondary.clone(),
                }
            };
            let mut progress = |st: WriteState| eprintln!("  {}", st.label());
            let settled = write::apply_dns(
                &mut client,
                &primary,
                &secondary,
                intent,
                force,
                &mut progress,
            );
            client.logout();
            println!("result: {}", settled.label());
            match settled {
                WriteState::Success { .. } => Ok(()),
                _ => Err(ApiError::Router(0)),
            }
        }
        Some(Cmd::Sms { inbox, json }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            let counters = client
                .get("sms/sms-count")
                .ok()
                .map(|n| parse::parse_sms_count(&n));
            let attempt = if inbox {
                Some(client.sms_list(1))
            } else {
                None
            };
            client.logout();
            let (messages, error) = match attempt {
                None => (Vec::new(), None),
                Some(Ok(node)) => (parse::parse_sms_list(&node), None),
                Some(Err(e)) => (Vec::new(), Some(format!("{} ({})", e, e.class()))),
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "counters": counters,
                        "inbox_attempted": inbox,
                        "messages": messages,
                        "error": error,
                    }))
                    .unwrap()
                );
            } else {
                match &counters {
                    Some(c) => println!(
                        "counters: inbox {} ({} unread) · outbox {} · draft {} · capacity {}",
                        c.inbox, c.unread, c.outbox, c.draft, c.max
                    ),
                    None => println!("counters: unavailable"),
                }
                match &error {
                    Some(e) => println!(
                        "inbox: blocked — {e}\n        read them at http://192.168.8.1 → SMS"
                    ),
                    None if inbox => println!("inbox: {} messages readable", messages.len()),
                    None => println!("inbox: not attempted (pass --inbox)"),
                }
            }
            // A requested inbox that the firmware refused is a failure the caller
            // must see in the exit code, even though the counters succeeded.
            if inbox && error.is_some() {
                return Err(ApiError::Quiet(1));
            }
            Ok(())
        }
        Some(Cmd::Device { json }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            let device = client
                .get("device/information")
                .ok()
                .map(|n| parse::parse_device(&n));
            let pin = client.pin_status().ok().map(|n| parse::parse_pin(&n));
            let data = client
                .mobile_data_switch()
                .ok()
                .map(|n| parse::parse_mobile_data(&n));
            client.logout();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "device": device,
                        "pin": pin,
                        "mobile_data": data,
                    }))
                    .unwrap()
                );
            } else {
                if let Some(d) = &device {
                    println!(
                        "device: {} · {} · wan {} · uptime {}",
                        d.name.clone().unwrap_or_default(),
                        d.software.clone().unwrap_or_default(),
                        d.wan_ip.clone().unwrap_or_default(),
                        d.uptime_secs.map(|u| format!("{u}s")).unwrap_or_default()
                    );
                }
                if let Some(p) = &pin {
                    println!(
                        "sim: state {} · pin {} · retries {} / puk {}{}",
                        p.sim_state.clone().unwrap_or_default(),
                        p.pin_state.clone().unwrap_or_default(),
                        p.pin_retries.clone().unwrap_or_default(),
                        p.puk_retries.clone().unwrap_or_default(),
                        if p.locked() {
                            "  ⚠ waiting for a PIN"
                        } else {
                            ""
                        }
                    );
                }
                if let Some(md) = data {
                    println!("mobile data: {}", if md { "on" } else { "off" });
                }
            }
            Ok(())
        }
        Some(Cmd::Restart {
            dry_run,
            allow_write,
        }) => {
            println!("payload: {}", HuaweiClient::restart_payload());
            if dry_run {
                println!("dry-run: nothing was sent");
                return Ok(());
            }
            if !allow_write {
                return Err(ApiError::Config(
                    "refusing to restart: pass --allow-write after confirming nobody is using the link".into(),
                ));
            }
            let mut client = connect(&cfg, cli.password_stdin)?;
            let result = client.restart();
            client.logout();
            match result {
                Ok(_) => {
                    println!("restart accepted — the router will drop the link for ~60s");
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Some(Cmd::Ping { host, count, json }) => {
            let output = std::process::Command::new("ping")
                .args(worker::ping_args(count, &host))
                .output()
                .map_err(|e| {
                    ApiError::Config(format!(
                        "cannot run ping: {e} — install iputils-ping (Debian/Ubuntu) or iputils (Arch/Fedora)"
                    ))
                })?;
            let text = String::from_utf8_lossy(&output.stdout).to_string()
                + &String::from_utf8_lossy(&output.stderr);
            let outcome = worker::parse_ping(&host, &text);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "host": outcome.host,
                        "sent": outcome.sent,
                        "received": outcome.received,
                        "loss_pct": outcome.loss_pct,
                        "avg_ms": outcome.avg_ms,
                        "ok": outcome.ok,
                    }))
                    .unwrap()
                );
            } else {
                println!("{host}: {}", outcome.detail);
                if let Some(avg) = outcome.avg_ms {
                    println!("rtt avg {avg:.1} ms");
                }
            }
            if outcome.ok {
                Ok(())
            } else {
                Err(ApiError::Network(outcome.detail))
            }
        }
        Some(Cmd::Dump { out, json }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            if json {
                let path = out.unwrap_or_else(|| std::path::PathBuf::from("routwatch-dump.json"));
                let bytes = headless::dump_to(&mut client, &cfg.router.ip, &path)?;
                client.logout();
                println!("dump written: {} ({bytes} bytes)", path.display());
                return Ok(());
            }
            let mut text = String::new();
            for spec in routwatch::router::endpoints::PROBE {
                text.push_str(&format!("\n##### {} #####\n", spec.path));
                match client.raw(spec.path) {
                    Ok(body) => text.push_str(body.trim()),
                    Err(e) => text.push_str(&format!("<error: {e}>")),
                }
                text.push('\n');
            }
            client.logout();
            match out {
                Some(path) => {
                    std::fs::write(&path, &text)
                        .map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
                    println!("wrote {}", path.display());
                }
                None => print!("{text}"),
            }
            Ok(())
        }
        Some(Cmd::Preview {
            ref size,
            ref fixture,
            demo_confirm,
            demo_settings,
            ansi,
        }) => {
            let (w, h) = parse_size(size)?;
            let mut app = build_app(&cli, &cfg);
            let snap: Snapshot = match fixture {
                Some(path) => {
                    let raw = std::fs::read_to_string(path)
                        .map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
                    serde_json::from_str(&raw)
                        .map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?
                }
                None => {
                    let mut client = connect(&cfg, cli.password_stdin)?;
                    let s = headless::collect(&mut client, &cfg.router.ip)?;
                    client.logout();
                    s
                }
            };
            app.on_snapshot(snap, 0);
            if demo_confirm {
                app.begin_confirm();
            }
            if demo_settings {
                app.settings_open = true;
            }
            if ansi {
                print!("{}", ui::render_ansi(&app, w, h));
            } else {
                print!("{}", ui::render_to_string(&app, w, h));
            }
            Ok(())
        }
    }
}

/// UI-only options, captured so commands can build an App after `cli.cmd` moves.
struct UiOpts {
    theme: Option<String>,
    view: Option<String>,
    graph: Option<String>,
    tab: Option<String>,
    config_path: Option<PathBuf>,
}

impl UiOpts {
    fn build(&self, cfg: &config::Config) -> App {
        let theme = self.theme.clone().unwrap_or_else(|| cfg.app.theme.clone());
        let view = self
            .view
            .as_deref()
            .and_then(View::parse)
            .or_else(|| View::parse(&cfg.app.view))
            .unwrap_or(View::Full);
        let graph = self
            .graph
            .as_deref()
            .map(GraphStyle::parse)
            .unwrap_or_else(|| GraphStyle::parse(&cfg.app.graph_style));
        let mut app = App::new(
            Theme::get(&theme),
            graph,
            view,
            cfg.app.budget_gb,
            cfg.app.refresh_ms,
        );
        if let Some(tab) = self.tab.as_deref().and_then(Tab::parse) {
            app.tab = tab;
        }
        if let Some(presets) = &cfg.app.presets {
            app.band_presets = presets.clone();
        }
        app.report_identity = cfg.app.report_identity;
        app.config_path = self.config_path.clone();
        app
    }
}

fn build_app(cli: &Cli, cfg: &config::Config) -> App {
    let theme = cli.theme.clone().unwrap_or_else(|| cfg.app.theme.clone());
    let view = cli
        .view
        .as_deref()
        .and_then(View::parse)
        .or_else(|| View::parse(&cfg.app.view))
        .unwrap_or(View::Full);
    let graph = cli
        .graph
        .as_deref()
        .map(GraphStyle::parse)
        .unwrap_or_else(|| GraphStyle::parse(&cfg.app.graph_style));
    let mut app = App::new(
        Theme::get(&theme),
        graph,
        view,
        cfg.app.budget_gb,
        cfg.app.refresh_ms,
    );
    if let Some(tab) = cli.tab.as_deref().and_then(Tab::parse) {
        app.tab = tab;
    }
    if let Some(presets) = &cfg.app.presets {
        app.band_presets = presets.clone();
    }
    app.report_identity = cli.report_identity || cfg.app.report_identity;
    app.config_path = cli.config.clone();
    app
}

fn parse_size(s: &str) -> Result<(u16, u16), ApiError> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| ApiError::Config(format!("bad size {s:?}; expected WIDTHxHEIGHT")))?;
    Ok((
        w.trim().parse().unwrap_or(100),
        h.trim().parse().unwrap_or(30),
    ))
}

fn read_password(cfg: &config::Config, password_stdin: bool) -> Result<String, ApiError> {
    if password_stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| ApiError::Config(format!("stdin: {e}")))?;
        Ok(buf.trim_end_matches(['\n', '\r']).to_string())
    } else {
        config::resolve_password(&cfg.router.password)
    }
}

fn connect(cfg: &config::Config, password_stdin: bool) -> Result<HuaweiClient, ApiError> {
    let password = read_password(cfg, password_stdin)?;
    let mut client = HuaweiClient::new(&cfg.router.ip, cfg.app.timeout_ms);
    client.login(&cfg.router.username, &password)?;
    Ok(client)
}

// --------------------------------------------------------------------- TUI

fn run_tui(mut app: App, cfg: config::Config, password: String) -> Result<(), ApiError> {
    let (tx, rx) = worker::spawn(cfg, password);

    enable_raw_mode().map_err(|e| ApiError::Config(format!("raw mode: {e}")))?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .map_err(|e| ApiError::Config(format!("alt screen: {e}")))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| ApiError::Config(format!("terminal: {e}")))?;
    terminal.clear().ok();

    while !app.quit {
        drain(&mut app, &rx);
        terminal.draw(|f| ui::draw(f, &app)).ok();

        if event::poll(Duration::from_millis(200)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => handle_key(&mut app, k, &tx),
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    let _ = tx.send(WorkerCmd::Stop);
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    drop(terminal);
    println!(
        "routwatch: session closed (last sample {}s old)",
        app.age().map(|a| a.as_secs()).unwrap_or(0)
    );
    Ok(())
}

/// Replay recorded frames into the same UI, with no router contact at all.
fn run_replay(app: &mut App, frames: Vec<record::RecordFrame>, speed: u64) -> Result<(), ApiError> {
    enable_raw_mode().map_err(|e| ApiError::Config(format!("raw mode: {e}")))?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .map_err(|e| ApiError::Config(format!("alt screen: {e}")))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| ApiError::Config(format!("terminal: {e}")))?;
    terminal.clear().ok();

    let tick = Duration::from_millis((1000 / speed.max(1)).max(20));
    let mut paused = false;
    let mut quit = false;
    let mut idx = 0usize;

    while idx < frames.len() && !quit {
        let frame = &frames[idx];
        app.on_snapshot(frame.snapshot.clone(), 0);
        terminal.draw(|f| ui::draw(f, app)).ok();

        let mut held = Duration::ZERO;
        while held < tick || paused {
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            quit = true;
                            break;
                        }
                        KeyCode::Char(' ') => paused = !paused,
                        KeyCode::Char('.') if paused => break,
                        _ => {}
                    },
                    Ok(_) => {}
                    Err(_) => {
                        quit = true;
                        break;
                    }
                }
            } else {
                held += Duration::from_millis(50);
            }
        }
        idx += 1;
    }

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    drop(terminal);
    println!(
        "routwatch: replay finished — {} frames",
        frames.len().min(idx)
    );
    Ok(())
}

fn drain(app: &mut App, rx: &Receiver<Ev>) {
    while let Ok(ev) = rx.try_recv() {
        match ev {
            Ev::Snapshot { snap, poll_ms } => app.on_snapshot(*snap, poll_ms),
            Ev::Session(s) => app.session = s,
            Ev::Error(e) => app.on_error(e),
            Ev::Write(state) => app.settle_write(state),
            Ev::Ping(outcome) => app.set_ping(outcome),
            Ev::Extras(extras) => app.on_extras(*extras),
            Ev::Phase { phase, state } => app.on_phase(phase, state),
            Ev::Notice(msg) => {
                app.note(msg.clone());
                app.log(routwatch::app::LogLevel::Info, msg);
            }
        }
    }
}

fn handle_key(app: &mut App, k: KeyEvent, tx: &Sender<WorkerCmd>) {
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }

    // The settings overlay is modal for appearance keys.
    if app.settings_open {
        match k.code {
            KeyCode::Esc | KeyCode::Char('S') => app.settings_open = false,
            KeyCode::Char('t') => {
                app.theme = Theme::get(routwatch::theme::next_name(&app.theme.id));
            }
            KeyCode::Char('v') => app.view = app.view.next(),
            KeyCode::Char('g') => app.graph = app.graph.next(),
            KeyCode::Char('[') => app.adjust_refresh(-500),
            KeyCode::Char(']') => app.adjust_refresh(500),
            KeyCode::Char('-') => app.adjust_budget(-10.0),
            KeyCode::Char('+') => app.adjust_budget(10.0),
            KeyCode::Char('w') => save_settings(app),
            KeyCode::Char('i') => app.report_identity = !app.report_identity,
            _ => {}
        }
        return;
    }

    // A text input owns the keyboard first.
    if app.input.is_some() {
        match k.code {
            KeyCode::Esc => app.input = None,
            KeyCode::Enter => {
                if let Some((kind, value)) = app.input_take() {
                    match kind {
                        InputKind::PingHost => {
                            if value.is_empty() {
                                app.note("no host given");
                            } else {
                                let _ = tx.send(WorkerCmd::Ping(value.clone()));
                                app.note(format!("pinging {value}…"));
                            }
                        }
                        InputKind::Dns => {
                            if let Err(e) = app.begin_dns_confirm(&value) {
                                app.note(e);
                            }
                        }
                        InputKind::RestartConfirm => {
                            if routwatch::app::restart_allowed(&value) {
                                let _ = tx.send(WorkerCmd::Restart);
                                app.note("restart sent — waiting for the router");
                            } else {
                                app.note(format!(
                                    "not sent: type {} exactly to restart",
                                    App::restart_word()
                                ));
                            }
                        }
                    }
                }
            }
            KeyCode::Backspace => app.input_backspace(),
            // A lone Esc followed immediately by a character can arrive as Alt+char;
            // never let that write into the field.
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::ALT) => app.input_push(c),
            _ => {}
        }
        return;
    }

    // A modal owns the keyboard: only its own answers are accepted.
    let confirming = matches!(app.write, WriteState::Confirming(_));
    if app.write.is_busy() {
        return;
    }
    if confirming {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => send_apply(app, tx),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_confirm(),
            _ => {}
        }
        return;
    }
    if app.write.is_settled() {
        if matches!(k.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ')) {
            app.settle_write(WriteState::Idle);
        }
        return;
    }

    let on_bands = app.tab == Tab::Bands;
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    // Letters answer in either case: the reference tools (syswatch, diskwatch)
    // accept `v` exactly like `V`. Shift keeps its own meaning where one exists
    // (D = DNS reset, R = lazy extras), and each of those also has a case-free
    // alias below so nothing depends on holding Shift.
    let code = normalise_key(k.code);
    match code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('?') => app.help = !app.help,
        KeyCode::Esc => app.help = false,
        KeyCode::Char('v') => app.cycle_view(),
        KeyCode::Char('l') => app.toggle_lite(),
        KeyCode::Char('g') => app.graph = app.graph.next(),
        KeyCode::Char('r') if shift && matches!(app.tab, Tab::Sms | Tab::Device) => {
            let _ = tx.send(WorkerCmd::Extras);
            app.mark_extras_requested();
            app.note("reading inbox + SIM state…");
        }
        KeyCode::Char('r') => {
            let _ = tx.send(WorkerCmd::Refresh);
        }
        KeyCode::Char('e') if matches!(app.tab, Tab::Sms | Tab::Device) => {
            let _ = tx.send(WorkerCmd::Extras);
            app.mark_extras_requested();
            app.note("reading inbox + SIM state…");
        }
        // Must stay AFTER the SMS/Device arm above: those two sections own `e`.
        KeyCode::Char('e') => export_report(app),
        KeyCode::Char('p') => {
            app.paused = !app.paused;
            let _ = tx.send(WorkerCmd::Pause(app.paused));
        }
        // --- Traffic / DNS tab ---
        KeyCode::Char('d') if matches!(app.tab, Tab::Traffic | Tab::Dns) => {
            if shift {
                app.begin_dns_reset();
            } else {
                app.open_input(InputKind::Dns);
            }
        }
        KeyCode::Char('z') if matches!(app.tab, Tab::Traffic | Tab::Dns) => app.begin_dns_reset(),
        KeyCode::Enter | KeyCode::Char('a') if app.tab == Tab::Dns => {
            // The Custom row opens the prompt instead of confirming a fixed pair.
            if app.dns_preset_is_custom() {
                app.open_input(InputKind::Dns);
            } else {
                app.begin_dns_preset();
            }
        }
        // --- Network tab: local ping ---
        KeyCode::Char('t') if app.tab == Tab::Network => {
            app.open_input(InputKind::PingHost);
        }
        // --- Clients tab: sort / copy ---
        KeyCode::Char('s') if app.tab == Tab::Clients => {
            app.sort_next();
        }
        KeyCode::Char('c') if app.tab == Tab::Clients => copy_selected(app),
        // --- Device: dump / restart prompt ---
        KeyCode::Char('x') if app.tab == Tab::Device => {
            let path = dump_path();
            app.note(format!("writing dump to {}…", path.display()));
            let _ = tx.send(WorkerCmd::Dump(path));
        }
        KeyCode::Char('b') if app.tab == Tab::Device => {
            app.open_input(InputKind::RestartConfirm);
        }
        // --- Bands tab: merge presets (load, save the tick, delete) ---
        KeyCode::Char('n') if on_bands => match app.band_preset_next() {
            Some(name) => {
                let mask = app.band_mask_hex();
                app.note(format!("preset {name} → 0x{mask} (a applies it)"));
            }
            None => app.note("no preset here: s saves the ticked bands"),
        },
        KeyCode::Char('s') if on_bands => match app.band_preset_save() {
            PresetSaved::Saved(preset) => match persist_presets(app) {
                Ok(path) => app.note(format!("merge {} saved to {}", preset.name, path.display())),
                Err(e) => app.note(format!("merge {} not saved: {e}", preset.name)),
            },
            PresetSaved::Duplicate(i) => {
                app.note(format!("that merge is already preset #{}", i + 1))
            }
            PresetSaved::Empty => app.note("tick bands first (space), then s saves the merge"),
        },
        KeyCode::Char('x') if on_bands => match app.band_preset_delete() {
            Some(preset) => match persist_presets(app) {
                Ok(path) => app.note(format!(
                    "preset {} deleted from {}",
                    preset.name,
                    path.display()
                )),
                Err(e) => app.note(format!(
                    "preset {} removed for this session only: {e}",
                    preset.name
                )),
            },
            None => app.note("no preset selected: n cycles them"),
        },
        // Settings: `,` is the family convention, `s` works everywhere except the
        // Clients tab (where it sorts), and Shift+S still opens it there.
        KeyCode::Char(',') => app.settings_open = true,
        KeyCode::Char('s') => app.settings_open = true,
        KeyCode::Char(c @ '1'..='9') => {
            if let Some(tab) = ui::tab_from_key(c) {
                // A digit always reaches its section, even from Lite/Dense.
                app.select_tab(tab);
                if matches!(tab, Tab::Sms | Tab::Device) && app.needs_extras() {
                    let _ = tx.send(WorkerCmd::Extras);
                    app.mark_extras_requested();
                }
            }
        }
        KeyCode::Tab => app.select_tab(app.tab.next()),
        KeyCode::BackTab => app.select_tab(app.tab.prev()),
        // --- Bands tab ---
        KeyCode::Char(' ') if on_bands => {
            app.band_toggle();
        }
        KeyCode::Char('m') if on_bands => {
            app.cycle_mode();
        }
        KeyCode::Char('o') if on_bands => {
            app.cycle_option();
        }
        KeyCode::Char('a') if on_bands => {
            app.begin_confirm();
        }
        // --- selection movement ---
        KeyCode::Down | KeyCode::Char('j') => {
            if on_bands {
                app.band_cursor_move(1);
            } else if app.tab == Tab::Dns {
                app.dns_preset_move(1);
            } else {
                app.client_cursor(1);
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if on_bands {
                app.band_cursor_move(-1);
            } else if app.tab == Tab::Dns {
                app.dns_preset_move(-1);
            } else {
                app.client_cursor(-1);
            }
        }
        _ => {}
    }
}

/// Letters are matched case-insensitively: `v` must do what `V` does, like the
/// reference tools. Digits, arrows and modifiers pass through untouched.
fn normalise_key(code: KeyCode) -> KeyCode {
    match code {
        KeyCode::Char(c) if c.is_ascii_alphabetic() => KeyCode::Char(c.to_ascii_lowercase()),
        other => other,
    }
}

/// Persist the appearance/polling settings the overlay exposes.
fn save_settings(app: &mut App) {
    let result = routwatch::config::load(app.config_path.clone()).and_then(|mut cfg| {
        cfg.app.theme = app.theme.id.clone();
        cfg.app.view = app.view.name().to_string();
        cfg.app.graph_style = app.graph.name().to_string();
        cfg.app.refresh_ms = app.refresh_ms;
        cfg.app.budget_gb = app.budget_gb as u64;
        cfg.app.report_identity = app.report_identity;
        routwatch::config::save(app.config_path.clone(), &cfg)
    });
    match result {
        Ok(path) => app.note(format!("settings saved to {}", path.display())),
        Err(e) => app.note(format!("save failed: {e}")),
    }
}

/// Write the merge presets back into config.toml (0600, through `config::save`).
fn persist_presets(app: &mut App) -> Result<std::path::PathBuf, String> {
    routwatch::config::load(app.config_path.clone())
        .and_then(|mut cfg| {
            cfg.app.presets = Some(app.band_presets.clone());
            routwatch::config::save(app.config_path.clone(), &cfg)
        })
        .map_err(|e| e.to_string())
}

/// `e`: write the last sample as a Markdown report. Redacted unless the user
/// opted into identity (`--report-identity`, the config key, or `i` in settings).
fn export_report(app: &mut App) {
    let Some(snap) = app.snap.clone() else {
        app.note("no sample yet — nothing to report");
        return;
    };
    let Some(dir) = routwatch::report::dir() else {
        app.note("no data directory; report not written");
        return;
    };
    let when = routwatch::record::now_unix();
    let redaction = app.report_redaction();
    let mode = if redaction.identity {
        "full"
    } else {
        "redacted"
    };
    match routwatch::report::write(&snap, &dir, when, redaction) {
        Ok(path) => {
            app.note(format!("{mode} report written: {}", path.display()));
            app.log(
                routwatch::app::LogLevel::Info,
                format!("report ({mode}): {}", path.display()),
            );
        }
        Err(e) => app.note(format!("report failed: {e}")),
    }
}

fn dump_path() -> std::path::PathBuf {
    let base = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("routwatch");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    base.join(format!("dump-{stamp}.json"))
}

/// Copy text through whichever clipboard tool this desktop ships.
///
/// Wayland has `wl-copy`, X11 has `xclip` or `xsel`, and a bare server has
/// none — so the first one present wins instead of assuming a compositor.
/// Returns the tool that accepted the text.
fn clipboard_copy(text: &str) -> Option<&'static str> {
    use std::io::Write as _;
    let attempts: [(&str, &[&str]); 3] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--input", "--clipboard"]),
    ];
    for (tool, args) in attempts {
        let Ok(mut child) = std::process::Command::new(tool)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            continue; // not installed on this distro/desktop: try the next
        };
        let wrote = child
            .stdin
            .as_mut()
            .map(|stdin| stdin.write_all(text.as_bytes()).is_ok())
            .unwrap_or(false);
        if wrote && child.wait().map(|st| st.success()).unwrap_or(false) {
            return Some(tool);
        }
    }
    None
}

/// Copy the selected client's MAC (or IP) with wl-copy/xclip/xsel.
fn copy_selected(app: &mut App) {
    let Some(idx) = app.selected_client() else {
        app.note("no client selected");
        return;
    };
    let value = app
        .sorted_clients()
        .get(idx)
        .and_then(|c| c.mac.clone().or_else(|| c.ip.clone()));
    let Some(value) = value else {
        app.note("selected client has no MAC/IP");
        return;
    };
    match clipboard_copy(&value) {
        Some(tool) => app.note(format!("copied {value} ({tool})")),
        None => app.note(format!(
            "clipboard unavailable (install wl-clipboard or xclip) · {value}"
        )),
    }
}

/// Hand the confirmed intent to the router thread and show it as in flight
/// immediately, so the UI never looks idle while a write is queued.
fn send_apply(app: &mut App, tx: &Sender<WorkerCmd>) {
    let Some(intent) = app.write.intent().cloned() else {
        return;
    };
    match intent.clone() {
        WriteIntent::ApplyBands { .. } => {
            let Some(snap) = &app.snap else { return };
            let payload = bands::resolve(
                &app.band_mode,
                &app.bands_selected,
                snap.all_bands_mask.as_deref(),
                Some(app.band_option.as_str()),
            );
            let _ = tx.send(WorkerCmd::ApplyBands {
                payload,
                intent: intent.clone(),
            });
        }
        WriteIntent::SetDns { primary, secondary } => {
            let _ = tx.send(WorkerCmd::ApplyDns {
                primary,
                secondary,
                reset: false,
                intent: intent.clone(),
            });
        }
        WriteIntent::ResetDns => {
            let _ = tx.send(WorkerCmd::ApplyDns {
                primary: String::new(),
                secondary: String::new(),
                reset: true,
                intent: intent.clone(),
            });
        }
    }
    app.settle_write(WriteState::Pending {
        intent,
        since: std::time::Instant::now(),
    });
}

fn print_human(s: &Snapshot) {
    let sig = &s.signal;
    println!(
        "router   {}  ({} / {})",
        s.router,
        s.device.name.as_deref().unwrap_or("?"),
        s.device.software.as_deref().unwrap_or("?")
    );
    println!(
        "network  {} · {} · ENDC {} · bars {}/{}",
        s.plmn.name,
        s.connection.net_type,
        if s.connection.endc { "on" } else { "off" },
        s.connection.signal_bars,
        s.connection.nr_bars
    );
    println!(
        "signal   RSRP {} · RSRQ {} · SINR {} · RSSI {}",
        opt(sig.rsrp_dbm, "dBm"),
        opt(sig.rsrq_db, "dB"),
        opt(sig.sinr_db, "dB"),
        opt(sig.rssi_dbm, "dBm")
    );
    println!(
        "cell     {} · {} · PCI {} · EARFCN {} · {}",
        sig.band.as_deref().unwrap_or("—"),
        match (sig.dl_mhz, sig.ul_mhz) {
            (Some(d), Some(u)) => format!("{d}/{u} MHz"),
            _ => "—".to_string(),
        },
        sig.pci.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
        sig.earfcn.as_deref().unwrap_or("—"),
        sig.tm.as_deref().unwrap_or("—")
    );
    println!(
        "speed    down {}/s · up {}/s",
        human_bytes(s.traffic.dl_rate_bytes),
        human_bytes(s.traffic.ul_rate_bytes)
    );
    println!(
        "usage    session {} down / {} up · total {} down",
        human_bytes(s.traffic.session_dl_bytes),
        human_bytes(s.traffic.session_ul_bytes),
        human_bytes(s.traffic.total_dl_bytes)
    );
    println!("clients  {}", s.clients.len());
    for c in &s.clients {
        println!(
            "         {:<28} {:<15} {} {}",
            c.name.as_deref().unwrap_or("(unnamed)"),
            c.ip.as_deref().unwrap_or("—"),
            c.mac.as_deref().unwrap_or("—"),
            c.freq.as_deref().unwrap_or("")
        );
    }
    if let Some(nr) = &sig.nr {
        println!(
            "5g       RSRP {} · SINR {} · {}",
            opt(nr.rsrp_dbm, "dBm"),
            opt(nr.sinr_db, "dB"),
            nr.band.as_deref().unwrap_or("—")
        );
    } else {
        println!("5g       not exposed by this firmware");
    }
}

fn opt<T: std::fmt::Display>(v: Option<T>, unit: &str) -> String {
    v.map(|x| format!("{x} {unit}"))
        .unwrap_or_else(|| "—".into())
}

fn human_bytes(v: u64) -> String {
    const K: f64 = 1024.0;
    let f = v as f64;
    if f >= K * K * K * K {
        format!("{:.2} TB", f / (K * K * K * K))
    } else if f >= K * K * K {
        format!("{:.2} GB", f / (K * K * K))
    } else if f >= K * K {
        format!("{:.1} MB", f / (K * K))
    } else if f >= K {
        format!("{:.0} KB", f / K)
    } else {
        format!("{v} B")
    }
}

#[cfg(test)]
mod key_tests {
    use super::normalise_key;
    use ratatui::crossterm::event::KeyCode;
    use routwatch::ui::tab_from_key;

    #[test]
    fn letters_collapse_to_lowercase_and_nothing_else_moves() {
        assert_eq!(normalise_key(KeyCode::Char('V')), KeyCode::Char('v'));
        assert_eq!(normalise_key(KeyCode::Char('L')), KeyCode::Char('l'));
        assert_eq!(normalise_key(KeyCode::Char('D')), KeyCode::Char('d'));
        assert_eq!(normalise_key(KeyCode::Char('v')), KeyCode::Char('v'));

        // digits, punctuation and navigation keys are untouched
        assert_eq!(normalise_key(KeyCode::Char('9')), KeyCode::Char('9'));
        assert_eq!(normalise_key(KeyCode::Char(',')), KeyCode::Char(','));
        assert_eq!(normalise_key(KeyCode::Char('?')), KeyCode::Char('?'));
        assert_eq!(normalise_key(KeyCode::Enter), KeyCode::Enter);
        assert_eq!(normalise_key(KeyCode::Tab), KeyCode::Tab);
        assert_eq!(normalise_key(KeyCode::Down), KeyCode::Down);

        // the normalised form is what the tab lookup sees, so digits still map
        assert!(tab_from_key('9').is_some());
    }
}
