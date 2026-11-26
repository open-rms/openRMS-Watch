mod config;
mod cdsp;
mod crc;
mod db;
mod detect;
mod dsp;
mod live;
mod monitor;
mod naming;
mod plan;
mod ports;
mod runner;
mod sdr;
mod settings;
mod upload;
mod web;
mod wide;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(version, about = "Watch HF channels for Mercury connections and log the RMS stations heard")]
struct Cli {
    /// Config file
    #[arg(short, long, default_value = "watch.toml", global = true)]
    config: PathBuf,
    /// Web page port (default 17373; also OPENRMS_WATCH_PORT). Overrides the config file.
    #[arg(long, global = true)]
    port: Option<u16>,
    /// Web page address, e.g. 0.0.0.0:17373 to allow other computers. Overrides the config file.
    #[arg(long, global = true)]
    listen: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run all channels and the web page
    Run,
    /// Serve the web page only (no receivers)
    Serve,
    /// Add callsigns (one per line) to the CRC lookup list
    ImportCalls { file: PathBuf },
    /// Recompute connection attempts from the raw frame log
    Rebuild,
    /// Show the decoder plan without starting anything
    Check,
    /// Print the Mercury CRC16 of callsigns
    Crc { calls: Vec<String> },
    /// Open the web page in the default browser
    Open,
    /// Set the web page password (needed to change settings from another computer)
    SetPassword { password: String },
    /// List attached SDRs; with --probe, open each and show its capabilities
    Devices {
        #[arg(long)]
        probe: bool,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    if let Cmd::Devices { probe } = &cli.cmd {
        let devs = sdr::list()?;
        if devs.is_empty() {
            println!("no SDRs found (is the SoapySDR module for your radio installed? try SoapySDRUtil --find)");
        }
        for d in devs {
            println!("{}  [{}]", d.label, d.args);
            if *probe {
                match sdr::probe(&d.args) {
                    Ok(c) => println!("{}", serde_json::to_string_pretty(&c)?),
                    Err(e) => println!("  probe failed: {e:#}"),
                }
            }
        }
        return Ok(());
    }
    if let Cmd::Crc { calls } = &cli.cmd {
        for c in calls {
            println!("{:04X}  {}", crc::callsign_crc16(c), crc::normalise(c));
        }
        return Ok(());
    }
    let mut cfg = config::Config::load(&cli.config)?;
    if let Some(l) = &cli.listen {
        cfg.watch.listen = l.clone();
    }
    let env_port = std::env::var("OPENRMS_WATCH_PORT").ok().and_then(|p| p.parse::<u16>().ok());
    if let Some(p) = cli.port.or(env_port) {
        cfg.watch.listen = ports::with_port(&cfg.watch.listen, p);
    }
    let conn = db::open(&cfg.watch.db)?;
    match cli.cmd {
        Cmd::ImportCalls { file } => {
            let n = db::import_calls_file(&conn, &file)?;
            println!("added {n} callsigns");
        }
        Cmd::Rebuild => {
            let n = db::rebuild(&conn, cfg.watch.attempt_window_s)?;
            println!("replayed {n} frames");
        }
        Cmd::Check => {
            let mut alloc = ports::PortAlloc::new(cfg.watch.base_port, 600);
            let fixed = plan(&cfg, &conn, &mut alloc)?;
            for w in wide_plan(&cfg, &conn, &mut alloc)? {
                println!(
                    "wideband {:<12} {} @ {:.3} MHz, {:.1} MS/s, {} decoders from port {}",
                    w.receiver.id, w.cfg.format, w.cfg.center_hz as f64 / 1e6, w.cfg.rate as f64 / 1e6,
                    w.cfg.decoders, w.ports.first().copied().unwrap_or(0)
                );
                for [lo, hi] in &w.cfg.segments {
                    println!("    scan {:.3}-{:.3} MHz", *lo as f64 / 1e6, *hi as f64 / 1e6);
                }
            }
            for (i, inst) in fixed.iter().enumerate() {
                println!(
                    "{i:2} {:<28} port {} shift {:+.0} Hz  {}",
                    inst.name,
                    inst.port,
                    inst.total_shift_hz(),
                    inst.channel.command.as_deref().map(|_| "command").unwrap_or("file")
                );
            }
        }
        Cmd::Serve => {
            drop(conn);
            if serve(&cfg, &cli.config, None)? {
                reexec()?;
            }
        }
        Cmd::Run => {
            if let Some(f) = &cfg.watch.calls_file {
                match db::import_calls_file(&conn, f) {
                    Ok(n) => info!("calls file {}: {n} new callsigns", f.display()),
                    Err(e) => warn!("calls file {}: {e}", f.display()),
                }
            }
            if !cfg.watch.mercury.exists() {
                anyhow::bail!("mercury binary not found: {} (run scripts/build-mercury.sh)", cfg.watch.mercury.display());
            }
            let _lock = lock_runtime(&runtime_dir(&cfg))?;
            let mut alloc = ports::PortAlloc::new(cfg.watch.base_port, 600);
            let instances = plan(&cfg, &conn, &mut alloc)?;
            let conn_for_plan = db::open(&cfg.watch.db)?;
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, rx) = std::sync::mpsc::channel::<runner::Event>();
            let window = cfg.watch.attempt_window_s;
            let contribute = cfg.contribute.enabled && cfg.contribute.token.is_some() && cfg.atlas.url.is_some();
            if cfg.contribute.enabled && !contribute {
                warn!("contribute is on but [atlas] url or [contribute] token is missing; staying local");
            }
            let meta: std::collections::HashMap<String, upload::ReceiverMeta> = cfg
                .receivers
                .iter()
                .map(|r| (r.id.clone(), upload::ReceiverMeta { ppm: r.ppm }))
                .collect();
            let writer = std::thread::spawn(move || {
                for ev in rx {
                    let r = (|| -> Result<()> {
                        match ev {
                            runner::Event::Frame(h) => {
                                let t = conn.unchecked_transaction()?;
                                let eid = db::insert_frame(&t, &h)?;
                                db::apply_frame(&t, &h, window)?;
                                if contribute {
                                    let ev = upload::frame_event(&t, &h, &eid, &meta)?;
                                    upload::enqueue(&t, &eid, &ev)?;
                                }
                                t.commit()?;
                            }
                            runner::Event::Coverage { channel_id, start_ms, end_ms } => {
                                db::record_coverage(&conn, channel_id, start_ms, end_ms)?;
                                if contribute {
                                    let ev = upload::coverage_event(&conn, channel_id, start_ms, end_ms)?;
                                    upload::enqueue(&conn, ev["id"].as_str().unwrap(), &ev)?;
                                }
                            }
                        }
                        Ok(())
                    })();
                    if let Err(e) = r {
                        error!("db: {e:#}");
                    }
                }
            });
            let mut threads = Vec::new();
            for w in wide_plan(&cfg, &conn_for_plan, &mut alloc)? {
                threads.extend(wide::start(w, tx.clone(), stop.clone())?);
            }
            drop(conn_for_plan);
            {
                let ucfg = upload::UploaderCfg {
                    db: cfg.watch.db.clone(),
                    atlas_url: cfg.atlas.url.clone(),
                    enabled: contribute,
                    token: cfg.contribute.token.clone(),
                    interval: std::time::Duration::from_secs(cfg.contribute.interval_s.max(10)),
                    window_s: cfg.watch.attempt_window_s,
                    sdr_label: cfg.receivers.iter().map(|r| if r.name.is_empty() { r.id.clone() } else { r.name.clone() }).collect::<Vec<_>>().join(", "),
                    decoders: cfg.widebands.iter().map(|w| w.decoders).sum::<usize>()
                        + cfg.channels.iter().filter(|c| c.enabled).map(|c| c.shifts_hz.len()).sum::<usize>(),
                    ppm: cfg.receivers.first().map(|r| r.ppm).unwrap_or(0.0),
                };
                let stop = stop.clone();
                threads.push(std::thread::spawn(move || upload::run(ucfg, stop)));
            }
            info!("starting {} fixed-channel decoder(s)", instances.len());
            for inst in instances {
                let (tx, stop) = (tx.clone(), stop.clone());
                threads.push(std::thread::spawn(move || inst.run(tx, stop)));
            }
            drop(tx);
            let restart = serve(&cfg, &cli.config, Some(stop.clone()))?;
            stop.store(true, Ordering::Relaxed);
            info!("stopping decoders");
            for t in threads {
                let _ = t.join();
            }
            let _ = writer.join();
            if restart {
                reexec()?;
            }
        }
        Cmd::Open => {
            let (host, port) = cfg.watch.listen.rsplit_once(':').unwrap_or(("127.0.0.1", "17373"));
            let host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
            let url = format!("http://{host}:{port}/");
            if std::process::Command::new("xdg-open").arg(&url).status().map(|s| !s.success()).unwrap_or(true) {
                println!("{url}");
            }
        }
        Cmd::SetPassword { password } => {
            let mut s = settings::from_config(&cfg);
            s.web.password = Some(password);
            let text = std::fs::read_to_string(&cli.config)?;
            let mut doc: toml_edit::DocumentMut = text.parse()?;
            if !doc.contains_table("web") {
                doc["web"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            doc["web"]["password_hash"] = toml_edit::value(settings::hash_password(s.web.password.as_deref().unwrap()));
            settings::save(&cli.config, &doc.to_string())?;
            println!("password set");
        }
        Cmd::Crc { .. } | Cmd::Devices { .. } => unreachable!(),
    }
    Ok(())
}

fn runtime_dir(cfg: &config::Config) -> PathBuf {
    cfg.watch.runtime_dir.clone().unwrap_or_else(|| {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("openrms-watch")
    })
}

/// Hold an exclusive lock on the runtime dir: two watchers sharing it would
/// fight over the same FIFOs and Mercury ports.
fn lock_runtime(dir: &std::path::Path) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(dir)?;
    let f = std::fs::File::create(dir.join("watch.lock"))?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        anyhow::bail!("another openrms-watch is already running with runtime dir {}", dir.display());
    }
    Ok(f)
}

