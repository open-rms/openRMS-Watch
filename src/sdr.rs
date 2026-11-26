//! SDR hardware through SoapySDR: enumerate devices, probe capabilities, stream IQ.

use anyhow::{Context, Result};
use num_complex::Complex32 as C32;
use serde::Serialize;
use soapysdr::Direction::Rx;
use std::collections::BTreeMap;

#[derive(Serialize, Clone, Debug)]
pub struct DeviceInfo {
    /// SoapySDR args string that opens exactly this device, e.g. "driver=hackrf,serial=...".
    pub args: String,
    pub driver: String,
    pub label: String,
    pub serial: Option<String>,
}

/// List attached SDRs (does not open them).
pub fn list() -> Result<Vec<DeviceInfo>> {
    let found = soapysdr::enumerate("").context("SoapySDR enumerate")?;
    let mut out = Vec::new();
    for a in found {
        let m: BTreeMap<String, String> = a.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let driver = m.get("driver").cloned().unwrap_or_default();
        let serial = m.get("serial").cloned();
        // the narrowest args that still identify the device
        let mut args = format!("driver={driver}");
        if let Some(s) = &serial {
            args += &format!(",serial={s}");
        } else if let Some(u) = m.get("uri") {
            args += &format!(",uri={u}");
        }
        out.push(DeviceInfo {
            label: m.get("label").cloned().unwrap_or_else(|| driver.clone()),
            args,
            driver,
            serial,
        });
    }
    Ok(out)
}

#[derive(Serialize, serde::Deserialize, Debug, Clone)]
pub struct Caps {
    pub args: String,
    pub hardware: String,
    pub freq_min_hz: f64,
    pub freq_max_hz: f64,
    pub hf_capable: bool,
    /// Rates usable for scanning: in the device's range and multiples of 8000.
    pub rates: Vec<u64>,
    pub gains: Vec<GainStage>,
    pub antennas: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Serialize, serde::Deserialize, Debug, Clone)]
pub struct GainStage {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

const SCAN_RATES: [u64; 10] =
    [2_048_000, 2_400_000, 3_200_000, 4_000_000, 6_400_000, 8_000_000, 10_000_000, 12_800_000, 16_000_000, 20_000_000];

/// Open a device briefly and report what it can do.
pub fn probe(args: &str) -> Result<Caps> {
    let dev = soapysdr::Device::new(args).with_context(|| format!("opening {args}"))?;
    caps_of(&dev, args)
}

fn caps_of(dev: &soapysdr::Device, args: &str) -> Result<Caps> {
    let hardware = dev.hardware_key().unwrap_or_default();
    let fr = dev.frequency_range(Rx, 0)?;
    let freq_min_hz = fr.iter().map(|r| r.minimum).fold(f64::MAX, f64::min);
    let freq_max_hz = fr.iter().map(|r| r.maximum).fold(0.0, f64::max);
    let rr = dev.get_sample_rate_range(Rx, 0)?;
    let rates = SCAN_RATES
        .iter()
        .copied()
        .filter(|r| rr.iter().any(|g| (*r as f64) >= g.minimum - 1.0 && (*r as f64) <= g.maximum + 1.0))
        .collect();
    let gains = dev
        .list_gains(Rx, 0)?
        .into_iter()
        .filter_map(|g| {
            let r = dev.gain_element_range(Rx, 0, g.as_str()).ok()?;
            Some(GainStage { name: g, min: r.minimum, max: r.maximum, step: r.step })
        })
        .collect();
    let antennas = dev.antennas(Rx, 0).unwrap_or_default();
    let driver = args.split(',').find_map(|kv| kv.strip_prefix("driver=")).unwrap_or("").to_string();
    let mut notes = Vec::new();
    let hf_capable = freq_min_hz <= 3.5e6;
    match driver.as_str() {
        "rtlsdr" => notes.push(
            "RTL-SDR: HF needs direct sampling (set direct_samp=2 on a V3/V4) or an upconverter. Max about 2.4 MS/s, so one band at a time.".into(),
        ),
        "hackrf" => notes.push("HackRF: covers HF directly but is weak there; keep the amp off near strong stations and use an HF low-pass filter.".into()),
        "plutosdr" => notes.push("Pluto/Pluto+: tunes 70 MHz and up, so HF needs an upconverter (set its offset).".into()),
        _ => {}
    }
    if !hf_capable && driver != "plutosdr" {
        notes.push(format!("Lowest frequency {:.1} MHz: HF needs an upconverter.", freq_min_hz / 1e6));
    }
    Ok(Caps { args: args.to_string(), hardware, freq_min_hz, freq_max_hz, hf_capable, rates, gains, antennas, notes })
}

/// Settings applied when streaming.
#[derive(Clone, Debug, Default)]
pub struct StreamSettings {
    pub rate: f64,
    pub center_hz: f64,
    pub bandwidth: Option<f64>,
    pub gain: Option<f64>,
    pub gains: BTreeMap<String, f64>,
    pub antenna: Option<String>,
    pub settings: BTreeMap<String, String>,
}

pub struct Source {
    _dev: soapysdr::Device,
    stream: soapysdr::RxStream<C32>,
    buf: Vec<C32>,
}

impl Source {
    pub fn open(args: &str, s: &StreamSettings) -> Result<Source> {
        let dev = soapysdr::Device::new(args).with_context(|| format!("opening {args}"))?;
        // remember what it can do: the settings page can't open it while we stream
        if let Ok(c) = caps_of(&dev, args) {
            if let Ok(v) = serde_json::to_value(&c) {
                crate::live::get().lock().unwrap().caps.insert(args.to_string(), v);
            }
        }
        for (k, v) in &s.settings {
            dev.write_setting(k.as_str(), v.as_str()).with_context(|| format!("setting {k}={v}"))?;
        }
        if let Some(a) = &s.antenna {
            dev.set_antenna(Rx, 0, a.as_str())?;
        }
        dev.set_sample_rate(Rx, 0, s.rate)?;
        if let Some(b) = s.bandwidth {
            dev.set_bandwidth(Rx, 0, b)?;
        }
        if dev.has_gain_mode(Rx, 0).unwrap_or(false) {
            dev.set_gain_mode(Rx, 0, false)?; // manual gain: AGC pumping hides weak signals
        }
        if let Some(g) = s.gain {
            dev.set_gain(Rx, 0, g)?;
        }
        for (k, v) in &s.gains {
            dev.set_gain_element(Rx, 0, k.as_str(), *v).with_context(|| format!("gain {k}"))?;
        }
        dev.set_frequency(Rx, 0, s.center_hz, ())?;
        let mut stream = dev.rx_stream::<C32>(&[0])?;
        let mtu = stream.mtu().unwrap_or(65536).max(1024);
        stream.activate(None)?;
        Ok(Source { _dev: dev, stream, buf: vec![C32::new(0.0, 0.0); mtu] })
    }

    /// Read about `n` samples (blocks until they arrive or ~1 s passes).
    pub fn read_block(&mut self, n: usize, out: &mut Vec<C32>) -> Result<()> {
        out.clear();
        let mut timeouts = 0;
        while out.len() < n {
            match self.stream.read(&mut [&mut self.buf[..]], 1_000_000) {
                Ok(k) => out.extend_from_slice(&self.buf[..k]),
                Err(e) if e.code == soapysdr::ErrorCode::Timeout => {
                    timeouts += 1;
                    if timeouts > 3 {
                        anyhow::bail!("SDR stopped delivering samples");
                    }
                }
                // overflow: samples were lost upstream; keep going
                Err(e) if e.code == soapysdr::ErrorCode::Overflow => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
