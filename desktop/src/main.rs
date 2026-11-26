//! openRMS Watch desktop: a window onto the watcher's local web page plus a tray icon.
//!
//! The engine (`openrms-watch run`) normally runs as a systemd user service set
//! up by the installer, so it keeps watching when this window is closed. This
//! app finds it from the config's `listen` address, starts the service (or, if
//! there is no service, the engine itself) when it isn't running, and shows its
//! page. All settings live in that page, the same as on a headless install.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

struct Engine {
    config: PathBuf,
    url: String,
    /// Set when this app started the engine itself (no systemd service).
    child: Mutex<Option<Child>>,
}

fn config_path() -> PathBuf {
    if let Some(p) = std::env::args().skip_while(|a| a != "--config").nth(1) {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("OPENRMS_WATCH_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("openrms-watch/watch.toml")
}

/// First run of a packaged app: write the default config, pointing at the
/// Mercury bundled with the app.
fn first_run_config(config: &PathBuf, resources: Option<PathBuf>) {
    let Some(res) = resources else { return };
    // An AppImage is mounted at a new temporary path on every launch, so its
    // Mercury is copied somewhere stable (refreshed each launch, for upgrades).
    let mercury = if std::env::var_os("APPIMAGE").is_some() {
        let dst = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("openrms-watch/mercury");
        let src = res.join("mercury");
        let stale = std::fs::metadata(&dst).map(|m| m.len()) .ok() != std::fs::metadata(&src).map(|m| m.len()).ok();
        if stale {
            let _ = std::fs::create_dir_all(dst.parent().unwrap());
            let tmp = dst.with_extension("new");
            if std::fs::copy(&src, &tmp).is_ok() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
                let _ = std::fs::rename(&tmp, &dst);
            }
        }
        dst
    } else {
        res.join("mercury")
    };
    if config.exists() {
        return;
    }
    let Ok(tpl) = std::fs::read_to_string(res.join("watch.toml.default")) else { return };
    let text = tpl
        .replace("@LIBDIR@/mercury", &mercury.display().to_string())
        .replace("@LIBDIR@", &res.display().to_string())
        .replace("@PORT@", "17373");
    if let Some(dir) = config.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(config, text).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(config, std::fs::Permissions::from_mode(0o600));
    }
}

fn autostart_file() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("autostart/openrms-watch-desktop.desktop")
}

fn set_autostart(on: bool) {
    let f = autostart_file();
    if on {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "openrms-watch-desktop".into());
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        let _ = std::fs::write(
            &f,
            format!("[Desktop Entry]\nType=Application\nName=openRMS Watch\nExec=\"{exe}\" --hidden\nX-GNOME-Autostart-enabled=true\n"),
        );
    } else {
        let _ = std::fs::remove_file(&f);
    }
}

fn page_url(config: &PathBuf) -> String {
    let listen = std::fs::read_to_string(config)
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| t.get("watch")?.get("listen")?.as_str().map(String::from))
        .unwrap_or_else(|| "127.0.0.1:17373".into());
    let (host, port) = listen.rsplit_once(':').unwrap_or(("127.0.0.1", "17373"));
    let host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
    format!("http://{host}:{port}/")
}

fn engine_up(url: &str) -> bool {
    ureq::get(&format!("{url}api/live")).timeout(Duration::from_secs(2)).call().is_ok()
}

