use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub watch: Watch,
    #[serde(default, rename = "receiver")]
    pub receivers: Vec<Receiver>,
    #[serde(default, rename = "channel")]
    pub channels: Vec<Channel>,
    #[serde(default, rename = "wideband")]
    pub widebands: Vec<Wideband>,
    #[serde(default)]
    pub web: WebCfg,
    /// `[atlas]` (openRMS-Atlas, formerly openRMS-Directory). The old `[directory]`
    /// section name still loads, with a deprecation warning.
    #[serde(default, alias = "directory")]
    pub atlas: AtlasCfg,
    #[serde(default)]
    pub contribute: Contribute,
}

/// Web page access. With a password set, changing settings needs it; a page
/// reachable from the network (`listen` not on 127.0.0.1) refuses changes until
/// one is set.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct WebCfg {
    /// "salt$sha256hex", written by `openrms-watch set-password` or the settings page.
    pub password_hash: Option<String>,
}

/// openRMS-Atlas (the gateway registry, formerly openRMS-Directory): its callsign
/// list names stations locally (no account needed), and contributing uploads to it.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct AtlasCfg {
    /// e.g. "https://atlas.openrms.dev"; unset = fully offline.
    pub url: Option<String>,
    /// openRMS-Accounts, where "Link to openRMS" signs in (default https://auth.openrms.dev).
    pub accounts_url: Option<String>,
}

impl AtlasCfg {
    pub fn accounts(&self) -> String {
        self.accounts_url.clone().unwrap_or_else(|| "https://auth.openrms.dev".into()).trim_end_matches('/').to_string()
    }
}

/// Opt-in upload of what this watcher hears (openRMS-Spec/watch/feed.md).
#[derive(Debug, Deserialize, Clone)]
pub struct Contribute {
    #[serde(default)]
    pub enabled: bool,
    /// The watcher key (ormsw_...) from "My watchers" on the site.
    pub token: Option<String>,
    #[serde(default = "d_interval")]
    pub interval_s: u64,
}

impl Default for Contribute {
    fn default() -> Self {
        Contribute { enabled: false, token: None, interval_s: d_interval() }
    }
}

fn d_interval() -> u64 { 600 } // batches every 10 minutes (Atlas: "live" = uploaded within 25 min)

/// One SDR scanning whole band segments: Mercury connect bursts are found in
/// the spectrum and a decoder from a pool is pointed at each one.
#[derive(Debug, Deserialize, Clone)]
pub struct Wideband {
    pub receiver: String,
    /// Shell command writing raw IQ to stdout. {center_hz}, {rate} and {ppm}
    /// are substituted.
    pub command: Option<String>,
    /// Or a raw IQ file replayed at real time (testing).
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub loop_file: bool,
    /// Or an SDR opened through SoapySDR: its args string from `openrms-watch devices`,
    /// e.g. "driver=hackrf,serial=...". The usual choice.
    pub device: Option<String>,
    /// Overall gain (dB), or per stage in `gains` (e.g. { LNA = 24, VGA = 20, AMP = 0 }).
    pub gain: Option<f64>,
    #[serde(default)]
    pub gains: std::collections::BTreeMap<String, f64>,
    pub antenna: Option<String>,
    /// Analog filter bandwidth, Hz (default: the driver's choice for the rate).
    pub bandwidth: Option<f64>,
    /// Upconverter LO in Hz (e.g. 125000000 for a Ham It Up): the SDR is tuned to
    /// center_hz + upconverter_hz. Frequencies everywhere else stay the real RF.
    #[serde(default)]
    pub upconverter_hz: f64,
    /// Driver settings, e.g. { direct_samp = "2" } for an RTL-SDR on HF.
    #[serde(default)]
    pub settings: std::collections::BTreeMap<String, String>,
    /// cs8 (HackRF), cu8 (RTL-SDR), cs16 (Pluto): for `command` and `file` sources only.
    #[serde(default)]
    pub format: String,
    /// Sample rate, Hz; must be a multiple of 8000.
    pub rate: u64,
    pub center_hz: u64,
    /// Band segments to scan, [low_hz, high_hz].
    pub segments: Vec<[u64; 2]>,
    /// Size of the decoder pool (each ~21% of a core with connect_only).
    #[serde(default = "d_decoders")]
    pub decoders: usize,
    /// Detection threshold over the local noise floor, dB (default 4).
    pub threshold_db: Option<f64>,
    /// Seconds of audio replayed into a decoder before the detection, so the
    /// first CALL is not lost.
    #[serde(default = "d_preroll")]
    pub preroll_s: f64,
}

