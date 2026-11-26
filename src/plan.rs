//! Turn "these bands on this SDR" into a sample rate and centre frequency.

use serde::Serialize;

/// Amateur bands a watcher can scan (IARU Region 2 edges; wide enough for
/// Regions 1 and 3 Mercury activity too).
pub const BANDS: &[(&str, u64, u64)] = &[
    ("160m", 1_800_000, 2_000_000),
    ("80m", 3_500_000, 4_000_000),
    ("60m", 5_330_500, 5_406_500),
    ("40m", 7_000_000, 7_300_000),
    ("30m", 10_100_000, 10_150_000),
    ("20m", 14_000_000, 14_350_000),
    ("17m", 18_068_000, 18_168_000),
    ("15m", 21_000_000, 21_450_000),
    ("12m", 24_890_000, 24_990_000),
    ("10m", 28_000_000, 29_700_000),
    ("6m", 50_000_000, 54_000_000),
];

pub fn band(name: &str) -> Option<(u64, u64)> {
    BANDS.iter().find(|b| b.0 == name).map(|b| (b.1, b.2))
}

pub fn band_name(lo: u64, hi: u64) -> Option<&'static str> {
    BANDS.iter().find(|b| b.1 == lo && b.2 == hi).map(|b| b.0)
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Plan {
    pub rate: u64,
    pub center_hz: u64,
    pub segments: Vec<[u64; 2]>,
    /// Rough CPU estimate in cores (scan DSP + decoders), for the settings page.
    pub cores: f64,
}

/// Usable fraction of the sample rate (filter roll-off at the edges).
const USABLE: f64 = 0.45;
/// Keep band edges this far from the centre (DC spike, and the HackRF's noisy
/// region around the tuned frequency).
const DC_GUARD_HZ: f64 = 300_000.0;

/// Pick the slowest rate (from `rates`) and a centre that fits every band,
/// keeping the centre as far from any band as possible.
/// `tune_min_hz`/`tune_max_hz` bound the centre the SDR can tune to
/// (`upconverter_hz` is added before checking).
pub fn plan(
    bands: &[(u64, u64)],
    rates: &[u64],
    tune_min_hz: f64,
    tune_max_hz: f64,
    upconverter_hz: f64,
    decoders: usize,
) -> Result<Plan, String> {
    if bands.is_empty() {
        return Err("choose at least one band".into());
    }
    let lo = bands.iter().map(|b| b.0).min().unwrap() as f64;
    let hi = bands.iter().map(|b| b.1).max().unwrap() as f64;
    let mut rates: Vec<u64> = rates.iter().copied().filter(|r| r % 8000 == 0).collect();
    rates.sort_unstable();
    for &rate in &rates {
        let half = rate as f64 * USABLE;
        if hi - lo > 2.0 * half {
            continue;
        }
        // candidate centres every 25 kHz across the feasible window
        let (cmin, cmax) = (hi - half, lo + half);
        let mut best: Option<(f64, f64)> = None; // (score, centre)
        let mut c = cmin;
        while c <= cmax + 1.0 {
            let tune = c + upconverter_hz;
            if tune >= tune_min_hz && tune <= tune_max_hz {
                // distance from centre to the nearest band (0 if inside one)
                let d = bands
                    .iter()
                    .map(|&(a, b)| {
                        let (a, b) = (a as f64, b as f64);
                        if c >= a && c <= b { 0.0 } else { (a - c).abs().min((b - c).abs()) }
                    })
                    .fold(f64::MAX, f64::min);
                if d >= DC_GUARD_HZ.min(half * 0.1) && best.is_none_or(|(s, _)| d > s) {
                    best = Some((d, c));
                }
            }
            c += 25_000.0;
        }
        if let Some((_, c)) = best {
            let c = (c / 1000.0).round() as u64 * 1000;
            let segs: Vec<[u64; 2]> = bands.iter().map(|&(a, b)| [a, b]).collect();
            // measured: ~43.8 MS/s of scan DSP per core for 3 segments on a laptop
            let cores = rate as f64 / 1e6 * segs.len() as f64 / (43.8 * 3.0) + decoders as f64 * 0.21;
            return Ok(Plan { rate, center_hz: c, segments: segs, cores: (cores * 10.0).round() / 10.0 });
        }
    }
    if tune_min_hz > lo + upconverter_hz {
        return Err(format!(
            "this SDR tunes from {:.1} MHz: HF needs an upconverter (set its offset)",
            tune_min_hz / 1e6
        ));
    }
    let max = rates.last().copied().unwrap_or(0) as f64;
    Err(format!(
        "these bands span {:.1} MHz but this SDR covers at most about {:.1} MHz at once; choose fewer or closer bands",
        (hi - lo) / 1e6,
        max * 2.0 * USABLE / 1e6
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HACKRF: [u64; 5] = [4_000_000, 8_000_000, 10_000_000, 16_000_000, 20_000_000];

    #[test]
    fn hackrf_40_to_17m_takes_16_msps_with_centre_off_band() {
        let b: Vec<_> = ["40m", "30m", "20m", "17m"].iter().map(|n| band(n).unwrap()).collect();
        let p = plan(&b, &HACKRF, 1e6, 6e9, 0.0, 8).unwrap();
        assert_eq!(p.rate, 16_000_000);
        for &(lo, hi) in &b {
            assert!((lo as f64) >= p.center_hz as f64 - 7.2e6 && (hi as f64) <= p.center_hz as f64 + 7.2e6);
            assert!(p.center_hz < lo || p.center_hz > hi, "centre inside a band");
        }
    }

    #[test]
    fn single_band_uses_slowest_rate() {
        let p = plan(&[band("40m").unwrap()], &HACKRF, 1e6, 6e9, 0.0, 4).unwrap();
        assert_eq!(p.rate, 4_000_000);
    }

    #[test]
    fn pluto_without_upconverter_explains() {
        let e = plan(&[band("40m").unwrap()], &[2_400_000], 70e6, 6e9, 0.0, 4).unwrap_err();
        assert!(e.contains("upconverter"), "{e}");
        // with a 125 MHz upconverter it works
        assert!(plan(&[band("40m").unwrap()], &[2_400_000], 70e6, 6e9, 125e6, 4).is_ok());
    }

    #[test]
    fn too_wide_for_rtl() {
        let b = [band("40m").unwrap(), band("20m").unwrap()];
        let e = plan(&b, &[2_048_000, 2_400_000], 0.0, 1.7e9, 0.0, 4).unwrap_err();
        assert!(e.contains("fewer"), "{e}");
    }
}
