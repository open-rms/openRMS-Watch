//! Complex-baseband DSP for the wideband receiver: NCO mixer, windowed-sinc
//! FIR design, multi-stage decimation, and USB channel extraction to 8 kHz audio.

use num_complex::Complex32 as C32;
use std::f64::consts::PI;

/// Odd-length Blackman windowed-sinc lowpass, DC gain 1.
/// `pass` and `stop` are the band edges in Hz at sample rate `fs`.
pub fn lowpass(fs: f64, pass: f64, stop: f64) -> Vec<f32> {
    assert!(stop > pass && stop <= fs / 2.0 + 1e-6 || stop > pass);
    let trans = (stop - pass).max(1.0);
    let mut n = (5.5 * fs / trans).ceil() as usize;
    n = n.clamp(15, 4001) | 1;
    let fc = (pass + stop) / 2.0 / fs; // cycles/sample
    let m = (n - 1) as f64 / 2.0;
    let mut h: Vec<f64> = (0..n)
        .map(|i| {
            let k = i as f64 - m;
            let sinc = if k == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * k).sin() / (PI * k) };
            let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos()
                + 0.08 * (4.0 * PI * i as f64 / (n - 1) as f64).cos();
            sinc * w
        })
        .collect();
    let s: f64 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= s);
    h.into_iter().map(|x| x as f32).collect()
}

/// Numerically controlled oscillator: multiplies by e^{j 2 pi f n / fs}.
pub struct Nco {
    ph: C32,
    step: C32,
    n: u32,
}

impl Nco {
    pub fn new(freq: f64, fs: f64) -> Nco {
        let w = 2.0 * PI * freq / fs;
        Nco { ph: C32::new(1.0, 0.0), step: C32::new(w.cos() as f32, w.sin() as f32), n: 0 }
    }
    #[inline]
    pub fn next(&mut self) -> C32 {
        let p = self.ph;
        self.ph *= self.step;
        self.n += 1;
        if self.n & 1023 == 0 {
            self.ph /= self.ph.norm();
        }
        p
    }
    pub fn mix(&mut self, x: &mut [C32]) {
        for s in x.iter_mut() {
            *s *= self.next();
        }
    }
}

/// FIR decimator by M with real taps (keeps its own history between calls).
pub struct Decim {
    taps: Vec<f32>,
    m: usize,
    buf: Vec<C32>,
    phase: usize,
}

impl Decim {
    pub fn new(taps: Vec<f32>, m: usize) -> Decim {
        let n = taps.len();
        Decim { taps, m, buf: vec![C32::new(0.0, 0.0); n - 1], phase: 0 }
    }
    pub fn process(&mut self, x: &[C32], out: &mut Vec<C32>) {
        let n = self.taps.len();
        self.buf.extend_from_slice(x);
        // output k uses buf[i .. i+n] where i = phase + k*m
        let mut i = self.phase;
        while i + n <= self.buf.len() {
            let w = &self.buf[i..i + n];
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (t, s) in self.taps.iter().zip(w) {
                re += t * s.re;
                im += t * s.im;
            }
            out.push(C32::new(re, im));
            i += self.m;
        }
        let keep_from = i.min(self.buf.len()).saturating_sub(0);
        // keep the last n-1 samples needed, shifting so that the next output index stays aligned
        let consumed = keep_from.min(self.buf.len() - (n - 1));
        self.buf.drain(..consumed);
        self.phase = i - consumed;
    }
}

/// Split a decimation factor into stages (largest first, each <= 10).
pub fn stage_factors(mut d: usize) -> Vec<usize> {
    let mut primes = Vec::new();
    let mut p = 2;
    while d > 1 {
        while d % p == 0 {
            primes.push(p);
            d /= p;
        }
        p += 1;
    }
    primes.sort_unstable_by(|a, b| b.cmp(a));
    let mut stages: Vec<usize> = Vec::new();
    for p in primes {
        // put into the smallest stage that stays <= 10, else start a new one
        if let Some(s) = stages.iter_mut().filter(|s| **s * p <= 10).min_by_key(|s| **s) {
            *s *= p;
        } else {
            stages.push(p);
        }
    }
    stages.sort_unstable_by(|a, b| b.cmp(a));
    stages
}

/// A chain of decimators from `fs_in` to `fs_in / d`, keeping +/-`pass` Hz clean.
/// The last stage's stopband edge is `final_stop` (for channel selectivity).
pub struct Chain {
    stages: Vec<Decim>,
    tmp: Vec<C32>,
}

impl Chain {
    pub fn new(fs_in: f64, d: usize, pass: f64, final_stop: f64) -> Chain {
        let factors = stage_factors(d);
        let mut fs = fs_in;
        let mut stages = Vec::new();
        for (i, &m) in factors.iter().enumerate() {
            let out = fs / m as f64;
            let last = i + 1 == factors.len();
            // Content between pass and out - pass may alias, but only onto
            // frequencies outside +/-pass, which later stages remove.
            let stop = if last { final_stop.min(out - pass) } else { out - pass };
            stages.push(Decim::new(lowpass(fs, pass, stop.max(pass + 1.0)), m));
            fs = out;
        }
        Chain { stages, tmp: Vec::new() }
    }
    pub fn process(&mut self, x: &[C32], out: &mut Vec<C32>) {
        if self.stages.is_empty() {
            out.extend_from_slice(x);
            return;
        }
        let mut cur = x.to_vec();
        let n = self.stages.len();
        for (i, st) in self.stages.iter_mut().enumerate() {
            self.tmp.clear();
            st.process(&cur, &mut self.tmp);
            if i + 1 == n {
                out.extend_from_slice(&self.tmp);
            } else {
                std::mem::swap(&mut cur, &mut self.tmp);
            }
        }
    }
}