fn has_user_service() -> bool {
    Command::new("systemctl")
        .args(["--user", "cat", "openrms-watch.service"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The engine binary: next to this app, else on PATH.
fn engine_bin() -> PathBuf {
    if let Ok(me) = std::env::current_exe() {
        let side = me.with_file_name("openrms-watch");
        if side.exists() {
            return side;
        }
    }
    PathBuf::from("openrms-watch")
}

impl Engine {
    /// Start the engine if it isn't running. Call on the main thread: an engine
    /// started here gets PR_SET_PDEATHSIG, which follows the *thread* that
    /// spawned it, and the main thread lives as long as the app.
    fn start(&self) -> Result<(), String> {
        if engine_up(&self.url) {
            return Ok(());
        }
        if has_user_service() {
            Command::new("systemctl")
                .args(["--user", "start", "openrms-watch.service"])
                .status()
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        let mut cmd = Command::new(engine_bin());
        cmd.arg("-c").arg(&self.config).arg("run");
        // an engine this app started must not outlive it, even if the app is killed
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let child = cmd.spawn().map_err(|e| format!("couldn't start openrms-watch: {e}"))?;
        *self.child.lock().unwrap() = Some(child);
        Ok(())
    }

    /// Wait (off the main thread) until the page answers.
    fn wait_up(&self) -> Result<(), String> {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if engine_up(&self.url) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        Err("the watcher didn't come up within 20 s".into())
    }

    /// Main thread.
    fn restart(&self) {
        if has_user_service() && self.child.lock().unwrap().is_none() {
            let _ = Command::new("systemctl").args(["--user", "restart", "openrms-watch.service"]).spawn();
        } else {
            self.stop();
            let _ = self.start();
        }
    }

    fn stop(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            // SIGTERM lets it stop its decoders cleanly
            let _ = Command::new("kill").arg(c.id().to_string()).status();
            let _ = c.wait();
        } else if has_user_service() {
            let _ = Command::new("systemctl").args(["--user", "stop", "openrms-watch.service"]).status();
        }
    }
}

/// One line for the tray: what the watcher is doing.
fn status_line(url: &str) -> String {
    let Ok(r) = ureq::get(&format!("{url}api/live")).timeout(Duration::from_secs(2)).call() else {
        return "Watcher not running".into();
    };
    let Ok(v) = r.into_json::<serde_json::Value>() else { return "Watcher running".into() };
    let e = &v["engine"];
    let bands = e["segments"].as_array().map_or(0, |a| a.len());
    let busy = e["decoders"].as_array().map_or(0, |a| a.iter().filter(|d| d["state"] != "idle").count());
    let contrib = &v["contribute"];
    let c = if contrib["enabled"] == true {
        if contrib["error"].is_string() { " · upload problem" } else { " · contributing" }
    } else {
        ""
    };
    let clip = e["receivers"].as_object().is_some_and(|m| m.values().any(|r| r["clip_pct"].as_f64().unwrap_or(0.0) > 0.5));
    format!(
        "{} band{} · {} decoder{} busy{}{}",
        bands,
        if bands == 1 { "" } else { "s" },
        busy,
        if busy == 1 { "" } else { "s" },
        c,
        if clip { " · SDR clipping" } else { "" }
    )
}

fn show_window(app: &AppHandle, url: &str) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let target: WebviewUrl = match url.parse() {
        Ok(u) => WebviewUrl::External(u),
        Err(_) => WebviewUrl::App("index.html".into()),
    };
    if let Ok(w) = WebviewWindowBuilder::new(app, "main", target)
        .title("openRMS Watch")
        .inner_size(1240.0, 820.0)
        .min_inner_size(720.0, 480.0)
        .build()
    {
        // closing the window keeps the watcher (and the tray) running
        let w2 = w.clone();
        w.on_window_event(move |ev| {
            if let WindowEvent::CloseRequested { api, .. } = ev {
                api.prevent_close();
                let _ = w2.hide();
            }
        });
    }
}

fn main() {
    let config = config_path();
    // started from the login autostart entry: tray only, no window
    let hidden = std::env::args().any(|a| a == "--hidden");
    let engine = Arc::new(Engine { url: String::new(), config, child: Mutex::new(None) });

    tauri::Builder::default()
        .setup({
            let engine_slot = engine.clone();
            move |app| {
                first_run_config(&engine_slot.config, app.path().resource_dir().ok());
                // now that the config exists, read where its page lives
                let engine = Arc::new(Engine {
                    url: page_url(&engine_slot.config),
                    config: engine_slot.config.clone(),
                    child: Mutex::new(None),
                });
                app.manage(engine.clone());
                let handle = app.handle().clone();
                let open = MenuItem::with_id(app, "open", "Open openRMS Watch", true, None::<&str>)?;
                let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
                let restart = MenuItem::with_id(app, "restart", "Restart watcher", true, None::<&str>)?;
                let autostart = tauri::menu::CheckMenuItem::with_id(
                    app, "autostart", "Start at login", true, autostart_file().exists(), None::<&str>,
                )?;
                let stop = MenuItem::with_id(app, "stop", "Stop watcher and quit", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "Close window (keep watching)", true, None::<&str>)?;
                let sep = PredefinedMenuItem::separator(app)?;
                let sep2 = PredefinedMenuItem::separator(app)?;
                let menu = Menu::with_items(app, &[&open, &sep, &status, &sep2, &restart, &autostart, &stop, &quit])?;
                let tray = TrayIconBuilder::with_id("main")
                    .icon(app.default_window_icon().cloned().expect("icon"))
                    .tooltip("openRMS Watch")
                    .menu(&menu)
                    .on_menu_event({
                        let engine = engine.clone();
                        move |app, ev| match ev.id().as_ref() {
                            "open" => show_window(app, &engine.url),
                            "restart" => engine.restart(),
                            "autostart" => set_autostart(!autostart_file().exists()),
                            "stop" => {
                                engine.stop();
                                app.exit(0);
                            }
                            "quit" => app.exit(0),
                            _ => {}
                        }
                    })
                    .build(app)?;

                // start the engine if needed (here, on the main thread), then open the page
                let started = engine.start();
                {
                    let (engine, handle) = (engine.clone(), handle.clone());
                    std::thread::spawn(move || {
                        let r = started.and_then(|_| engine.wait_up());
                        let url = engine.url.clone();
                        let h = handle.clone();
                        let _ = handle.run_on_main_thread(move || match r {
                            Ok(_) if hidden => {}
                            Ok(_) => show_window(&h, &url),
                            Err(e) => {
                                eprintln!("openrms-watch-desktop: {e}");
                                show_window(&h, "app://index.html");
                            }
                        });
                    });
                }
                // keep the tray status fresh
                {
                    let url = engine.url.clone();
                    std::thread::spawn(move || loop {
                        let line = status_line(&url);
                        let _ = status.set_text(&line);
                        let _ = tray.set_tooltip(Some(format!("openRMS Watch: {line}")));
                        std::thread::sleep(Duration::from_secs(10));
                    });
                }
                Ok(())
            }
        })
        .build(tauri::generate_context!())
        .expect("starting openRMS Watch desktop")
        .run(move |app, ev| match ev {
            // keep running with only the tray when the window is hidden
            tauri::RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
            tauri::RunEvent::Exit => {
                if let Some(e) = app.try_state::<Arc<Engine>>() {
                    if let Some(mut c) = e.child.lock().unwrap().take() {
                        let _ = Command::new("kill").arg(c.id().to_string()).status();
                        let _ = c.wait();
                    }
                }
            }
            _ => {}
        });
    // closing the app never stops a service-run engine; an engine we started
    // dies with us (PR_SET_PDEATHSIG covers a killed app too)
    let _ = engine;
}