fn wide_plan(cfg: &config::Config, conn: &rusqlite::Connection, alloc: &mut ports::PortAlloc) -> Result<Vec<wide::WideCtx>> {
    let runtime = runtime_dir(cfg);
    let mut out = Vec::new();
    for w in &cfg.widebands {
        let receiver = cfg.receiver(&w.receiver).clone();
        db::upsert_receiver(conn, &receiver.id, &receiver.name, &receiver.kind, &receiver.location)?;
        let seg_channels = w
            .segments
            .iter()
            .map(|[lo, hi]| {
                db::upsert_range(conn, &receiver.id, *lo, *hi, &format!("scan {}", config::band_of((lo + hi) / 2)))
            })
            .collect::<Result<Vec<_>>>()?;
        let ports = (0..w.decoders.max(1)).map(|_| alloc.take3()).collect::<Result<Vec<_>>>()?;
        out.push(wide::WideCtx {
            cfg: w.clone(),
            dir: runtime.join(format!("wide-{}", receiver.id)),
            receiver,
            seg_channels,
            mercury: cfg.watch.mercury.clone(),
            ports,
            connect_only: cfg.watch.connect_only,
        });
    }
    Ok(out)
}

fn plan(cfg: &config::Config, conn: &rusqlite::Connection, alloc: &mut ports::PortAlloc) -> Result<Vec<runner::Instance>> {
    let runtime = runtime_dir(cfg);
    for r in &cfg.receivers {
        db::upsert_receiver(conn, &r.id, &r.name, &r.kind, &r.location)?;
    }
    let mut out = Vec::new();
    for ch in cfg.channels.iter().filter(|c| c.enabled) {
        let channel_id = db::upsert_channel(conn, &ch.receiver, ch.dial_hz, &ch.label)?;
        for &shift in &ch.shifts_hz {
            let name = if ch.shifts_hz.len() > 1 {
                format!("{}@{}{:+.0}", ch.receiver, ch.dial_hz, shift)
            } else {
                format!("{}@{}", ch.receiver, ch.dial_hz)
            };
            out.push(runner::Instance {
                dir: runtime.join(name.replace(['@', '+'], "_")),
                name,
                channel: ch.clone(),
                channel_id,
                receiver: cfg.receiver(&ch.receiver).clone(),
                shift_hz: shift,
                port: alloc.take3()?,
                mercury: cfg.watch.mercury.clone(),
                connect_only: cfg.watch.connect_only,
            });
        }
    }
    Ok(out)
}