/// Mercury's passband: carriers centred on 1500 Hz audio, widest mode 2750 Hz.
pub const AUDIO_CENTER: f64 = 1500.0;
const CH_PASS: f64 = 1400.0;
const CH_STOP: f64 = 1700.0;

/// Extract one USB channel from a complex stream: the signal centred at
/// `offset` Hz (relative to the stream's 0 Hz) comes out as 8 kHz real audio
/// centred on 1500 Hz, with a slow AGC.
pub struct UsbExtractor {
    nco: Nco,
    chain: Chain,
    up: Nco,
    tmp: Vec<C32>,
    base: Vec<C32>,
    gain: f32,
}

impl UsbExtractor {
    pub fn new(fs_in: f64, offset: f64) -> UsbExtractor {
        let d = (fs_in / 8000.0).round() as usize;
        assert!((fs_in - d as f64 * 8000.0).abs() < 1e-3, "rate must be a multiple of 8000");
        UsbExtractor {
            nco: Nco::new(-offset, fs_in),
            chain: Chain::new(fs_in, d, CH_PASS, CH_STOP),
            up: Nco::new(AUDIO_CENTER, 8000.0),
            tmp: Vec::new(),
            base: Vec::new(),
            gain: 0.0,
        }
    }

    pub fn process(&mut self, x: &[C32], audio: &mut Vec<i16>) {
        self.tmp.clear();
        self.tmp.extend_from_slice(x);
        self.nco.mix(&mut self.tmp);
        self.base.clear();
        self.chain.process(&self.tmp, &mut self.base);
        // AGC per 20 ms block, target RMS ~ -20 dBFS
        const TARGET: f32 = 3200.0;
        for blk in self.base.chunks(160) {
            let p: f32 = blk.iter().map(|z| z.norm_sqr()).sum::<f32>() / blk.len() as f32;
            let rms = (2.0 * p).sqrt().max(1e-9); // real part after upshift has half the power
            let want = TARGET / rms;
            if self.gain == 0.0 {
                self.gain = want;
            } else if want < self.gain {
                self.gain += (want - self.gain) * 0.3; // fast attack
            } else {
                self.gain += (want - self.gain) * 0.01; // slow release (~2 s)
            }
            for z in blk {
                let y = (z * self.up.next()).re * 2.0 * self.gain;
                audio.push(y.clamp(-32767.0, 32767.0) as i16);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(fs: f64, f: f64, n: usize, amp: f32) -> Vec<C32> {
        let mut o = Nco::new(f, fs);
        (0..n).map(|_| o.next() * amp).collect()
    }

    fn power_at(x: &[i16], f: f64, fs: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &s) in x.iter().enumerate() {
            let a = 2.0 * PI * f * n as f64 / fs;
            re += s as f64 * a.cos();
            im -= s as f64 * a.sin();
        }
        ((re * re + im * im).sqrt() / x.len() as f64).powi(2)
    }

    #[test]
    fn factors() {
        assert_eq!(stage_factors(50), vec![10, 5]);
        assert_eq!(stage_factors(1600).iter().product::<usize>(), 1600);
        assert!(stage_factors(1600).iter().all(|&s| s <= 10));
        assert_eq!(stage_factors(1), Vec::<usize>::new());
    }

    #[test]
    fn decim_chunking_is_seamless() {
        // processing in odd-sized chunks must equal processing in one go
        let x = tone(48000.0, 1234.0, 9000, 1.0);
        let mut a = Decim::new(lowpass(48000.0, 2000.0, 6000.0), 6);
        let mut b = Decim::new(lowpass(48000.0, 2000.0, 6000.0), 6);
        let (mut oa, mut ob) = (Vec::new(), Vec::new());
        a.process(&x, &mut oa);
        for c in x.chunks(37) {
            b.process(c, &mut ob);
        }
        assert_eq!(oa.len(), ob.len());
        for (p, q) in oa.iter().zip(&ob) {
            assert!((p - q).norm() < 1e-4);
        }
    }

    #[test]
    fn usb_extracts_upper_sideband_only() {
        let fs = 400_000.0;
        let center = 37_000.0; // channel centre offset in the stream
        // wanted: a carrier 300 Hz above centre -> 1800 Hz audio
        // mirror: 300 Hz below -> 1200 Hz audio; neighbour 3 kHz away -> rejected
        let n = 400_000;
        let mut x = tone(fs, center + 300.0, n, 0.01);
        for (s, t) in x.iter_mut().zip(tone(fs, center + 3000.0, n, 0.3)) {
            *s += t;
        }
        let mut ex = UsbExtractor::new(fs, center);
        let mut audio = Vec::new();
        for c in x.chunks(8192) {
            ex.process(c, &mut audio);
        }
        assert!((audio.len() as i64 - 8000).abs() <= 2, "{}", audio.len());
        let a = &audio[2000..];
        let want = power_at(a, 1800.0, 8000.0);
        let mirror = power_at(a, 1200.0, 8000.0);
        let neighbour = power_at(a, 4500.0 % 4000.0, 8000.0).max(power_at(a, 3500.0, 8000.0));
        assert!(want > 1000.0 * mirror, "want {want} mirror {mirror}");
        assert!(want > 1000.0 * neighbour, "want {want} neighbour {neighbour}");
    }
}
