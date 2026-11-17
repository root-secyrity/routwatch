use clap::{Parser, Subcommand};
use routwatch::app::{App, InputKind, PresetSaved, Tab, View};
use routwatch::config;
use routwatch::error::ApiError;
use routwatch::headless;
use routwatch::model::{DnsInfo, Snapshot};
use routwatch::router::{bands, parse, HuaweiClient};
use routwatch::theme::{GraphStyle, Theme};
use routwatch::ui;
use routwatch::worker::{self, Cmd as WorkerCmd, Ev};
use routwatch::write::{self, WriteIntent, WriteState};
use routwatch::{completion, record};
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

/// Every section `Tab::parse` accepts, in tab order. Kept in one place so the
/// help cannot advertise a shorter list than the tool answers to;
/// `the_tab_help_names_every_section_it_accepts` holds the two together.
const TAB_HELP: &str =
    "Start on a tab: dashboard, bands, traffic, network, clients, sms, device, dns, signal";

/// Printed whenever the firmware's own `dhcpsdns_display` flag says the LAN DHCP
/// DNS page does not exist: a write there is ignored, so the CLI says so up front.
const DNS_NOT_EDITABLE: &str =
    "support: LAN DHCP DNS is not configurable on this firmware (dhcpsdns_display=0)";

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

    #[arg(long, global = true, help = TAB_HELP)]
    tab: Option<String>,

    /// Hide serial, IMEI, MACs, WAN IP and client names in `snapshot`, so the
    /// output can be pasted in a public issue
    #[arg(long, global = true)]
    redact: bool,

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
        /// With --raw, print every PROBE endpoint, not the first eight
        #[arg(long)]
        all: bool,
    },
    /// Read-only audit: remote management, DNS in use, admin surface
    Privacy {
        #[arg(long)]
        json: bool,
        /// Print the raw XML of every probed endpoint (contains device data)
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
        /// Keep watching the link after a successful write, for this many
        /// seconds. Nothing is reverted without a `y` on stdin.
        #[arg(long, value_name = "SECS")]
        watch: Option<u64>,
    },
    /// Try band sets, measure each, print a ranked table
    Scan {
        /// Comma separated sets, each set its own argument (e.g. --set 1,3 --set 20,28)
        #[arg(long = "set", value_name = "LIST", required = true)]
        sets: Vec<String>,
        #[arg(long, default_value = "auto")]
        mode: String,
        #[arg(long)]
        option: Option<String>,
        /// Upper bound in seconds for the link to come back before a set is
        /// called no-service. The wait ends as soon as an address returns.
        #[arg(long, default_value_t = 60)]
        settle: u64,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        allow_write: bool,
        #[arg(long)]
        allow_data: bool,
        #[arg(long)]
        json: bool,
        /// Do not add the runs to the saved speed history
        #[arg(long)]
        no_save: bool,
        /// Measure even when this host's route to the internet does not leave
        /// through the router
        #[arg(long)]
        ignore_route: bool,
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
    /// Restart the router - destructive, needs --allow-write; --dry-run prints only
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
        /// Replace an existing output file (default: refuse)
        #[arg(long)]
        force: bool,
    },
    /// One document to paste in a public issue when asking for a model to be
    /// supported: capabilities, endpoint field names, redacted snapshot
    Share {
        /// Where to write it (default: routwatch-share-<unix>.md)
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
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
    /// Download/upload test from this host through the CPE (plain HTTP)
    Speedtest {
        #[arg(long)]
        json: bool,
        /// Print the plan and exit without sending a byte
        #[arg(long)]
        dry_run: bool,
        /// Confirm the data this run will spend
        #[arg(long)]
        allow_data: bool,
        /// Skip the upload
        #[arg(long)]
        download_only: bool,
        #[arg(long, value_name = "URL")]
        download_url: Option<String>,
        #[arg(long, value_name = "URL")]
        upload_url: Option<String>,
        #[arg(long)]
        seconds: Option<u64>,
        #[arg(long)]
        max_mb: Option<u64>,
        #[arg(long)]
        upload_mb: Option<u64>,
        /// Do not add this run to the saved speed history
        #[arg(long)]
        no_save: bool,
        /// List the saved speed runs and exit (no network)
        #[arg(long)]
        history: bool,
    },
    /// Ask the network for a new WAN address
    Ip {
        #[command(subcommand)]
        action: IpAction,
    },
    /// Saved long-term history
    History {
        /// Signal rows (no network)
        #[arg(long)]
        signal: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
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
    /// Render one frame to stdout (no TTY needed) - review the layout anywhere
    Preview {
        /// WIDTHxHEIGHT, e.g. 100x30
        #[arg(long, default_value = "100x30")]
        size: String,
        /// Render from a saved snapshot JSON instead of contacting the router
        #[arg(long, value_name = "FILE")]
        fixture: Option<PathBuf>,
        /// Render the speed history panel from a saved JSONL ledger (review
        /// only: nothing is written back)
        #[arg(long, value_name = "FILE")]
        history_fixture: Option<PathBuf>,
        /// Layout review: open the apply-confirm modal (sends nothing)
        #[arg(long, hide = true)]
        demo_confirm: bool,
        /// Layout review: open the settings overlay
        #[arg(long, hide = true)]
        demo_settings: bool,
        /// Emit real ANSI colours instead of plain symbols - for README
        /// screenshots and for checking a theme without a terminal
        #[arg(long)]
        ansi: bool,
    },
}

#[derive(Subcommand)]
enum IpAction {
    /// Drop the data connection, bring it back, and watch for the address
    Renew {
        #[arg(long)]
        allow_write: bool,
        #[arg(long)]
        json: bool,
        /// Cap on the wait for an address, in seconds (a ceiling, not a duration)
        #[arg(long, default_value_t = 60)]
        settle: u64,
    },
}

fn main() {
    // Die quietly on a closed pipe (`routwatch snapshot | head`) instead of
    // panicking with "failed printing to stdout".
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // `--help` and `--version` are successes; every other parse failure
            // is the documented usage failure (exit 3), not clap's default 2
            // which this project uses for network errors.
            let ok = matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            let _ = e.print();
            std::process::exit(if ok { 0 } else { 3 });
        }
    };
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
    // Writing to the radio and spending data are the two ways `scan` can hurt.
    // The flag is answered before any config read, so a machine that was never
    // set up gets the same sentence instead of a missing-config error.
    if let Some(Cmd::Scan {
        dry_run,
        allow_write,
        allow_data,
        ..
    }) = &cli.cmd
    {
        if !dry_run && !allow_write {
            eprintln!("scan applies band sets: pass --allow-write, or --dry-run to see the plan");
            return Err(ApiError::Quiet(3));
        }
        if !dry_run && !allow_data {
            eprintln!("scan spends data: pass --allow-data, or --dry-run to see the plan");
            return Err(ApiError::Quiet(3));
        }
    }

    // Offline commands must work without a config file; everything that talks
    // to the router keeps the strict loader (and its 0600 check). `scan
    // --dry-run` prints a plan and touches nothing, so it belongs here;
    // `ip renew` does not, because it really writes.
    // A renew drops the link for a few seconds: it is a write, and the flag is
    // answered before any config read so a machine never set up hears the rule.
    if let Some(Cmd::Ip {
        action: IpAction::Renew { allow_write, .. },
    }) = &cli.cmd
    {
        if !allow_write {
            eprintln!("ip renew switches the data connection off and on: pass --allow-write");
            return Err(ApiError::Quiet(3));
        }
    }
    let offline = matches!(
        cli.cmd,
        Some(Cmd::Preview { .. })
            | Some(Cmd::Replay { .. })
            | Some(Cmd::Speedtest { .. })
            | Some(Cmd::History { .. })
            | Some(Cmd::Scan { dry_run: true, .. })
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
            let app = build_app(&cli, &cfg)?;
            let stdin_password = read_stdin_password(cli.password_stdin)?;
            let password = read_password(&cfg, stdin_password.as_deref())?;
            run_tui(app, cfg, password)
        }
        Some(Cmd::Privacy { json, raw }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            let audit = routwatch::privacy::Audit::collect(&mut client);
            let bodies: Vec<(&'static str, String)> = if raw {
                // Same probe list the audit read, so the two cannot drift apart.
                routwatch::router::endpoints::PRIVACY
                    .iter()
                    .filter_map(|spec| {
                        client
                            .raw(spec.path)
                            .ok()
                            .map(|body| (spec.path, routwatch::scrub::scrub_xml(&body)))
                    })
                    .collect()
            } else {
                Vec::new()
            };
            client.logout();
            if json {
                println!("{}", serde_json::to_string_pretty(&audit).unwrap());
            } else {
                for c in &audit.checks {
                    println!("{:<14} {:<11} {}", c.name, c.state.as_str(), c.detail);
                }
            }
            if raw {
                eprintln!("raw: endpoint XML with serial, IMEI, MACs and host names replaced by <redacted>");
                for (path, body) in &bodies {
                    eprintln!("\n##### {path} #####\n{}", body.trim());
                }
            }
            Ok(())
        }
        Some(Cmd::Doctor { json, live }) => {
            let stdin_password = read_stdin_password(cli.password_stdin)?;
            let report = headless::doctor(
                &cfg,
                &cfg_path.display().to_string(),
                live,
                stdin_password.as_deref(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                println!("scope: {}", report.scope);
                for c in &report.capabilities {
                    println!("  {:<34} {:<12} {}", c.id, c.state, c.detail);
                }
            }
            // Only the core capabilities decide the exit code: a firmware that
            // lacks an optional endpoint is still a healthy router.
            if let Some(failed) = crate::headless::first_core_failure(&report.capabilities) {
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
            let mut snap = snap.expect("checked above");
            if cli.redact {
                snap = routwatch::report::redacted(&snap);
            }
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
                    "{}\t{}\t{}",
                    snap.plmn.name,
                    snap.connection.net_type,
                    snap.signal
                        .rsrp_dbm
                        .map(|v| format!("{v:.0} dBm"))
                        .unwrap_or_else(|| "-".to_string())
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
            let mut failures = 0u32;
            loop {
                match headless::collect(&mut client, &cfg.router.ip) {
                    Ok(snap) => {
                        let snap = if cli.redact {
                            routwatch::report::redacted(&snap)
                        } else {
                            snap
                        };
                        record::append(&path, &record::RecordFrame::capture(snap))
                            .map_err(|e| ApiError::Config(format!("recording: {e}")))?;
                        written += 1;
                        failures = 0;
                        eprintln!("recorded {written} → {}", path.display());
                    }
                    Err(e) => {
                        failures += 1;
                        eprintln!("routwatch: sample failed ({e}) - {failures}/3");
                        if failures >= 3 {
                            client.logout();
                            return Err(ApiError::Network(format!(
                                "three samples in a row failed ({e}); giving up after {written} frames"
                            )));
                        }
                    }
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
            run_replay(&mut app, frames, speed.max(1), cli.redact)
        }
        Some(Cmd::Snapshot { json, raw, all }) => {
            // A flag that looks like protection and is not is worse than none:
            // the raw XML carries IMEI, serial and MACs in the clear.
            if raw && cli.redact {
                return Err(ApiError::Config(
                    "--redact cannot protect --raw: the raw XML carries IMEI, serial and MACs. \
                     Use `snapshot --json --redact` for the parsed view, or `dump --redact` for \
                     the same endpoints with those values replaced"
                        .into(),
                ));
            }
            let mut client = connect(&cfg, cli.password_stdin)?;
            if raw {
                let take = if all {
                    routwatch::router::endpoints::PROBE.len()
                } else {
                    8
                };
                for spec in routwatch::router::endpoints::PROBE.iter().take(take) {
                    match client.raw(spec.path) {
                        Ok(body) => println!("\n##### {} #####\n{}", spec.path, body.trim()),
                        Err(e) => println!("\n##### {} #####\n<error: {e}>", spec.path),
                    }
                }
                client.logout();
                return Ok(());
            }
            let mut snap = headless::collect(&mut client, &cfg.router.ip)?;
            client.logout();
            // `--redact` exists so the person who owns an unsupported router can
            // paste this in a public issue without leaking who they are.
            if cli.redact {
                snap = routwatch::report::redacted(&snap);
            }
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
            watch,
        }) => {
            if watch.is_some() && set.is_none() {
                return Err(ApiError::Usage(
                    "--watch needs --set: there is no write to watch".into(),
                ));
            }
            // The credentials are kept for the watch: a radio restart can drop
            // the session, and the watch has to sign in again by itself.
            let (mut client, creds) = connect_with(&cfg, cli.password_stdin)?;
            if let Some(list_arg) = set {
                let mode_code = bands::mode_code(&mode).ok_or_else(|| {
                    ApiError::Config(format!("unknown mode {mode:?}; use auto, 4g or 5g"))
                })?;
                let selection = bands::parse_list(&list_arg).map_err(ApiError::Config)?;
                let catalog = client
                    .get("net/net-mode-list")
                    .ok()
                    .map(|n| bands::from_net_mode_list(&n));
                let lte_all = catalog.as_ref().and_then(|c| c.lte_all).map(bands::to_hex);
                let network_all = catalog
                    .as_ref()
                    .and_then(|c| c.network_all)
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
                    lte_all.as_deref(),
                    network_all.as_deref(),
                    option_code.as_deref(),
                );
                let (current, current_mask) = write::read_mask(&mut client)?;
                println!(
                    "current: mode={} mask 0x{} -> {}",
                    current.network_mode.clone().unwrap_or_else(|| "-".into()),
                    current_mask.clone().unwrap_or_else(|| "-".into()),
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
                let limits = write::VerifyLimits::from_secs(cfg.app.verify_secs);
                let settled = write::apply_bands_with(
                    &mut client,
                    &payload,
                    intent,
                    force,
                    &mut progress,
                    limits,
                );
                let undo_payload = bands::BandPayload {
                    network_mode: current
                        .network_mode
                        .clone()
                        .unwrap_or_else(|| "00".to_string()),
                    network_band: network_all
                        .clone()
                        .unwrap_or_else(|| current_mask.clone().unwrap_or_else(|| "0".to_string())),
                    lte_band: current_mask.clone().unwrap_or_else(|| "0".to_string()),
                    network_option: current.network_option.clone(),
                };
                let undo_intent = WriteIntent::ApplyBands {
                    mode: undo_payload.network_mode.clone(),
                    bands: current_mask
                        .as_deref()
                        .and_then(bands::from_hex)
                        .map(bands::bands_from_mask)
                        .unwrap_or_default(),
                    mask: undo_payload.lte_band.clone(),
                    option: undo_payload.network_option.clone(),
                };
                let result = match settled {
                    WriteState::Success { .. } => {
                        println!("result: {}", settled.label());
                        match watch {
                            Some(secs) if secs > 0 => watch_link(
                                &mut client,
                                &creds,
                                Some((&undo_payload, &undo_intent)),
                                cfg.app.verify_secs,
                                secs,
                            ),
                            _ => Ok(()),
                        }
                    }
                    other => {
                        println!("result: {}", other.label());
                        Err(ApiError::WriteFailed(other.label()))
                    }
                };
                // One logout per path: the watch keeps the session open on
                // purpose, and closes it here like every other branch.
                client.logout();
                result
            } else {
                let node = client.get("net/net-mode-list")?;
                let catalog = bands::from_net_mode_list(&node);
                let mode = parse::parse_net_mode(&client.get("net/net-mode")?);
                client.logout();

                if json {
                    let payload = serde_json::json!({
                        "supported": catalog.defs.iter().map(|d| serde_json::json!({
                            "band": d.band, "label": d.label, "mask": format!("{:X}", d.value)
                        })).collect::<Vec<_>>(),
                        "all_bands_mask": catalog.lte_all.map(bands::to_hex),
                        "network_bands_mask": catalog.network_all.map(bands::to_hex),
                        "current": mode,
                        "current_bands": mode.lte_band.as_deref()
                            .and_then(bands::from_hex)
                            .map(bands::bands_from_mask),
                    });
                    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
                } else if list {
                    println!(
                        "device supports {} LTE bands; all-bands mask {}",
                        catalog.defs.len(),
                        catalog
                            .lte_all
                            .map(bands::to_hex)
                            .unwrap_or_else(|| "unknown".into())
                    );
                    for d in &catalog.defs {
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
        Some(Cmd::Scan {
            sets,
            mode,
            option,
            settle,
            dry_run,
            json,
            no_save,
            ignore_route,
            ..
        }) => {
            let selection: Vec<Vec<u8>> = sets
                .iter()
                .map(|s| bands::parse_list(s).map_err(ApiError::Config))
                .collect::<Result<_, _>>()?;
            let mode_code = bands::mode_code(&mode).ok_or_else(|| {
                ApiError::Config(format!("unknown mode {mode:?}; use auto, 4g or 5g"))
            })?;
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
            // The three steps of a set and their ceilings. `settle` and `grace`
            // are ours; the measurement ceiling is the speedtest module's own
            // number, so a change to one of its phases moves the estimate with it.
            let plan = cfg.app.speedtest.clone().sanitised();
            let measure_secs = routwatch::speedtest::worst_case_secs(&plan);
            let per_set = settle + LINK_GRACE.as_secs() + measure_secs;
            let secs = per_set * selection.len() as u64;
            let mb = plan.max_mb * selection.len() as u64;
            let shown: Vec<String> = selection
                .iter()
                .map(|s| s.iter().map(u8::to_string).collect::<Vec<_>>().join(","))
                .collect();
            if dry_run {
                println!("scan: {} sets: {}", selection.len(), shown.join(" | "));
                println!(
                    "time: upper bound {secs} s ({per_set} s per set: --settle {settle} s cap, {} s grace, {measure_secs} s measurement)",
                    LINK_GRACE.as_secs()
                );
                println!("data: upper bound {mb} MB ({} MB per set)", plan.max_mb);
                println!(
                    "each ceiling is an upper bound: the settle wait ends as soon as an address returns, and a measurement that fails early spends less"
                );
                println!(
                    "radius: the measurement is this host's path through the router, not the WAN link alone; a VPN or a second interface in that path makes every row say nothing about the set"
                );
                println!("dry-run: nothing was sent");
                return Ok(());
            }
            // A speed figure from a path that does not leave through the router
            // says nothing about the set under test, so the run stops unless the
            // user says they know the path.
            if !ignore_route {
                let out_dev = route_dev("1.1.1.1");
                let router_dev = route_dev(&cfg.router.ip);
                if let (Some(a), Some(b)) = (&out_dev, &router_dev) {
                    if a != b {
                        return Err(ApiError::Config(format!(
                            "scan measures this host's path: the internet leaves via {a}, while {} is on {b}. A VPN or a second interface sits in the measured path; pass --ignore-route to measure anyway",
                            cfg.router.ip
                        )));
                    }
                }
            }

            let mut client = connect(&cfg, cli.password_stdin)?;
            let catalog = client
                .get("net/net-mode-list")
                .ok()
                .map(|n| bands::from_net_mode_list(&n));
            let lte_all = catalog.as_ref().and_then(|c| c.lte_all).map(bands::to_hex);
            let network_all = catalog
                .as_ref()
                .and_then(|c| c.network_all)
                .map(bands::to_hex);
            // Read before the first write: this is the mask the run returns to.
            let start = write::read_mode(&mut client)?;
            let start_bands = start
                .lte_band
                .as_deref()
                .and_then(bands::from_hex)
                .map(bands::bands_from_mask)
                .unwrap_or_default();
            let snap = headless::collect(&mut client, &cfg.router.ip).ok();
            let radio = routwatch::history::Radio {
                net_type: snap
                    .as_ref()
                    .map(|s| s.connection.net_type.clone())
                    .filter(|s| !s.is_empty()),
                band: None,
                band_at_start: Some(
                    start_bands
                        .iter()
                        .map(|b| format!("B{b}"))
                        .collect::<Vec<_>>()
                        .join(","),
                )
                .filter(|s| !s.is_empty()),
                rsrp_dbm: snap.as_ref().and_then(|s| s.signal.rsrp_dbm),
                rsrq_db: snap.as_ref().and_then(|s| s.signal.rsrq_db),
                sinr_db: snap.as_ref().and_then(|s| s.signal.sinr_db),
            };
            let interrupted = std::sync::atomic::AtomicBool::new(false);
            // Installed before the first write, dropped back to the default once
            // the starting mask is on the radio again.
            let handler: extern "C" fn(libc::c_int) = on_sigint;
            unsafe {
                libc::signal(libc::SIGINT, handler as libc::sighandler_t);
            }
            let (rows, restore_err, applied) = {
                let mut io = LiveScan {
                    client: &mut client,
                    plan: &plan,
                    mode_code: mode_code.to_string(),
                    option: option_code.clone(),
                    lte_all: lte_all.clone(),
                    network_all: network_all.clone(),
                    settle: Duration::from_secs(settle),
                    start: start.clone(),
                    start_bands: start_bands.clone(),
                    last_bands: None,
                    wrote: false,
                    save: !no_save,
                    radio,
                    interrupted: &interrupted,
                };
                let (rows, err) = routwatch::scan::run(&selection, &mut io);
                (rows, err, io.wrote)
            };
            unsafe {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
            }
            client.logout();

            if json {
                println!("{}", serde_json::to_string_pretty(&rows).unwrap());
            } else {
                print_scan_table(&rows);
            }
            if !applied {
                println!("nothing to restore: no set was applied");
            } else if let Some(why) = &restore_err {
                let last = rows
                    .iter()
                    .rev()
                    .find(|r| !matches!(r.outcome, routwatch::scan::Outcome::NotRun));
                let on = last.map(|r| label_of(&r.bands)).unwrap_or_default();
                println!(
                    "bands NOT restored: the router is still on {on}. Run: routwatch bands --set {} --mode {mode_code} --allow-write ({why})",
                    label_of(&start_bands)
                );
                return Err(ApiError::Quiet(1));
            } else {
                println!(
                    "bands restored to the set the run started with ({})",
                    label_of(&start_bands)
                );
            }
            Ok(())
        }
        Some(Cmd::Dns {
            set,
            reset,
            dry_run,
            allow_write,
            force,
            json,
        }) => {
            if reset && set.is_some() {
                return Err(ApiError::Usage(
                    "--set and --reset contradict each other; pass one".into(),
                ));
            }
            let mut client = connect(&cfg, cli.password_stdin)?;
            let doc = client.get_dhcp();
            let editable = client.dhcp_dns_editable();
            let current = doc.as_ref().ok().map(|d| DnsInfo {
                status: d.dns_status.clone(),
                primary: d.primary.clone(),
                secondary: d.secondary.clone(),
                lan_ip: d.dhcp_ip.clone(),
                editable,
            });
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
                        d.status.clone().unwrap_or_else(|| "-".into()),
                        d.primary.clone().unwrap_or_else(|| "-".into()),
                        d.secondary.clone().unwrap_or_else(|| "-".into())
                    );
                }
                if editable == Some(false) {
                    println!("{}", DNS_NOT_EDITABLE);
                }
                println!("use --set PRIMARY,SECONDARY or --reset");
                client.logout();
                return Ok(());
            }
            let doc = match doc {
                Ok(d) => d,
                Err(e) => {
                    client.logout();
                    return Err(ApiError::Config(format!(
                        "cannot read dhcp/settings: the DNS write needs the whole document ({e})"
                    )));
                }
            };
            let plan = match write::plan_dns(&doc, reset, &primary, &secondary) {
                Ok(plan) => plan,
                Err(e) => {
                    client.logout();
                    return Err(ApiError::Usage(e));
                }
            };
            if let Some(d) = &current {
                println!(
                    "current: status={} primary={} secondary={} lan={}",
                    d.status.clone().unwrap_or_else(|| "-".into()),
                    d.primary.clone().unwrap_or_else(|| "-".into()),
                    d.secondary.clone().unwrap_or_else(|| "-".into()),
                    d.lan_ip.clone().unwrap_or_else(|| "-".into())
                );
            }
            if editable == Some(false) {
                println!("{}", DNS_NOT_EDITABLE);
            }
            println!(
                "target:  primary={} secondary={}",
                plan.primary, plan.secondary
            );
            println!("payload: {}", plan.body);
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
                other => Err(ApiError::WriteFailed(other.label())),
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
                        "inbox: blocked - {e}\n        read them at http://192.168.8.1 → SMS"
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
                    println!("restart accepted - the router will drop the link for ~60s");
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Some(Cmd::Speedtest {
            json,
            dry_run,
            allow_data,
            download_only,
            download_url,
            upload_url,
            seconds,
            max_mb,
            upload_mb,
            no_save,
            history,
        }) => {
            if history {
                let Some(p) = routwatch::history::path() else {
                    eprintln!("no data dir; nothing to list");
                    return Ok(());
                };
                let l = routwatch::history::load(&p);
                if let Some(w) = &l.warn {
                    eprintln!("{w}");
                }
                if l.rows.is_empty() {
                    println!("no saved speed runs");
                    return Ok(());
                }
                println!(
                    "{:<10} {:>12} {:>12} {:>6} {:>7} {:<9} host",
                    "age", "down", "up", "idle", "loaded", "status"
                );
                let now = routwatch::app::unix_now();
                for e in &l.rows {
                    println!(
                        "{:<10} {:>12} {:>12} {:>6} {:>7} {:<9} {}",
                        routwatch::ui::widgets::dur(now.saturating_sub(e.ended_unix)),
                        routwatch::ui::widgets::bitrate(e.download_bps),
                        routwatch::ui::widgets::bitrate(e.upload_bps),
                        e.ping_idle_ms
                            .map(|v| format!("{v:.0}"))
                            .unwrap_or_else(|| "-".into()),
                        e.ping_loaded_ms
                            .map(|v| format!("{v:.0}"))
                            .unwrap_or_else(|| "-".into()),
                        e.status.label(),
                        e.ping_host.as_deref().unwrap_or("-"),
                    );
                }
                return Ok(());
            }
            let mut plan = cfg.app.speedtest.clone();
            if let Some(v) = download_url {
                plan.download_url = v;
            }
            if let Some(v) = upload_url {
                plan.upload_url = v;
            }
            if let Some(v) = seconds {
                plan.duration_secs = v;
            }
            if let Some(v) = max_mb {
                plan.max_mb = v;
            }
            if let Some(v) = upload_mb {
                plan.upload_mb = v;
            }
            if download_only {
                plan.download_only = true;
            }
            // The same clamp the TUI applies, so a CLI typo costs a retry.
            let plan = plan.sanitised();
            if dry_run {
                if json {
                    println!("{}", serde_json::to_string_pretty(&plan).unwrap());
                } else {
                    println!(
                        "speedtest: {} s, max {} MB down, {} MB up (download only: {})\n  down {}\n  up   {}",
                        plan.duration_secs,
                        plan.max_mb,
                        plan.upload_mb,
                        plan.download_only,
                        plan.download_url,
                        plan.upload_url
                    );
                }
                return Ok(());
            }
            if !allow_data {
                // The same rule as a write: name the flag instead of spending
                // data because the command was typed without thinking.
                eprintln!(
                    "speedtest spends data: pass --allow-data to run it, or --dry-run to see the plan"
                );
                return Err(ApiError::Quiet(3));
            }
            let started = std::time::Instant::now();
            let r = routwatch::speedtest::run(
                &plan,
                &std::sync::atomic::AtomicBool::new(false),
                |_| {},
            )
            .map_err(ApiError::Network)?;
            if !no_save {
                if let Some(p) = routwatch::history::path() {
                    // The CLI does not ping, so the row carries no ping figures:
                    // a saved row describes what was actually measured.
                    let entry = routwatch::history::Entry::from_result(
                        &r,
                        &plan,
                        routwatch::history::Source::Cli,
                        None,
                        routwatch::history::Pings::default(),
                        started.elapsed().as_millis() as u64,
                        routwatch::app::unix_now(),
                    );
                    match routwatch::history::append(&p, &entry) {
                        Ok(()) => eprintln!("saved to {}", p.display()),
                        Err(e) => eprintln!("not saved: {e}"),
                    }
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&r).unwrap());
            } else {
                println!(
                    "download {:.1} Mbps · upload {:.1} Mbps · ttfb {:.0} ms",
                    r.download_bps / 1e6,
                    r.upload_bps / 1e6,
                    r.ttfb_ms
                );
                if let Some(note) = &r.note {
                    println!("note: {note}");
                }
            }
            Ok(())
        }
        Some(Cmd::Ip {
            action: IpAction::Renew { json, settle, .. },
        }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            let limits = routwatch::write::VerifyLimits::from_secs(settle);
            let out = routwatch::write::renew_ip(&mut client, limits, std::thread::sleep);
            client.logout();
            if json {
                let v = match &out {
                    routwatch::write::RenewOutcome::Changed {
                        before,
                        after,
                        secs,
                    } => serde_json::json!({
                        "outcome": "changed", "before": before, "after": after, "secs": secs
                    }),
                    routwatch::write::RenewOutcome::Unchanged { ip, secs } => serde_json::json!({
                        "outcome": "unchanged", "ip": ip, "secs": secs
                    }),
                    routwatch::write::RenewOutcome::LinkDown { why } => serde_json::json!({
                        "outcome": "link down", "why": why
                    }),
                    routwatch::write::RenewOutcome::NoAddress { attempts, last_err } => {
                        serde_json::json!({
                            "outcome": "no address", "attempts": attempts, "last_err": last_err
                        })
                    }
                };
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                match &out {
                    routwatch::write::RenewOutcome::Changed {
                        before,
                        after,
                        secs,
                    } => {
                        println!("before: {before}");
                        println!("after:  {after} (changed after {secs} s)");
                    }
                    routwatch::write::RenewOutcome::Unchanged { ip, secs } => {
                        println!("before: {ip}");
                        println!("after:  {ip} (unchanged after {secs} s: the ISP kept the lease)");
                    }
                    routwatch::write::RenewOutcome::LinkDown { why } => {
                        println!("the data connection is still off ({why})");
                        println!(
                            "the router still answers on its LAN address. Run: routwatch ip renew --allow-write,"
                        );
                        println!("or switch the mobile data connection on from the router page.");
                    }
                    routwatch::write::RenewOutcome::NoAddress { attempts, last_err } => {
                        let why = last_err
                            .clone()
                            .unwrap_or_else(|| "the device answered without an address".into());
                        println!(
                            "the data connection is on but no address came back in {attempts} reads: {why}"
                        );
                    }
                }
            }
            match out.exit_code() {
                0 => Ok(()),
                code => Err(ApiError::Quiet(code)),
            }
        }
        Some(Cmd::History {
            signal,
            json,
            limit,
        }) => {
            if !signal {
                eprintln!(
                    "history: only the signal log is saved so far: pass --signal (speed runs are `speedtest --history`)"
                );
                return Err(ApiError::Quiet(2));
            }
            let Some(p) = routwatch::signal_history::path() else {
                eprintln!("history: this platform has no data directory, so no log is kept");
                return Err(ApiError::Quiet(2));
            };
            let l = routwatch::signal_history::load(&p);
            if l.damaged > 0 {
                eprintln!(
                    "history: {} of {} rows could not be read and are skipped",
                    l.damaged, l.total
                );
            }
            let rows: Vec<routwatch::signal_history::Row> =
                l.rows.iter().rev().take(limit).rev().cloned().collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&rows).unwrap());
            } else if rows.is_empty() {
                println!("no signal rows yet: they are written by every command that collects");
            } else {
                print!("{}", routwatch::signal_history::table(&rows));
            }
            Ok(())
        }
        Some(Cmd::Ping { host, count, json }) => {
            let output = std::process::Command::new("ping")
                .args(worker::ping_args(count, &host))
                .output()
                .map_err(|e| {
                    ApiError::Config(format!(
                        "cannot run ping: {e} - install iputils-ping (Debian/Ubuntu) or iputils (Arch/Fedora)"
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
        Some(Cmd::Share { out }) => {
            let stdin_password = read_stdin_password(cli.password_stdin)?;
            let password = read_password(&cfg, stdin_password.as_deref())?;
            let mut client = HuaweiClient::new(&cfg.router.ip, cfg.app.timeout_ms);
            client.login(&cfg.router.username, &password)?;

            // What the firmware answers, decided by the same code `doctor` uses.
            let report = headless::doctor(
                &cfg,
                &cfg_path.display().to_string(),
                true,
                stdin_password.as_deref(),
            );
            // Field names only: the values of identity tags are dropped here.
            let mut lines = Vec::new();
            for spec in routwatch::router::endpoints::PROBE {
                let (state, fields) = match client.raw(spec.path) {
                    Ok(body) => ("ok".to_string(), routwatch::scrub::field_names(&body)),
                    Err(e) => (e.class().to_string(), Vec::new()),
                };
                lines.push(routwatch::share::EndpointLine {
                    path: spec.path.to_string(),
                    state,
                    fields,
                });
            }
            let snap = headless::collect(&mut client, &cfg.router.ip)?;
            client.logout();

            let when = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let text = routwatch::share::render(
                env!("CARGO_PKG_VERSION"),
                &cfg.router.ip,
                when,
                &report,
                &snap,
                &lines,
            );
            let path = out
                .unwrap_or_else(|| std::path::PathBuf::from(format!("routwatch-share-{when}.md")));
            routwatch::share::write(&text, &path)
                .map_err(|e| ApiError::Config(format!("{}: {e}", path.display())))?;
            println!("share written: {} ({} bytes)", path.display(), text.len());
            println!("paste it in a model report; it carries no serial, IMEI, MAC or SSID");
            Ok(())
        }
        Some(Cmd::Dump { out, json, force }) => {
            let mut client = connect(&cfg, cli.password_stdin)?;
            if json {
                let path = out.unwrap_or_else(|| std::path::PathBuf::from("routwatch-dump.json"));
                let bytes =
                    headless::dump_to(&mut client, &cfg.router.ip, &path, cli.redact, force)?;
                client.logout();
                println!("dump written: {} ({bytes} bytes)", path.display());
                return Ok(());
            }
            let mut text = String::new();
            for spec in routwatch::router::endpoints::PROBE {
                text.push_str(&format!("\n##### {} #####\n", spec.path));
                match client.raw(spec.path) {
                    Ok(body) => {
                        let body = if cli.redact {
                            routwatch::scrub::scrub_xml(body.trim())
                        } else {
                            body.trim().to_string()
                        };
                        text.push_str(&body);
                    }
                    Err(e) => text.push_str(&format!("<error: {e}>")),
                }
                text.push('\n');
            }
            client.logout();
            match out {
                Some(path) => {
                    routwatch::output::write_private(&path, text.as_bytes(), force)
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
            ref history_fixture,
            demo_confirm,
            demo_settings,
            ansi,
        }) => {
            let (w, h) = parse_size(size)?;
            let mut app = build_app(&cli, &cfg)?;
            // Read the ledger so the usage table shows the same numbers the TUI
            // would, but never take the lock and never write: a preview is a
            // read-only frame.
            app.usage = routwatch::usage::load(&routwatch::usage::path());
            let mut snap: Snapshot = match fixture {
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
            if cli.redact {
                snap = routwatch::report::redacted(&snap);
            }
            app.on_snapshot(snap, 0);
            if let Some(p) = history_fixture {
                let text = std::fs::read_to_string(p)
                    .map_err(|e| ApiError::Config(format!("{}: {e}", p.display())))?;
                let loaded = routwatch::history::load_from_str(&text);
                app.history = loaded
                    .rows
                    .into_iter()
                    .take(routwatch::history::SHOWN)
                    .collect();
                app.history_total = loaded.total;
            }
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

fn build_app(cli: &Cli, cfg: &config::Config) -> Result<App, ApiError> {
    let theme = cli.theme.clone().unwrap_or_else(|| cfg.app.theme.clone());
    let view = match cli.view.as_deref() {
        Some(v) => View::parse(v).ok_or_else(|| {
            ApiError::Usage(format!("unknown view {v:?}; use full, lite or dense"))
        })?,
        None => View::parse(&cfg.app.view).unwrap_or(View::Full),
    };
    let graph = match cli.graph.as_deref() {
        Some(g) => GraphStyle::try_parse(g).ok_or_else(|| {
            ApiError::Usage(format!(
                "unknown graph {g:?}; use dots, bars, braille or ascii"
            ))
        })?,
        None => GraphStyle::parse(&cfg.app.graph_style),
    };
    let mut app = App::new(
        Theme::get(&theme),
        graph,
        view,
        cfg.app.budget_gb,
        cfg.app.refresh_ms,
    );
    if let Some(name) = cli.tab.as_deref() {
        let tab = Tab::parse(name).ok_or_else(|| {
            ApiError::Usage(format!("unknown tab {name:?}; run --help for the list"))
        })?;
        app.tab = tab;
    }
    if let Some(presets) = &cfg.app.presets {
        app.band_presets = presets.clone();
    }
    app.report_identity = cli.report_identity || cfg.app.report_identity;
    // One clamp, read by both the confirmation modal and the run itself.
    app.set_speed_cfg(cfg.app.speedtest.clone());
    app.config_path = cli.config.clone();
    Ok(app)
}

fn parse_size(s: &str) -> Result<(u16, u16), ApiError> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| ApiError::Usage(format!("bad size {s:?}; expected WIDTHxHEIGHT")))?;
    let w: u16 = w
        .trim()
        .parse()
        .map_err(|_| ApiError::Usage(format!("bad width {:?} in {s:?}", w.trim())))?;
    let h: u16 = h
        .trim()
        .parse()
        .map_err(|_| ApiError::Usage(format!("bad height {:?} in {s:?}", h.trim())))?;
    if w < 24 || h < 6 {
        return Err(ApiError::Usage(format!(
            "{w}x{h} is below the 24x6 minimum the frame can draw"
        )));
    }
    Ok((w, h))
}

/// `--password-stdin` is read here, once, so every caller (commands, doctor and
/// the TUI) can pass the same value down instead of only some of them reading it.
fn read_stdin_password(enabled: bool) -> Result<Option<String>, ApiError> {
    if !enabled {
        return Ok(None);
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| ApiError::Config(format!("stdin: {e}")))?;
    Ok(Some(buf.trim_end_matches(['\n', '\r']).to_string()))
}

fn read_password(cfg: &config::Config, stdin_password: Option<&str>) -> Result<String, ApiError> {
    config::resolve_password(&cfg.router.password, stdin_password).map(|(password, _)| password)
}

fn connect(cfg: &config::Config, password_stdin: bool) -> Result<HuaweiClient, ApiError> {
    Ok(connect_with(cfg, password_stdin)?.0)
}

/// The same login, keeping the credentials: a command that holds the session
/// open across a radio restart must be able to sign in again on its own.
fn connect_with(
    cfg: &config::Config,
    password_stdin: bool,
) -> Result<(HuaweiClient, (String, String)), ApiError> {
    let stdin_password = read_stdin_password(password_stdin)?;
    let password = read_password(cfg, stdin_password.as_deref())?;
    let mut client = HuaweiClient::new(&cfg.router.ip, cfg.app.timeout_ms);
    client.login(&cfg.router.username, &password)?;
    Ok((client, (cfg.router.username.clone(), password)))
}

/// One line from stdin, `y` in either case. Anything else, or no stdin at all,
/// means no: the revert is offered, never taken.
fn confirmed_on_stdin() -> bool {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

/// The watch, wired to this process: a real clock, real stdin, stdout lines.
fn watch_link(
    client: &mut HuaweiClient,
    creds: &(String, String),
    undo: Option<(&bands::BandPayload, &WriteIntent)>,
    verify_secs: u64,
    secs: u64,
) -> Result<(), ApiError> {
    let mut sleep = |d: Duration| std::thread::sleep(d);
    let mut confirm = confirmed_on_stdin;
    let mut log = |line: String| println!("watch: {line}");
    let mut io = routwatch::watch::WatchIo {
        sleep: &mut sleep,
        confirm: &mut confirm,
        log: &mut log,
    };
    routwatch::watch::watch_link(
        client,
        (creds.0.as_str(), creds.1.as_str()),
        undo,
        verify_secs,
        secs,
        &mut io,
    )?;
    Ok(())
}

// --------------------------------------------------------------------- TUI

/// Runs the restore closure exactly once, on drop. Every early return between
/// raw mode and the event loop must restore echo, canonical input and signals,
/// `?` included.
struct RestoreOnDrop<F: FnMut()>(Option<F>);

impl<F: FnMut()> Drop for RestoreOnDrop<F> {
    fn drop(&mut self) {
        if let Some(mut f) = self.0.take() {
            f();
        }
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
}

/// A panic inside the TUI would otherwise unwind past the guard's usefulness
/// on some paths and print the message into the alternate screen; restoring in
/// the hook means the backtrace lands on a normal terminal.
fn install_restore_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            default(info);
        }));
    });
}

fn run_tui(mut app: App, cfg: config::Config, password: String) -> Result<(), ApiError> {
    // The ledger lives beside the recordings, under the user's data dir. One
    // writer at a time: a second window runs read-only rather than racing.
    let usage_path = routwatch::usage::path();
    app.usage = routwatch::usage::load(&usage_path);
    match routwatch::usage::claim(&usage_path) {
        Ok(Some(guard)) => {
            app.usage_lock = Some(guard);
            app.usage_path = Some(usage_path);
        }
        Ok(None) => app.note("another routwatch is using the ledger: read-only this session"),
        Err(e) => app.note(format!("usage ledger not locked: {e}")),
    }
    // Read once at start; the panel stays current by appending what it records.
    app.load_history();
    let (tx, rx) = worker::spawn(cfg, password);

    install_restore_hook();
    enable_raw_mode().map_err(|e| ApiError::Config(format!("raw mode: {e}")))?;
    let _restore = RestoreOnDrop(Some(restore_terminal));
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .map_err(|e| ApiError::Config(format!("alt screen: {e}")))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| ApiError::Config(format!("terminal: {e}")))?;
    terminal.clear().ok();

    while !app.quit {
        drain(&mut app, &rx);
        // Once every 30 s, not on every poll: the ledger is a file, and the
        // counters move by kilobytes a second.
        if app.usage_dirty
            && app
                .usage_saved
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
        {
            if let Some(path) = app.usage_path.as_deref() {
                match routwatch::usage::save(path, &app.usage) {
                    Ok(()) => {
                        app.usage_dirty = false;
                        app.usage_saved = Some(std::time::Instant::now());
                    }
                    Err(e) => app.note(format!("usage not saved: {e}")),
                }
            }
        }
        terminal.draw(|f| ui::draw(f, &app)).ok();

        if event::poll(Duration::from_millis(200)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => handle_key(&mut app, k, &tx),
                // A resize must repaint from scratch: the terminal has already
                // discarded the old cells, and anything the diff thinks it
                // already drew is now a hole. Clearing costs one full frame and
                // is what keeps a plot from looking cut after a zoom.
                Ok(Event::Resize(_, _)) => {
                    terminal.clear().ok();
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    // Last write before the terminal goes back: whatever the 30 s window would
    // have kept is worth one more small file, and this is the only moment the
    // lock is known to still be ours.
    if app.usage_dirty {
        if let Some(path) = app.usage_path.as_deref() {
            let _ = routwatch::usage::save(path, &app.usage);
        }
    }

    // Wait for the worker to log out; the process must not exit before the
    // router sees the logout POST.
    let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(0);
    let _ = tx.send(WorkerCmd::Stop(ack_tx));
    let _ = ack_rx.recv_timeout(Duration::from_secs(3));
    drop(terminal);
    drop(_restore);
    println!(
        "routwatch: session closed (last sample {}s old)",
        app.age().map(|a| a.as_secs()).unwrap_or(0)
    );
    Ok(())
}

/// Replay recorded frames into the same UI, with no router contact at all.
fn run_replay(
    app: &mut App,
    frames: Vec<record::RecordFrame>,
    speed: u64,
    redact: bool,
) -> Result<(), ApiError> {
    install_restore_hook();
    enable_raw_mode().map_err(|e| ApiError::Config(format!("raw mode: {e}")))?;
    let _restore = RestoreOnDrop(Some(restore_terminal));
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
        let snapshot = if redact {
            routwatch::report::redacted(&frame.snapshot)
        } else {
            frame.snapshot.clone()
        };
        app.on_snapshot(snapshot, 0);
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

    drop(terminal);
    drop(_restore);
    println!(
        "routwatch: replay finished - {} frames",
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
            Ev::SpeedProgress(id, p) => app.on_speed_progress(id, p),
            Ev::SpeedDone(id, res) => app.on_speed_done(id, res.map(|b| *b)),
            Ev::Extras(extras) => app.on_extras(*extras),
            Ev::Phase { phase, state } => app.on_phase(phase, state),
            Ev::Notice(msg) => {
                app.note(msg.clone());
                app.log(routwatch::app::LogLevel::Info, msg);
            }
        }
    }
    // Every frame checks the budget: a thread that died without answering must
    // not leave the panel spinning.
    app.speed_watchdog();
}

fn handle_key(app: &mut App, k: KeyEvent, tx: &Sender<WorkerCmd>) {
    // The settings overlay is modal for appearance keys.
    if app.settings_open {
        match k.code {
            KeyCode::Esc | KeyCode::Char('S') => app.settings_open = false,
            KeyCode::Char('t') => {
                app.theme = routwatch::theme::cycle(&app.theme.id);
                app.note(format!("theme {} · w saves", app.theme.id));
            }
            KeyCode::Char('v') => app.view = app.view.next(),
            KeyCode::Char('g') => app.graph = app.graph.next(),
            KeyCode::Char('[') => {
                app.adjust_refresh(-500);
                let _ = tx.send(WorkerCmd::SetInterval(app.refresh_ms));
            }
            KeyCode::Char(']') => {
                app.adjust_refresh(500);
                let _ = tx.send(WorkerCmd::SetInterval(app.refresh_ms));
            }
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
                                app.note("restart sent - waiting for the router");
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
    // The speed test spends data, so it asks once. Placed before the budget
    // reset so the two modals never read the same key.
    if app.speed_confirm {
        match k.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                let (id, cfg, cancel) = app.start_speed_test();
                // The clamped copy the modal printed, and the flag the thread
                // polls: no third source of truth.
                let host = cfg.ping_host.clone();
                let _ = tx.send(WorkerCmd::SpeedTest(cfg, host, id, cancel));
                app.note("speed test started · its bytes count in the router's month total");
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => app.speed_confirm = false,
            _ => {}
        }
        return;
    }

    let confirming = matches!(app.write, WriteState::Confirming(_));
    // A write in flight owns the exit keys too: quitting mid-write leaves the
    // router with a change nobody read back.
    if app.write.is_busy() {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            app.note("a write is in progress; wait for it to settle");
        }
        return;
    }
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        app.quit = true;
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
    // The budget reset is local, so it gets its own small confirm instead of
    // borrowing the router-write modal.
    if app.reset_confirm {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let sample = app.snap.as_ref().map(|s| {
                    (
                        routwatch::usage::source_total(&s.traffic, s.quota.as_ref()),
                        s.quota.is_some(),
                    )
                });
                match sample {
                    Some((total, has_quota)) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        routwatch::usage::reset(&mut app.usage, now, total, has_quota);
                        app.usage_dirty = true;
                        // `note` replaces the previous message, so it is written
                        // once, after the save attempt: a failure must not be
                        // overwritten by a success line.
                        match app.usage_path.clone() {
                            Some(path) => match routwatch::usage::save(&path, &app.usage) {
                                Ok(()) => {
                                    app.usage_dirty = false;
                                    app.note("budget baseline reset");
                                }
                                Err(e) => app.note(format!("usage not saved: {e}")),
                            },
                            None => app.note("baseline reset · read-only session: not saved"),
                        }
                        app.reset_confirm = false;
                    }
                    None => {
                        app.reset_confirm = false;
                        app.note("no sample yet: nothing to reset against");
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.reset_confirm = false;
            }
            _ => {}
        }
        return;
    }
    // `Esc` stops a running test. After every overlay block, so an open modal
    // keeps its own Esc, and guarded on input so a typed name is not cancelled
    // by the same key.
    if app.speed_running && k.code == KeyCode::Esc && app.input.is_none() {
        app.cancel_speed_test();
        return;
    }
    // `u` belongs to the verdict it undoes, so it is read before the banner that
    // swallows every other key while a write is settled.
    if app.tab == Tab::Bands && matches!(normalise_key(k.code), KeyCode::Char('u')) {
        // The revert is a write like any other: the four values the device
        // carried before the apply go back out on the worker's path, forced,
        // because the apply it undoes has already landed and the device
        // therefore matches the payload it is being sent.
        // A failed write says nothing about what the device carries now, so it
        // offers no revert: the point would be written blind.
        let blocked = app.write.is_busy() || matches!(app.write, WriteState::Failed { .. });
        let write_back = if blocked {
            None
        } else {
            app.undo_payload().zip(app.undo_intent())
        };
        match (write_back, app.band_undo()) {
            (Some((payload, intent)), Some(label)) => {
                let _ = tx.send(WorkerCmd::ApplyBands {
                    payload,
                    intent: intent.clone(),
                    force: true,
                });
                app.note(format!("{label} · writing"));
                app.settle_write(WriteState::Pending {
                    intent,
                    since: Instant::now(),
                });
            }
            // The point exists but the write cannot go out: say which of the two
            // stopped it rather than pretend the revert happened.
            (_, Some(label)) => {
                app.note(label);
                if app.write.is_settled() {
                    app.settle_write(WriteState::Idle);
                }
            }
            // Two different facts: an old point with no usable band, versus no
            // point at all.
            (_, None) if app.undo_mask.is_some() => {
                app.note("undo point has no band this device lists")
            }
            (_, None) => app.note("nothing to undo: no apply yet this session"),
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

    // While the feed is open it owns j/k/space and its own Esc. `i` toggles it
    // in both directions, and everything else (q, digits, Tab) still works.
    if app.events_open {
        match code {
            KeyCode::Char('i') | KeyCode::Esc => {
                app.events_open = false;
                return;
            }
            KeyCode::Char(' ') => {
                app.events_paused = !app.events_paused;
                if !app.events_paused {
                    app.events_back = 0;
                }
                return;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.event_move(1);
                return;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.event_move(-1);
                return;
            }
            _ => {}
        }
    }

    match code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('i') => {
            app.events_open = true;
            app.events_paused = false;
            app.events_back = 0;
        }
        // `T` is a deliberate two-key gesture: the test spends the user's data.
        KeyCode::Char('t') if shift && app.tab == Tab::Network => {
            app.begin_speed_test();
        }
        // The theme is an appearance key: it answers from any section, and the
        // settings overlay keeps its own `t` so the row there still works.
        KeyCode::Char('t') => {
            app.theme = routwatch::theme::cycle(&app.theme.id);
            app.note(format!(
                "theme {}{}",
                app.theme.id,
                if app.settings_open { " · w saves" } else { "" }
            ));
        }
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
        // Two keys, one job each: `p` pings, `P` pauses, from any section.
        // Shift represents `P` here because letters are matched case-free.
        KeyCode::Char('p') if shift => {
            app.paused = !app.paused;
            let _ = tx.send(WorkerCmd::Pause(app.paused));
        }
        KeyCode::Char('p') => {
            app.open_input(InputKind::PingHost);
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
        // Local baseline reset, not a router write: it asks once and never
        // touches the month counters.
        KeyCode::Char('b') if app.tab == Tab::Traffic => app.reset_confirm = true,
        KeyCode::Enter | KeyCode::Char('a') if app.tab == Tab::Dns => {
            // The Custom row opens the prompt instead of confirming a fixed pair.
            if app.dns_preset_is_custom() {
                app.open_input(InputKind::Dns);
            } else {
                app.begin_dns_preset();
            }
        }
        // --- Clients tab: sort / copy ---
        KeyCode::Char('s') if app.tab == Tab::Clients && !shift => {
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
        // Refresh period: `[` and `]` work from any view, not only inside the
        // settings overlay (where they also appear).
        KeyCode::Char('[') => {
            app.adjust_refresh(-500);
            let _ = tx.send(WorkerCmd::SetInterval(app.refresh_ms));
        }
        KeyCode::Char(']') => {
            app.adjust_refresh(500);
            let _ = tx.send(WorkerCmd::SetInterval(app.refresh_ms));
        }
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
        app.note("no sample yet - nothing to report");
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
/// none - so the first one present wins instead of assuming a compositor.
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
                snap.network_bands_mask
                    .as_deref()
                    .or(snap.all_bands_mask.as_deref()),
                Some(app.band_option.as_str()),
            );
            let _ = tx.send(WorkerCmd::ApplyBands {
                payload,
                intent: intent.clone(),
                force: false,
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
        sig.band.as_deref().unwrap_or("-"),
        match (sig.dl_mhz, sig.ul_mhz) {
            (Some(d), Some(u)) => format!("{d}/{u} MHz"),
            _ => "-".to_string(),
        },
        sig.pci.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
        sig.earfcn.as_deref().unwrap_or("-"),
        sig.tm.as_deref().unwrap_or("-")
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
            c.ip.as_deref().unwrap_or("-"),
            c.mac.as_deref().unwrap_or("-"),
            c.freq.as_deref().unwrap_or("")
        );
    }
    if let Some(nr) = &sig.nr {
        println!(
            "5g       RSRP {} · SINR {} · {}",
            opt(nr.rsrp_dbm, "dBm"),
            opt(nr.sinr_db, "dB"),
            nr.band.as_deref().unwrap_or("-")
        );
    } else {
        println!("5g       not exposed by this firmware");
    }
}

fn opt<T: std::fmt::Display>(v: Option<T>, unit: &str) -> String {
    v.map(|x| format!("{x} {unit}"))
        .unwrap_or_else(|| "-".into())
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

/// How long the link is given to hold an address before a measurement starts:
/// the address comes back before the route and DNS behind it settle.
const LINK_GRACE: Duration = Duration::from_secs(5);

/// Set by the SIGINT handler and read by the loop. A signal handler may only
/// touch an atomic: everything else (the restore, the exit code) happens on the
/// normal path the flag sends the loop down.
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_sigint(_sig: libc::c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// "1,3,5": the spelling the table and the restore hint use.
fn label_of(bands: &[u8]) -> String {
    bands
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The interface this host would send a packet to `target` through, read from
/// the kernel's own routing answer.
fn route_dev(target: &str) -> Option<String> {
    let out = std::process::Command::new("ip")
        .args(["route", "get", target])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    text.split_whitespace()
        .skip_while(|w| *w != "dev")
        .nth(1)
        .map(str::to_string)
}

/// The table T3.4 describes: measured rows first, then the ones that carry no
/// number, in the order they were tried.
fn print_scan_table(rows: &[routwatch::scan::SetResult]) {
    println!("radius: this measures this host's path through the router, not the WAN link alone");
    println!(
        "{:<12} {:<27} {:>11} {:>11} {:>8} note",
        "set", "result", "down", "up", "ttfb"
    );
    for i in routwatch::scan::rank(rows) {
        let r = &rows[i];
        let (down, up, ttfb) = match &r.outcome {
            routwatch::scan::Outcome::Measured {
                down_bps,
                up_bps,
                ttfb_ms,
            } => (
                ui::widgets::bitrate(*down_bps),
                up_bps
                    .map(ui::widgets::bitrate)
                    .unwrap_or_else(|| "-".into()),
                format!("{ttfb_ms:.0} ms"),
            ),
            _ => ("-".into(), "-".into(), "-".into()),
        };
        let note = r
            .note
            .clone()
            .or_else(|| r.outcome.why().map(str::to_string))
            .unwrap_or_default();
        println!(
            "{:<12} {:<27} {:>11} {:>11} {:>8} {}",
            label_of(&r.bands),
            r.outcome.label(),
            down,
            up,
            ttfb,
            note
        );
    }
}

/// The real `ScanIo`: one router client, one speed-test plan, one SIGINT flag.
struct LiveScan<'a> {
    client: &'a mut HuaweiClient,
    plan: &'a routwatch::config::Speedtest,
    mode_code: String,
    option: Option<String>,
    lte_all: Option<String>,
    network_all: Option<String>,
    settle: Duration,
    start: routwatch::model::NetMode,
    start_bands: Vec<u8>,
    last_bands: Option<Vec<u8>>,
    wrote: bool,
    save: bool,
    radio: routwatch::history::Radio,
    interrupted: &'a std::sync::atomic::AtomicBool,
}

impl LiveScan<'_> {
    /// The payload for a set, built against the device's own masks so the
    /// router is never handed a constant this firmware did not report.
    fn payload(&self, bands: &[u8]) -> bands::BandPayload {
        bands::resolve(
            &self.mode_code,
            bands,
            self.lte_all.as_deref(),
            self.network_all.as_deref(),
            self.option.as_deref(),
        )
    }

    fn write(
        &mut self,
        payload: &bands::BandPayload,
        bands: &[u8],
        force: bool,
    ) -> Result<(), String> {
        let intent = WriteIntent::ApplyBands {
            mode: payload.network_mode.clone(),
            bands: bands.to_vec(),
            mask: payload.lte_band.clone(),
            option: payload.network_option.clone(),
        };
        match write::apply_bands(self.client, payload, intent, force, &mut |_| {}) {
            WriteState::Success { .. } => Ok(()),
            other => Err(other.label()),
        }
    }
}

impl routwatch::scan::ScanIo for LiveScan<'_> {
    fn apply(&mut self, bands: &[u8]) -> Result<(), String> {
        let payload = self.payload(bands);
        let out = self.write(&payload, bands, false);
        if out.is_ok() {
            self.wrote = true;
            self.last_bands = Some(bands.to_vec());
        }
        out
    }

    fn settle(&mut self) -> routwatch::scan::Settle {
        let limits = write::VerifyLimits {
            reread: write::VERIFY_REREAD,
            window: self.settle,
        };
        let (hit, _, _) = write::poll_with(
            || parse::wan_addr(self.client),
            |ip: &Option<String>| ip.is_some(),
            limits,
            std::thread::sleep,
        );
        if self.interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            return routwatch::scan::Settle::Interrupted;
        }
        match hit {
            Some(_) => {
                std::thread::sleep(LINK_GRACE);
                routwatch::scan::Settle::Ready
            }
            None => routwatch::scan::Settle::NoService,
        }
    }

    fn measure(&mut self) -> Result<routwatch::scan::Outcome, String> {
        let started = std::time::Instant::now();
        let r = routwatch::speedtest::run(self.plan, self.interrupted, |_| {})?;
        if self.save {
            if let Some(path) = routwatch::history::path() {
                let mut radio = self.radio.clone();
                radio.band = self.last_bands.as_deref().map(label_of);
                let entry = routwatch::history::Entry::from_result(
                    &r,
                    self.plan,
                    routwatch::history::Source::Cli,
                    Some(&radio),
                    routwatch::history::Pings::default(),
                    started.elapsed().as_millis() as u64,
                    routwatch::app::unix_now(),
                );
                if let Err(e) = routwatch::history::append(&path, &entry) {
                    eprintln!("speed run not saved: {e}");
                }
            }
        }
        Ok(routwatch::scan::Outcome::Measured {
            down_bps: r.download_bps,
            up_bps: (r.upload_bps > 0.0).then_some(r.upload_bps),
            ttfb_ms: r.ttfb_ms,
        })
    }

    fn applied(&self) -> bool {
        self.wrote
    }

    fn restore(&mut self) -> Result<(), String> {
        // `force` because the radio may already be back on the starting mask by
        // itself after the last set failed: the user asked for that mask, so the
        // write proves it rather than trusting a read.
        let payload = bands::BandPayload {
            network_mode: self
                .start
                .network_mode
                .clone()
                .unwrap_or_else(|| "00".to_string()),
            network_band: self
                .start
                .network_band
                .clone()
                .unwrap_or_else(|| self.start.lte_band.clone().unwrap_or_default()),
            lte_band: self
                .start
                .lte_band
                .clone()
                .unwrap_or_else(|| "0".to_string()),
            network_option: self.start.network_option.clone(),
        };
        let bands_now = self.start_bands.clone();
        self.write(&payload, &bands_now, true)
    }

    fn interrupted(&self) -> bool {
        self.interrupted.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod key_tests {
    use super::handle_key;
    use super::normalise_key;
    use super::RestoreOnDrop;
    use super::WorkerCmd;
    use super::TAB_HELP;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
    use routwatch::app::{App, Tab, View};
    use routwatch::ui::tab_from_key;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::channel;

    #[test]
    fn the_feed_key_owns_j_k_space_and_esc_only_while_it_is_open() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            routwatch::theme::by_name("dark"),
            routwatch::theme::GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        for i in 0..3 {
            app.log(routwatch::app::LogLevel::Info, format!("line {i}"));
        }

        // `i` opens the feed and starts following.
        handle_key(&mut app, key(KeyCode::Char('i')), &tx);
        assert!(app.events_open, "i must open the feed");
        assert!(!app.events_paused);

        // `j` moves inside the feed instead of moving a client selection.
        handle_key(&mut app, key(KeyCode::Char('j')), &tx);
        assert_eq!(app.event_cursor(), Some(1));
        assert!(app.events_paused, "moving pauses");
        let clients_before = app.client_sel;
        handle_key(&mut app, key(KeyCode::Char('j')), &tx);
        assert_ne!(app.event_cursor(), Some(1), "j did not move the feed");
        assert_eq!(app.client_sel, clients_before, "j leaked to the clients");

        // space pauses and resumes; resuming returns to the newest line.
        handle_key(&mut app, key(KeyCode::Char(' ')), &tx);
        assert!(!app.events_paused);
        assert_eq!(app.event_cursor(), Some(0));

        // digits still reach a section while the feed is open.
        handle_key(&mut app, key(KeyCode::Char('3')), &tx);
        assert_eq!(app.tab, Tab::Traffic);

        // Esc closes the feed, not the help panel.
        handle_key(&mut app, key(KeyCode::Esc), &tx);
        assert!(!app.events_open);
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn shift_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c.to_ascii_uppercase()), KeyModifiers::SHIFT)
    }

    fn bare_app() -> App {
        App::new(
            routwatch::theme::by_name("dark"),
            routwatch::theme::GraphStyle::Dots,
            View::Full,
            100,
            2000,
        )
    }

    #[test]
    fn t_cycles_the_theme_from_any_section_without_opening_a_prompt() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = bare_app();
        app.select_tab(Tab::Network);
        let before = app.theme.id.clone();
        handle_key(&mut app, key(KeyCode::Char('t')), &tx);
        assert_ne!(app.theme.id, before, "t must cycle the theme");
        assert!(
            app.input.is_none(),
            "t must not open the ping prompt any more"
        );
        assert!(
            app.live_notice().unwrap_or("").contains("theme "),
            "the change must be announced, got {:?}",
            app.live_notice()
        );

        // The handler is case-free, so Shift+T does the same from another tab.
        app.select_tab(Tab::Bands);
        let before = app.theme.id.clone();
        handle_key(&mut app, key(KeyCode::Char('T')), &tx);
        assert_ne!(app.theme.id, before, "T must cycle too");
        assert_eq!(
            app.theme.id, "auto",
            "the requested name is what the cycle reports"
        );
    }

    #[test]
    fn a_typing_prompt_keeps_t_for_itself() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = bare_app();
        app.select_tab(Tab::Network);
        handle_key(&mut app, key(KeyCode::Char('p')), &tx);
        assert!(
            app.input.is_some(),
            "p must open the ping prompt on Network"
        );

        let before = app.theme.id.clone();
        handle_key(&mut app, key(KeyCode::Char('t')), &tx);
        assert_eq!(app.theme.id, before, "typing must not cycle the theme");
        let typed = app
            .input
            .as_ref()
            .map(|i| i.value.clone())
            .unwrap_or_default();
        assert!(
            typed.ends_with('t'),
            "the letter belongs to the prompt, got {typed:?}"
        );
    }

    #[test]
    fn p_pings_and_shift_p_pauses_from_any_section() {
        let (tx, _rx) = std::sync::mpsc::channel();

        // `p` pings from Network and from anywhere else; one job only.
        for tab in [Tab::Network, Tab::Dashboard, Tab::Bands] {
            let mut app = bare_app();
            app.select_tab(tab);
            handle_key(&mut app, key(KeyCode::Char('p')), &tx);
            assert!(
                app.input.is_some(),
                "p must open the ping prompt on {tab:?}"
            );
            assert!(!app.paused, "p must not pause on {tab:?}");
        }

        // `P` pauses everywhere, including Network.
        for tab in [Tab::Network, Tab::Dashboard] {
            let mut app = bare_app();
            app.select_tab(tab);
            handle_key(&mut app, shift_key('p'), &tx);
            assert!(app.paused, "P must pause on {tab:?}");
            assert!(app.input.is_none(), "P must not open a prompt on {tab:?}");
        }
    }

    #[test]
    fn the_undo_key_answers_over_a_settled_verdict() {
        use routwatch::write::WriteState;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            routwatch::theme::by_name("dark"),
            routwatch::theme::GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.select_tab(Tab::Bands);
        app.bands_supported = vec![1, 3, 5];
        app.bands_selected = vec![1];
        app.undo_mask = Some(vec![1, 3]);
        app.settle_write(WriteState::Success {
            intent: app.band_intent(),
            note: "verified".into(),
        });
        assert!(matches!(app.write, WriteState::Success { .. }));

        handle_key(&mut app, key(KeyCode::Char('u')), &tx);
        assert_eq!(
            app.bands_selected,
            vec![1, 3],
            "u must reach undo even while the verdict banner is up"
        );
        assert!(
            matches!(app.write, WriteState::Idle),
            "the banner yields after the undo key"
        );
    }

    #[test]
    fn the_undo_key_sends_the_saved_four_values_back_forced() {
        use routwatch::app::UndoPoint;
        use routwatch::write::WriteState;
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            routwatch::theme::by_name("dark"),
            routwatch::theme::GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.select_tab(Tab::Bands);
        app.bands_supported = vec![1, 3, 5];
        app.bands_selected = vec![1];
        app.band_mode = "03".into();
        // The device carried these four values before the apply that landed.
        app.undo_full = Some(UndoPoint {
            net_mode: "00".into(),
            bands: vec![1, 3],
            mask: "80000009".into(),
            option: None,
        });
        app.undo_mask = Some(vec![1, 3]);
        app.settle_write(WriteState::Success {
            intent: app.band_intent(),
            note: "verified".into(),
        });
        assert!(matches!(app.write, WriteState::Success { .. }));

        handle_key(&mut app, key(KeyCode::Char('u')), &tx);
        match rx.try_recv() {
            Ok(WorkerCmd::ApplyBands {
                payload,
                force,
                intent,
            }) => {
                assert!(
                    force,
                    "the mask is already the target: the write must be forced"
                );
                assert_eq!(payload.network_mode, "00", "the saved mode goes back");
                assert_eq!(payload.lte_band, "80000009", "the saved mask goes back");
                assert_eq!(
                    payload.network_option, None,
                    "the saved option goes back, null included"
                );
                match intent {
                    routwatch::write::WriteIntent::ApplyBands { mask, mode, .. } => {
                        assert_eq!(mask, "80000009");
                        assert_eq!(mode, "00");
                    }
                    other => panic!("{other:?}"),
                }
            }
            _ => panic!("u must send one apply, and only one"),
        }
        assert!(
            matches!(app.write, WriteState::Pending { .. }),
            "the revert is a write in flight, not a note"
        );
        assert_eq!(
            app.bands_selected,
            vec![1, 3],
            "the tick list follows the point being written"
        );
    }

    #[test]
    fn a_failed_write_leaves_u_with_nothing_to_send() {
        use routwatch::app::UndoPoint;
        use routwatch::write::WriteState;
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            routwatch::theme::by_name("dark"),
            routwatch::theme::GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.select_tab(Tab::Bands);
        app.bands_supported = vec![1, 3, 5];
        app.bands_selected = vec![1];
        app.undo_full = Some(UndoPoint {
            net_mode: "00".into(),
            bands: vec![1, 3],
            mask: "80000009".into(),
            option: None,
        });
        app.settle_write(WriteState::Failed {
            intent: app.band_intent(),
            err: "router refused".into(),
        });

        handle_key(&mut app, key(KeyCode::Char('u')), &tx);
        assert!(
            rx.try_recv().is_err(),
            "a failed write offers no revert to send"
        );
    }

    #[test]
    fn the_refresh_keys_work_without_the_settings_overlay() {
        use super::WorkerCmd;
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            routwatch::theme::by_name("dark"),
            routwatch::theme::GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );

        handle_key(&mut app, key(KeyCode::Char('[')), &tx);
        assert_eq!(app.refresh_ms, 1500, "`[` must shorten the period");
        assert!(
            matches!(rx.try_recv(), Ok(WorkerCmd::SetInterval(1500))),
            "the worker was not told about the new period"
        );

        handle_key(&mut app, key(KeyCode::Char(']')), &tx);
        assert_eq!(app.refresh_ms, 2000);
        assert!(matches!(rx.try_recv(), Ok(WorkerCmd::SetInterval(2000))));
    }

    #[test]
    fn completion_lists_exactly_the_cli_surface() {
        use clap::CommandFactory;
        let names: Vec<String> = <super::Cli as CommandFactory>::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        for name in &names {
            assert!(
                routwatch::completion::SUBCOMMANDS.contains(&name.as_str()),
                "completion is missing subcommand {name}"
            );
        }
        for sub in routwatch::completion::SUBCOMMANDS {
            assert!(
                names.contains(&(*sub).to_string()),
                "no such subcommand {sub}"
            );
        }
        for flag in ["--redact", "--report-identity", "--generate-config"] {
            assert!(
                routwatch::completion::GLOBAL_FLAGS.contains(&flag),
                "completion is missing {flag}"
            );
        }
    }

    #[test]
    fn shift_s_opens_settings_on_the_clients_tab() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            routwatch::theme::by_name("dark"),
            routwatch::theme::GraphStyle::Dots,
            View::Full,
            100,
            2000,
        );
        app.tab = Tab::Clients;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT),
            &tx,
        );
        assert!(app.settings_open, "Shift+S must open settings, not sort");
    }

    /// The list the help advertises and the list `Tab::parse` accepts must be
    /// the same list: nine sections, `dns` and `signal` included.
    #[test]
    fn the_tab_help_names_every_section_it_accepts() {
        let listed: Vec<&str> = TAB_HELP
            .split_once(": ")
            .expect("tab help keeps its `Start on a tab: …` shape")
            .1
            .split(", ")
            .collect();

        for tab in Tab::ALL {
            let name = tab.title().to_lowercase();
            assert!(
                listed.contains(&name.as_str()),
                "--tab help never names {name:?}: {TAB_HELP}"
            );
            assert!(
                Tab::parse(&name).is_some(),
                "Tab::parse rejects its own title {name:?}"
            );
        }
        for word in &listed {
            assert!(
                Tab::ALL
                    .iter()
                    .any(|t| t.title().eq_ignore_ascii_case(word)),
                "--tab help advertises {word:?}, which is not a tab"
            );
        }
        assert_eq!(
            listed.len(),
            Tab::ALL.len(),
            "the list repeats or misses one"
        );
    }

    #[test]
    fn the_guard_restores_once_on_drop() {
        use std::cell::Cell;
        use std::rc::Rc;
        let hits = Rc::new(Cell::new(0));
        let h = hits.clone();
        {
            let _g = RestoreOnDrop(Some(move || h.set(h.get() + 1)));
        }
        assert_eq!(hits.get(), 1);
    }

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

    #[test]
    fn shift_t_on_network_asks_before_spending_data() {
        let (tx, _rx) = channel();
        let mut app = bare_app();
        app.select_tab(Tab::Network);
        let before = app.theme.id.clone();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::SHIFT),
            &tx,
        );
        assert!(app.speed_confirm, "T must ask first");
        assert_eq!(app.theme.id, before, "T must not be read as the theme key");
        assert!(app.input.is_none());
    }

    #[test]
    fn shift_t_outside_network_still_cycles_the_theme() {
        let (tx, _rx) = channel();
        let mut app = bare_app();
        let before = app.theme.id.clone();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::SHIFT),
            &tx,
        );
        assert_ne!(app.theme.id, before);
    }

    #[test]
    fn escape_during_a_test_asks_the_thread_to_stop_and_sends_nothing_else() {
        let (tx, rx) = channel();
        let mut app = bare_app();
        app.select_tab(Tab::Network);
        app.start_speed_test();
        handle_key(&mut app, key(KeyCode::Esc), &tx);
        assert!(app.speed_cancel.load(Ordering::Relaxed));
        assert!(
            app.speed_running,
            "the panel waits for the thread's last event"
        );
        assert!(
            rx.try_recv().is_err(),
            "cancelling is a flag, not a command"
        );
    }

    #[test]
    fn escape_with_a_modal_open_closes_the_modal_not_the_test() {
        // The cancel arm sits after every overlay, so an open modal keeps its Esc.
        let (tx, _rx) = channel();
        let mut app = bare_app();
        app.select_tab(Tab::Network);
        app.start_speed_test();
        app.reset_confirm = true;
        handle_key(&mut app, key(KeyCode::Esc), &tx);
        assert!(!app.reset_confirm, "the modal took the key");
        assert!(
            !app.speed_cancel.load(Ordering::Relaxed),
            "the test was not cancelled"
        );
    }

    #[test]
    fn enter_in_the_confirm_modal_sends_one_speed_command() {
        let (tx, rx) = channel();
        let mut app = bare_app();
        app.select_tab(Tab::Network);
        app.begin_speed_test();
        handle_key(&mut app, key(KeyCode::Enter), &tx);
        assert!(!app.speed_confirm);
        assert!(app.speed_running);
        match rx.try_recv() {
            Ok(WorkerCmd::SpeedTest(cfg, host, id, _)) => {
                assert_eq!(id, app.speed_run);
                assert_eq!(cfg.max_mb, app.speed_cfg.max_mb);
                assert_eq!(host, app.speed_cfg.ping_host, "the host the run pings");
            }
            other => panic!("expected one SpeedTest command, got {other:?}"),
        }
    }
}