fn d_decoders() -> usize { 8 }
fn d_preroll() -> f64 { 6.0 }

#[derive(Debug, Deserialize)]
pub struct Watch {
    /// Path to a Mercury built with patches/mercury-monitor-crc.patch.
    pub mercury: PathBuf,
    #[serde(default = "d_db")]
    pub db: PathBuf,
    #[serde(default = "d_listen")]
    pub listen: String,
    /// First TCP port handed to Mercury instances (each takes 2, plus a broadcast port).
    #[serde(default = "d_base_port")]
    pub base_port: u16,
    /// Where per-instance FIFOs, INIs and logs go. Default: $XDG_RUNTIME_DIR/openrms-watch.
    pub runtime_dir: Option<PathBuf>,
    /// An RMS not heard for this long, while we were listening on its channel, is "stale".
    #[serde(default = "d_stale")]
    pub stale_hours: f64,
    /// ...and "gone" after this many days.
    #[serde(default = "d_gone")]
    pub gone_days: f64,
    /// CALL retries / ACCEPT / DISCONNECT within this window belong to one attempt.
    #[serde(default = "d_window")]
    pub attempt_window_s: i64,
    /// Optional file of known callsigns (one per line, exact form incl. SSID)
    /// used to name the RMS behind a CRC. Re-read at startup.
    pub calls_file: Option<PathBuf>,
    /// Run only the decoders that carry connect frames (DATAC16, MFSK,
    /// MFSK500): about half the CPU. Off: also log broadcast and 1.9.x frames.
    #[serde(default = "d_true")]
    pub connect_only: bool,
}

fn d_db() -> PathBuf { "watch.db".into() }
fn d_listen() -> String { "127.0.0.1:17373".into() }
fn d_base_port() -> u16 { 17400 }
fn d_stale() -> f64 { 24.0 }
fn d_gone() -> f64 { 7.0 }
fn d_window() -> i64 { 180 }