/// Serve the web page until Ctrl-C or a settings save. Returns true if the
/// process should restart itself (settings changed).
fn serve(cfg: &config::Config, config_path: &std::path::Path, stop: Option<Arc<AtomicBool>>) -> Result<bool> {
    let host = cfg.watch.listen.rsplit_once(':').map(|x| x.0).unwrap_or("127.0.0.1");
    let restart = Arc::new(tokio::sync::Notify::new());
    let restart_flag = Arc::new(AtomicBool::new(false));
    let state = web::AppState {
        db: cfg.watch.db.clone(),
        stale_hours: cfg.watch.stale_hours,
        gone_days: cfg.watch.gone_days,
        config_path: std::path::absolute(config_path)?,
        password_hash: cfg.web.password_hash.clone(),
        lan: !matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]"),
        restart: restart.clone(),
        restart_flag: restart_flag.clone(),
        atlas_url: cfg.atlas.url.clone(),
        accounts_url: cfg.atlas.accounts(),
        link: Default::default(),
        link_token: Default::default(),
    };
    let listen = cfg.watch.listen.clone();
    tokio::runtime::Runtime::new()?.block_on(async move {
        let l = tokio::net::TcpListener::bind(&listen).await.with_context(|| format!("binding {listen}"))?;
        info!("web page on http://{listen}/");
        let stop2 = stop.clone();
        axum::serve(l, web::router(state))
            .with_graceful_shutdown(async move {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                    _ = restart.notified() => {}
                }
                info!("shutting down");
                if let Some(s) = stop2 {
                    s.store(true, Ordering::Relaxed);
                }
            })
            .await?;
        anyhow::Ok(())
    })?;
    Ok(restart_flag.load(Ordering::SeqCst))
}

/// Replace this process with a fresh copy (same arguments) to pick up new settings.
fn reexec() -> Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    info!("restarting with new settings");
    let err = std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec();
    Err(err.into())
}
