//! What the engine is doing right now, for the status and settings pages:
//! SDR levels (clipping), a spectrum per scanned segment, and the decoder pool.
//! The engine owns the SDR, so the web pages read it from here instead of
//! opening the device themselves.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Default, Clone)]
pub struct RxLive {
    /// Percentage of I/Q samples at or near full scale over the last half second.
    pub clip_pct: f64,
    /// RMS level, dB relative to full scale.
    pub rms_dbfs: f64,
    pub msps: f64,
    pub updated_ms: i64,
    pub error: Option<String>,
}

#[derive(Serialize, Default, Clone)]
pub struct SegLive {
    pub receiver: String,
    pub lo_hz: u64,
    pub hi_hz: u64,
    /// Smoothed power (dB, arbitrary reference) across the segment, low to high.
    pub db: Vec<f32>,
    /// Local noise floor at the same points.
    pub floor: Vec<f32>,
    pub threshold_db: f64,
    pub updated_ms: i64,
}

#[derive(Serialize, Default, Clone)]
pub struct DecLive {
    pub idx: usize,
    /// idle | listening | confirmed | pinned
    pub state: &'static str,
    pub dial_hz: Option<u64>,
    pub since_ms: Option<i64>,
}

#[derive(Serialize, Default, Clone)]
pub struct Live {
    pub started_ms: i64,
    pub receivers: BTreeMap<String, RxLive>,
    pub segments: Vec<SegLive>,
    pub decoders: Vec<DecLive>,
    /// Capabilities of the SDRs in use, probed when they were opened.
    pub caps: BTreeMap<String, serde_json::Value>,
}

static LIVE: OnceLock<Mutex<Live>> = OnceLock::new();

pub fn get() -> &'static Mutex<Live> {
    LIVE.get_or_init(|| Mutex::new(Live { started_ms: crate::db::now_ms(), ..Default::default() }))
}

pub fn snapshot() -> Live {
    get().lock().unwrap().clone()
}

/// Level statistics for one block of IQ.
pub fn levels(x: &[num_complex::Complex32]) -> (f64, f64) {
    if x.is_empty() {
        return (0.0, -120.0);
    }
    let mut clip = 0usize;
    let mut p = 0.0f64;
    for s in x {
        if s.re.abs() >= 0.98 || s.im.abs() >= 0.98 {
            clip += 1;
        }
        p += s.norm_sqr() as f64;
    }
    (100.0 * clip as f64 / x.len() as f64, 10.0 * (p / x.len() as f64 / 2.0).max(1e-12).log10())
}

/// Reduce a spectrum to at most `n` points by max-hold (keeps narrow signals visible).
pub fn decimate_max(v: &[f32], n: usize) -> Vec<f32> {
    if v.len() <= n {
        return v.to_vec();
    }
    let step = v.len() as f64 / n as f64;
    (0..n)
        .map(|i| {
            let a = (i as f64 * step) as usize;
            let b = (((i + 1) as f64 * step) as usize).min(v.len()).max(a + 1);
            v[a..b].iter().copied().fold(f32::MIN, f32::max)
        })
        .collect()
}
