//! The settings page's model of the config file.
//!
//! The page edits the common setup: one SDR scanning a set of bands, plus the
//! web, decoding and contribute options. Saving patches those keys in the TOML
//! with toml_edit, so comments and anything the page doesn't know about (extra
//! receivers, fixed channels, command sources) survive. The new file is
//! validated by loading it before it replaces the old one.

use crate::config::Config;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub web: Web,
    pub station: Station,
    pub radio: Radio,
    pub decoding: Decoding,
    pub atlas: Atlas,
    pub contribute: Contribute,
    /// Read-only: the config has parts the page can't edit (shown as a note).
    pub advanced_config: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Web {
    pub port: u16,
    /// Reachable from other computers (binds 0.0.0.0); requires a password.
    pub lan: bool,
    pub password_set: bool,
    /// Write-only: a new password ("" leaves it unchanged).
    pub password: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Station {
    pub name: String,
    pub location: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Radio {
    /// SoapySDR args, from the device list.
    pub device: String,
    pub bands: Vec<String>,
    pub gain: Option<f64>,
    pub gains: BTreeMap<String, f64>,
    pub antenna: Option<String>,
    pub settings: BTreeMap<String, String>,
    pub ppm: f64,
    pub upconverter_hz: f64,
    /// Chosen automatically from the bands unless set (Expert).
    pub rate: Option<u64>,
    pub center_hz: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Decoding {
    pub decoders: usize,
    pub connect_only: bool,
    pub threshold_db: Option<f64>,
    pub stale_hours: f64,
    pub gone_days: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Atlas {
    pub url: String,
    /// openRMS-Accounts (sign-in for "Link to openRMS"); empty = https://auth.openrms.dev
    pub accounts_url: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Contribute {
    pub enabled: bool,
    pub token_set: bool,
    /// Write-only: a new watcher key; "" clears it; absent leaves it.
    pub token: Option<String>,
}

pub fn hash_password(pw: &str) -> String {
    let salt = ulid::Ulid::generate().to_string();
    let h = Sha256::digest(format!("{salt}:{pw}").as_bytes());
    format!("{salt}${}", h.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

pub fn check_password(stored: &str, pw: &str) -> bool {
    let Some((salt, hex)) = stored.split_once('$') else { return false };
    let h = Sha256::digest(format!("{salt}:{pw}").as_bytes());
    let got: String = h.iter().map(|b| format!("{b:02x}")).collect();
    // constant-time compare
    got.len() == hex.len() && got.bytes().zip(hex.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

pub fn from_config(cfg: &Config) -> Settings {
    let (host, port) = cfg.watch.listen.rsplit_once(':').unwrap_or(("127.0.0.1", "17373"));
    let w = cfg.widebands.first();
    let rx = w.map(|w| cfg.receiver(&w.receiver)).or(cfg.receivers.first());
    Settings {
        web: Web {
            port: port.parse().unwrap_or(17373),
            lan: !(host == "127.0.0.1" || host == "localhost" || host == "::1"),
            password_set: cfg.web.password_hash.is_some(),
            password: None,
        },
        station: Station {
            name: rx.map(|r| r.name.clone()).unwrap_or_default(),
            location: rx.map(|r| r.location.clone()).unwrap_or_default(),
        },
        radio: Radio {
            device: w.and_then(|w| w.device.clone()).unwrap_or_default(),
            bands: w
                .map(|w| w.segments.iter().filter_map(|[a, b]| crate::plan::band_name(*a, *b).map(String::from)).collect())
                .unwrap_or_default(),
            gain: w.and_then(|w| w.gain),
            gains: w.map(|w| w.gains.clone()).unwrap_or_default(),
            antenna: w.and_then(|w| w.antenna.clone()),
            settings: w.map(|w| w.settings.clone()).unwrap_or_default(),
            ppm: rx.map(|r| r.ppm).unwrap_or(0.0),
            upconverter_hz: w.map(|w| w.upconverter_hz).unwrap_or(0.0),
            rate: w.map(|w| w.rate),
            center_hz: w.map(|w| w.center_hz),
        },
        decoding: Decoding {
            decoders: w.map(|w| w.decoders).unwrap_or(8),
            connect_only: cfg.watch.connect_only,
            threshold_db: w.and_then(|w| w.threshold_db),
            stale_hours: cfg.watch.stale_hours,
            gone_days: cfg.watch.gone_days,
        },
        atlas: Atlas {
            url: cfg.atlas.url.clone().unwrap_or_default(),
            accounts_url: cfg.atlas.accounts_url.clone().unwrap_or_default(),
        },
        contribute: Contribute {
            enabled: cfg.contribute.enabled,
            token_set: cfg.contribute.token.as_deref().is_some_and(|t| !t.is_empty()),
            token: None,
        },
        advanced_config: cfg.widebands.len() > 1
            || !cfg.channels.is_empty()
            || w.is_some_and(|w| w.device.is_none()),
    }
}

fn table<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    if !doc.contains_table(name) {
        doc[name] = Item::Table(Table::new());
    }
    doc[name].as_table_mut().unwrap()
}

fn first_of<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    if doc.get(name).and_then(|i| i.as_array_of_tables()).is_none_or(|a| a.is_empty()) {
        let mut a = ArrayOfTables::new();
        a.push(Table::new());
        doc[name] = Item::ArrayOfTables(a);
    }
    doc[name].as_array_of_tables_mut().unwrap().get_mut(0).unwrap()
}

/// Apply the page's settings to the TOML text. `plan` supplies rate/centre
/// when they are automatic.
pub fn apply(text: &str, s: &Settings, plan: Option<&crate::plan::Plan>) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing the config")?;
    {
        let wt = table(&mut doc, "watch");
        let host = if s.web.lan { "0.0.0.0" } else { "127.0.0.1" };
        wt["listen"] = value(format!("{host}:{}", s.web.port));
        wt["connect_only"] = value(s.decoding.connect_only);
        wt["stale_hours"] = value(s.decoding.stale_hours);
        wt["gone_days"] = value(s.decoding.gone_days);
    }
    if let Some(pw) = s.web.password.as_deref().filter(|p| !p.is_empty()) {
        table(&mut doc, "web")["password_hash"] = value(hash_password(pw));
    }
    let rx_id = {
        let r = first_of(&mut doc, "receiver");
        if r.get("id").is_none() {
            r["id"] = value("sdr");
        }
        r["name"] = value(s.station.name.clone());
        r["location"] = value(s.station.location.clone());
        r["ppm"] = value(s.radio.ppm);
        if r.get("kind").is_none() {
            r["kind"] = value("local-sdr");
        }
        r["id"].as_str().unwrap_or("sdr").to_string()
    };
    if !s.radio.device.is_empty() {
        let w = first_of(&mut doc, "wideband");
        w["receiver"] = value(rx_id);
        w["device"] = value(s.radio.device.clone());
        w.remove("command");
        w.remove("file");
        let mut segs = Array::new();
        for b in &s.radio.bands {
            let (lo, hi) = crate::plan::band(b).with_context(|| format!("unknown band {b}"))?;
            let mut a = Array::new();
            a.push(lo as i64);
            a.push(hi as i64);
            segs.push(a);
        }
        w["segments"] = value(segs);
        match plan {
            Some(p) => {
                w["rate"] = value(p.rate as i64);
                w["center_hz"] = value(p.center_hz as i64);
            }
            None => {
                let (Some(r), Some(c)) = (s.radio.rate, s.radio.center_hz) else { bail!("rate and centre are needed") };
                w["rate"] = value(r as i64);
                w["center_hz"] = value(c as i64);
            }
        }
        // the driver picks a filter to match the new rate
        w.remove("bandwidth");
        match s.radio.gain {
            Some(g) => w["gain"] = value(g),
            None => {
                w.remove("gain");
            }
        }
        let mut gains = InlineTable::new();
        for (k, v) in &s.radio.gains {
            gains.insert(k, (*v).into());
        }
        if gains.is_empty() {
            w.remove("gains");
        } else {
            w["gains"] = value(gains);
        }
        match &s.radio.antenna {
            Some(a) if !a.is_empty() => w["antenna"] = value(a.clone()),
            _ => {
                w.remove("antenna");
            }
        }
        let mut st = InlineTable::new();
        for (k, v) in &s.radio.settings {
            st.insert(k, v.clone().into());
        }
        if st.is_empty() {
            w.remove("settings");
        } else {
            w["settings"] = value(st);
        }
        if s.radio.upconverter_hz != 0.0 {
            w["upconverter_hz"] = value(s.radio.upconverter_hz);
        } else {
            w.remove("upconverter_hz");
        }
        w["decoders"] = value(s.decoding.decoders.clamp(1, 64) as i64);
        match s.decoding.threshold_db {
            Some(t) => w["threshold_db"] = value(t),
            None => {
                w.remove("threshold_db");
            }
        }
    }
    {
        // the section was [directory] before openRMS-Atlas: drop the old name
        doc.remove("directory");
        let d = table(&mut doc, "atlas");
        if s.atlas.url.trim().is_empty() {
            d.remove("url");
        } else {
            d["url"] = value(s.atlas.url.trim().trim_end_matches('/'));
        }
        if s.atlas.accounts_url.trim().is_empty() {
            d.remove("accounts_url");
        } else {
            d["accounts_url"] = value(s.atlas.accounts_url.trim().trim_end_matches('/'));
        }
    }
    {
        let c = table(&mut doc, "contribute");
        c["enabled"] = value(s.contribute.enabled);
        match s.contribute.token.as_deref() {
            Some("") => {
                c.remove("token");
            }
            Some(t) => {
                let t = t.trim();
                if !t.starts_with("ormsw_") {
                    bail!("that isn't a watcher key (they start with ormsw_)");
                }
                c["token"] = value(t);
            }
            None => {}
        }
    }
    Ok(doc.to_string())
}

/// Write `text` to `path` after checking it loads; keeps a .bak of the old file.
pub fn save(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("toml.new");
    std::fs::write(&tmp, text)?;
    if let Err(e) = Config::load(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.context("the new settings don't make a valid config"));
    }
    if path.exists() {
        std::fs::copy(path, path.with_extension("toml.bak"))?;
    }
    // the file can hold a watcher key: owner-only
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"# my watcher
[watch]
mercury = "/opt/mercury"   # keep this comment
db = "w.db"

[[receiver]]
id = "hackrf"
name = "HackRF"
ppm = -24.06

[[wideband]]
receiver = "hackrf"
command = "hackrf_transfer -r - -f {center_hz}"
format = "cs8"
rate = 12800000
center_hz = 12200000
segments = [[7000000, 7300000]]
"#;

    #[test]
    fn old_directory_section_loads_and_is_renamed_on_save() {
        let dir = std::env::temp_dir().join(format!("orw-atlas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("watch.toml");
        let old = format!("{BASE}\n[directory]\nurl = \"https://atlas.openrms.dev\"\n");
        std::fs::write(&p, &old).unwrap();
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.atlas.url.as_deref(), Some("https://atlas.openrms.dev"), "[directory] is an alias of [atlas]");
        let s = from_config(&cfg);
        let plan = crate::plan::Plan { rate: 12_800_000, center_hz: 12_200_000, segments: vec![], cores: 1.0 };
        let text = apply(&old, &s, Some(&plan)).unwrap();
        assert!(text.contains("[atlas]") && !text.contains("[directory]"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn round_trip_keeps_comments_and_validates() {
        let dir = std::env::temp_dir().join(format!("orw-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("watch.toml");
        std::fs::write(&p, BASE).unwrap();
        let cfg = Config::load(&p).unwrap();
        let mut s = from_config(&cfg);
        assert_eq!(s.radio.bands, vec!["40m"]);
        assert!(s.advanced_config, "command source is beyond the page");
        s.radio.device = "driver=hackrf".into();
        s.radio.bands = vec!["40m".into(), "20m".into()];
        s.radio.gains = [("LNA".to_string(), 24.0)].into();
        s.web.port = 18000;
        s.web.password = Some("hunter22".into());
        s.atlas.url = "http://localhost:8787/".into();
        s.contribute = Contribute { enabled: true, token_set: false, token: Some("ormsw_abc".into()) };
        let plan = crate::plan::Plan { rate: 16_000_000, center_hz: 12_200_000, segments: vec![], cores: 1.0 };
        let text = apply(BASE, &s, Some(&plan)).unwrap();
        assert!(text.contains("# keep this comment"));
        assert!(!text.contains("hackrf_transfer"), "device replaces the command source");
        save(&p, &text).unwrap();
        let cfg2 = Config::load(&p).unwrap();
        assert_eq!(cfg2.watch.listen, "127.0.0.1:18000");
        assert_eq!(cfg2.widebands[0].segments.len(), 2);
        assert_eq!(cfg2.widebands[0].rate, 16_000_000);
        assert_eq!(cfg2.contribute.token.as_deref(), Some("ormsw_abc"));
        assert!(check_password(cfg2.web.password_hash.as_deref().unwrap(), "hunter22"));
        assert!(!check_password(cfg2.web.password_hash.as_deref().unwrap(), "wrong"));
        let s2 = from_config(&cfg2);
        assert!(s2.contribute.token_set && s2.web.password_set && !s2.advanced_config);
        assert!(apply(&text, &Settings { contribute: Contribute { token: Some("nope".into()), ..Default::default() }, ..s2 }, Some(&plan)).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
