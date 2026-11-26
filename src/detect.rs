//! Find Mercury connect bursts in a band segment.
//!
//! CALL / ACCEPT / CQ / DISCONNECT all go out on DATAC16: an OFDM burst about
//! 187 Hz wide and 3.1 s long, centred on the station's dial + 1500 Hz. The
//! detector averages ~0.6 s of FFTs, smooths across bins, compares against a
//! local noise floor, and finds runs of elevated bins of that width. A run is
//! reported when it *starts* after its frequency was quiet for 3 s and is seen
//! on two looks, unless that frequency has been occupied more than 30% of the
//! time over the last couple of minutes. A connect frame starts out of silence;
//! interference spurs and carriers are always there (never quiet), and ones that
//! flicker are caught by the occupancy test. The caller replays its pre-roll
//! buffer, so the start of the burst still reaches the decoder. The centre estimate is good to a few Hz, well
//! inside Mercury's +/-50 Hz acquisition window.

use num_complex::Complex32 as C32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

pub struct Detection {
    /// Centre frequency offset from the segment stream's 0 Hz.
    pub offset_hz: f64,
    pub width_hz: f64,
    pub snr_db: f64,
}

struct Candidate {
    offset: f64,
    hits: u32,
    last_frame: u64,
    /// started out of silence on a rarely used frequency
    fresh: bool,
    reported: bool,
}

/// How long a frequency must have been quiet before a burst there counts as new.
const QUIET_S: f64 = 3.0;
/// Occupancy averaging time, and the share above which a frequency is "busy".
const OCC_TAU_S: f64 = 120.0;
const OCC_MAX: f64 = 0.3;

pub struct Detector {
    fs: f64,
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    pending: Vec<C32>,
    acc: Vec<f32>,
    acc_count: usize,
    averages: usize,
    lo_bin: usize,
    hi_bin: usize,
    frame: u64,
    cands: Vec<Candidate>,
    /// per search bin: last look it was above threshold, and occupancy (EMA)
    last_on: Vec<u64>,
    occ: Vec<f64>,
    /// last look's smoothed spectrum and floor (dB), for the status page
    snap: Option<(Vec<f32>, Vec<f32>)>,
    pub min_width: f64,
    pub max_width: f64,
    pub threshold_db: f64,
}