#[derive(Debug, Deserialize, Clone)]
pub struct Receiver {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// local-sdr, kiwisdr, file, ... (free text, shown in the UI)
    #[serde(default)]
    pub kind: String,
    /// Maidenhead grid or free-text location.
    #[serde(default)]
    pub location: String,
    /// Receiver frequency error in ppm; positive means signals come out LOW
    /// (so the watcher shifts them up by ppm * dial frequency). If a channel's
    /// command contains {ppm}, the tool (rtl_fm -p {ppm}) corrects it instead.
    /// Mercury only acquires within about +/-50 Hz: 20 ppm is 280 Hz at 14 MHz.
    #[serde(default)]
    pub ppm: f64,
    /// Fixed audio shift in Hz for every channel of this receiver (e.g. an
    /// upconverter's LO error, which does not scale with the dial frequency).
    #[serde(default)]
    pub offset_hz: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Channel {
    pub receiver: String,
    /// USB dial frequency of the RMS channel, Hz.
    pub dial_hz: u64,
    #[serde(default)]
    pub label: String,
    /// Shell command that writes raw s16le mono 8 kHz audio to stdout.
    /// {dial_hz}, {dial_khz} and {ppm} are substituted.
    pub command: Option<String>,
    /// Or: replay a raw s16le 8 kHz file in real time (testing).
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub loop_file: bool,
    /// Extra decoders at these audio shifts (Hz), to search for an off-frequency
    /// station. Each entry costs one more Mercury (~half a core). Default [0].
    #[serde(default = "d_shifts")]
    pub shifts_hz: Vec<f64>,
    #[serde(default = "d_true")]
    pub enabled: bool,
}

fn d_shifts() -> Vec<f64> { vec![0.0] }
fn d_true() -> bool { true }

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if let Ok(raw) = text.parse::<toml::Table>() {
            if raw.contains_key("directory") {
                tracing::warn!("{}: [directory] is now [atlas] (openRMS-Atlas); please rename the section", path.display());
            }
        }
        // Relative paths are relative to the config file, made absolute because
        // Mercury runs from its own working directory.
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let base = std::path::absolute(parent)?;
        let rel = |p: &mut PathBuf| {
            let joined = if p.is_relative() { base.join(&*p) } else { p.clone() };
            // tidy "a/../b" without requiring the path to exist yet
            let mut out = PathBuf::new();
            for c in joined.components() {
                match c {
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    std::path::Component::CurDir => {}
                    c => out.push(c),
                }
            }
            *p = out;
        };
        rel(&mut cfg.watch.mercury);
        rel(&mut cfg.watch.db);
        if let Some(p) = cfg.watch.calls_file.as_mut() {
            rel(p);
        }
        if let Some(p) = cfg.watch.runtime_dir.as_mut() {
            rel(p);
        }
        for c in cfg.channels.iter_mut() {
            if let Some(p) = c.file.as_mut() {
                rel(p);
            }
        }
        for w in cfg.widebands.iter_mut() {
            if let Some(p) = w.file.as_mut() {
                rel(p);
            }
        }
        for w in &cfg.widebands {
            if !cfg.receivers.iter().any(|r| r.id == w.receiver) {
                bail!("wideband names unknown receiver {:?}", w.receiver);
            }
            let n = [w.command.is_some(), w.file.is_some(), w.device.is_some()].iter().filter(|b| **b).count();
            if n != 1 {
                bail!("wideband {} needs exactly one of device, command or file", w.receiver);
            }
            if w.rate % 8000 != 0 {
                bail!("wideband {}: rate {} is not a multiple of 8000", w.receiver, w.rate);
            }
            if w.device.is_none() && !matches!(w.format.as_str(), "cs8" | "cu8" | "cs16") {
                bail!("wideband {}: format must be cs8, cu8 or cs16", w.receiver);
            }
            let half = w.rate as f64 * 0.45;
            for [lo, hi] in &w.segments {
                if hi <= lo {
                    bail!("wideband {}: segment {lo}-{hi} is empty", w.receiver);
                }
                if (*lo as f64) < w.center_hz as f64 - half || (*hi as f64) > w.center_hz as f64 + half {
                    bail!("wideband {}: segment {lo}-{hi} is outside the usable span {:.0}-{:.0}",
                          w.receiver, w.center_hz as f64 - half, w.center_hz as f64 + half);
                }
                if *lo <= w.center_hz + 5000 && *hi + 5000 >= w.center_hz {
                    bail!("wideband {}: segment {lo}-{hi} covers the centre frequency (DC spike); move center_hz", w.receiver);
                }
            }
        }
        for c in &cfg.channels {
            if !cfg.receivers.iter().any(|r| r.id == c.receiver) {
                bail!("channel {} names unknown receiver {:?}", c.dial_hz, c.receiver);
            }
            if c.command.is_some() == c.file.is_some() {
                bail!("channel {} needs exactly one of command or file", c.dial_hz);
            }
            if c.shifts_hz.is_empty() {
                bail!("channel {} has an empty shifts_hz", c.dial_hz);
            }
        }
        Ok(cfg)
    }

    pub fn receiver(&self, id: &str) -> &Receiver {
        self.receivers.iter().find(|r| r.id == id).expect("validated")
    }
}

pub fn band_of(hz: u64) -> &'static str {
    let mhz = hz as f64 / 1e6;
    match mhz {
        m if (1.8..2.0).contains(&m) => "160m",
        m if (3.5..4.0).contains(&m) => "80m",
        m if (5.25..5.45).contains(&m) => "60m",
        m if (7.0..7.3).contains(&m) => "40m",
        m if (10.1..10.15).contains(&m) => "30m",
        m if (14.0..14.35).contains(&m) => "20m",
        m if (18.068..18.168).contains(&m) => "17m",
        m if (21.0..21.45).contains(&m) => "15m",
        m if (24.89..24.99).contains(&m) => "12m",
        m if (28.0..29.7).contains(&m) => "10m",
        m if (50.0..54.0).contains(&m) => "6m",
        m if (144.0..148.0).contains(&m) => "2m",
        m if (420.0..450.0).contains(&m) => "70cm",
        _ => "other",
    }
}