impl Detector {
    /// `band_lo` / `band_hi`: the part of the stream to search, in Hz offsets.
    pub fn new(fs: f64, band_lo: f64, band_hi: f64) -> Detector {
        let n = ((fs / 8.0) as usize).next_power_of_two();
        let fft = FftPlanner::new().plan_fft_forward(n);
        let window = (0..n)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32)
            .collect();
        let bin = fs / n as f64;
        let to_bin = |f: f64| ((f / bin).round() as i64).rem_euclid(n as i64) as usize;
        // work in fftshifted bin indices: 0 Hz at n/2
        let lo = (to_bin(band_lo) + n / 2) % n;
        let hi = (to_bin(band_hi) + n / 2) % n;
        let averages = ((0.6 * fs / n as f64).round() as usize).max(2);
        Detector {
            fs, n, fft, window,
            pending: Vec::with_capacity(n),
            acc: vec![0.0; n],
            acc_count: 0,
            averages,
            lo_bin: lo.min(hi),
            hi_bin: lo.max(hi),
            frame: 0,
            cands: Vec::new(),
            last_on: Vec::new(),
            occ: Vec::new(),
            snap: None,
            min_width: 110.0,
            max_width: 320.0,
            threshold_db: 4.0,
        }
    }

    /// The latest spectrum (once per look), for the status page.
    pub fn take_snapshot(&mut self) -> Option<(Vec<f32>, Vec<f32>)> {
        self.snap.take()
    }

    pub fn bin_hz(&self) -> f64 {
        self.fs / self.n as f64
    }

    /// Feed samples; returns newly confirmed bursts.
    pub fn process(&mut self, x: &[C32]) -> Vec<Detection> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < x.len() {
            let take = (self.n - self.pending.len()).min(x.len() - i);
            self.pending.extend_from_slice(&x[i..i + take]);
            i += take;
            if self.pending.len() == self.n {
                let mut buf: Vec<C32> = self.pending.iter().zip(&self.window).map(|(s, w)| s * *w).collect();
                self.pending.clear();
                self.fft.process(&mut buf);
                for (a, z) in self.acc.iter_mut().zip(&buf) {
                    *a += z.norm_sqr();
                }
                self.acc_count += 1;
                if self.acc_count == self.averages {
                    out.extend(self.analyse());
                    self.acc.iter_mut().for_each(|a| *a = 0.0);
                    self.acc_count = 0;
                }
            }
        }
        out
    }

    fn analyse(&mut self) -> Vec<Detection> {
        self.frame += 1;
        let n = self.n;
        let bin = self.bin_hz();
        let (lo, hi) = (self.lo_bin, self.hi_bin);
        if hi <= lo + 8 {
            return vec![];
        }
        // shifted power spectrum over the band, in dB, smoothed over ~40 Hz
        let shifted = |k: usize| self.acc[(k + n / 2) % n];
        let len = hi - lo;
        let raw: Vec<f32> = (lo..hi).map(|k| shifted(k).max(1e-20)).collect();
        let sw = ((40.0 / bin).round() as usize).max(1) | 1;
        let half = sw / 2;
        let mut prefix = vec![0.0f64; len + 1];
        for (i, v) in raw.iter().enumerate() {
            prefix[i + 1] = prefix[i] + *v as f64;
        }
        let sm: Vec<f64> = (0..len)
            .map(|i| {
                let a = i.saturating_sub(half);
                let b = (i + half + 1).min(len);
                10.0 * ((prefix[b] - prefix[a]) / (b - a) as f64).log10()
            })
            .collect();
        // local floor: 25th percentile of ~3 kHz blocks, interpolated
        let blk = ((3000.0 / bin) as usize).clamp(16, len);
        let nblk = len.div_ceil(blk);
        let floors: Vec<f64> = (0..nblk)
            .map(|b| {
                let mut v: Vec<f64> = sm[b * blk..((b + 1) * blk).min(len)].to_vec();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                v[v.len() / 4]
            })
            .collect();
        let floor_at = |i: usize| {
            let pos = (i as f64 + 0.5) / blk as f64 - 0.5;
            let a = pos.floor().clamp(0.0, (nblk - 1) as f64) as usize;
            let b = (a + 1).min(nblk - 1);
            let t = (pos - a as f64).clamp(0.0, 1.0);
            floors[a] * (1.0 - t) + floors[b] * t
        };
        // runs above threshold
        let mut dets = Vec::new();
        let mut i = 0;
        while i < len {
            if sm[i] - floor_at(i) > self.threshold_db {
                let start = i;
                let (mut wsum, mut fsum, mut peak) = (0.0, 0.0, f64::MIN);
                while i < len && sm[i] - floor_at(i) > self.threshold_db * 0.5 {
                    let ex = 10f64.powf((sm[i] - floor_at(i)) / 10.0) - 1.0;
                    wsum += ex;
                    fsum += ex * i as f64;
                    peak = peak.max(sm[i] - floor_at(i));
                    i += 1;
                }
                // the smoothing widens a run by about one smoothing window
                let width = ((i - start) as f64 - sw as f64 + 1.0).max(1.0) * bin;
                if width >= self.min_width && width <= self.max_width && wsum > 0.0 {
                    let c = fsum / wsum + lo as f64; // shifted bin index
                    let off = (c - (n / 2) as f64) * bin;
                    dets.push((Detection { offset_hz: off, width_hz: width, snr_db: peak }, start, i));
                }
            }
            i += 1;
        }
        // onset + occupancy: judge each run against the history *before* this look
        let frame = self.frame;
        let look_s = self.averages as f64 * self.n as f64 / self.fs;
        if self.last_on.len() != len {
            // at start-up nothing has been seen quiet yet
            self.last_on = vec![frame; len];
            self.occ = vec![0.0; len];
        }
        let quiet = (QUIET_S / look_s).ceil() as u64;
        let mut out = Vec::new();
        for (d, a, b) in dets {
            // quiet: at most a fifth of the run's bins were active in the last QUIET_S
            let recent = (a..b).filter(|&k| frame - self.last_on[k] <= quiet).count();
            let fresh_now = recent * 5 <= (b - a);
            let occ = (a..b).map(|k| self.occ[k]).sum::<f64>() / (b - a).max(1) as f64;
            if let Some(c) = self.cands.iter_mut().find(|c| (c.offset - d.offset_hz).abs() < 25.0) {
                if frame - c.last_frame <= 2 {
                    c.hits += 1;
                } else {
                    // a new run on a known frequency
                    c.hits = 1;
                    c.fresh = fresh_now && occ < OCC_MAX;
                    c.reported = false;
                }
                c.offset = 0.5 * (c.offset + d.offset_hz);
                c.last_frame = frame;
                if c.hits >= 2 && c.fresh && !c.reported {
                    c.reported = true;
                    out.push(Detection { offset_hz: c.offset, ..d });
                }
            } else {
                self.cands.push(Candidate {
                    offset: d.offset_hz, hits: 1, last_frame: frame,
                    fresh: fresh_now && occ < OCC_MAX, reported: false,
                });
            }
        }
        self.cands.retain(|c| frame - c.last_frame <= 8);
        self.snap = Some((
            crate::live::decimate_max(&sm.iter().map(|v| *v as f32).collect::<Vec<_>>(), 800),
            crate::live::decimate_max(&(0..len).map(|k| floor_at(k) as f32).collect::<Vec<_>>(), 800),
        ));
        // update history
        let alpha = (look_s / OCC_TAU_S).min(1.0);
        let thr = self.threshold_db;
        for k in 0..len {
            let on = sm[k] - floor_at(k) > thr;
            if on {
                self.last_on[k] = frame;
            }
            self.occ[k] += alpha * ((on as u8 as f64) - self.occ[k]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdsp::Nco;

    /// Deterministic pseudo-noise.
    struct Lcg(u64);
    impl Lcg {
        fn g(&mut self) -> f32 {
            // sum of uniforms ~ gaussian
            let mut s = 0.0;
            for _ in 0..4 {
                self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                s += ((self.0 >> 33) as f32 / (1u64 << 31) as f32) - 0.5;
            }
            s
        }
    }

    /// Sum of tones spread over `width` Hz around `f`: an OFDM-like block.
    fn ofdm(fs: f64, f: f64, width: f64, n: usize, amp: f32) -> Vec<C32> {
        let k = 16;
        let mut ncos: Vec<Nco> = (0..k)
            .map(|i| Nco::new(f - width / 2.0 + width * i as f64 / (k - 1) as f64, fs))
            .collect();
        (0..n).map(|_| ncos.iter_mut().map(|o| o.next()).sum::<C32>() * (amp / k as f32)).collect()
    }

    #[test]
    fn finds_datac16_like_burst_and_ignores_carriers_and_wide_signals() {
        let fs = 64_000.0;
        let n = (fs * 10.0) as usize;
        let mut rng = Lcg(7);
        let mut x: Vec<C32> = (0..n).map(|_| C32::new(rng.g(), rng.g()) * 0.05).collect();
        let add = |x: &mut Vec<C32>, y: Vec<C32>| x.iter_mut().zip(y).for_each(|(a, b)| *a += b);
        // wanted: a 3.1 s burst from 4.5 s (after the start-up quiet period) to 7.6 s
        let mut burst = vec![C32::new(0.0, 0.0); n];
        let (b0, b1) = ((fs * 4.5) as usize, (fs * 7.6) as usize);
        burst[b0..b1].copy_from_slice(&ofdm(fs, 12_345.0, 187.0, b1 - b0, 0.08));
        add(&mut x, burst);
        // the same shape, but continuous: an interference spur or a beacon, never reported
        add(&mut x, ofdm(fs, -20_500.0, 187.0, n, 0.08));
        add(&mut x, ofdm(fs, -8_000.0, 2400.0, n, 0.3)); // wide data signal
        let mut cw = Nco::new(20_000.0, fs);
        add(&mut x, (0..n).map(|_| cw.next() * 0.5).collect()); // carrier
        let mut d = Detector::new(fs, -30_000.0, 30_000.0);
        let mut found = Vec::new();
        for c in x.chunks(10_000) {
            found.extend(d.process(c));
        }
        assert_eq!(found.len(), 1, "found {:?}", found.iter().map(|f| f.offset_hz).collect::<Vec<_>>());
        assert!((found[0].width_hz - 187.0).abs() < 120.0);
        assert!((found[0].offset_hz - 12_345.0).abs() < 15.0, "centre {}", found[0].offset_hz);
    }
}
